"""Offline tests for the degrade-phase fault executor (#1588). No SSH, no AWS."""

import copy
import io
import json
from pathlib import Path
import shlex
import tempfile
import unittest

import faults
from faults import Refusal, commands
from plan import expand

HERE = Path(__file__).parent
PHASE = 48 * 3600
START = 1_800_000_000
UNIT = 'botho-x.service'
IPS = {'controller': '10.0.0.10', 'n1': '10.0.0.11', 'n2': '10.0.0.12',
       'n3': '10.0.0.13', 'n4': '10.0.0.14', 'n5': '10.0.0.15'}


def inventory_rows():
    return [{'role': role, 'id': 'i-%02d' % n, 'state': 'running', 'public_ip': '203.0.113.%d' % n,
             'private_ip': ip, 'hostname': '%s.q20261024-x1-test.internal' % role}
            for n, (role, ip) in enumerate(IPS.items())]


PLAN = json.loads((HERE / 'x-series-plan.json').read_text())
SUMMARY, _ = expand(copy.deepcopy(PLAN))
STEPS = SUMMARY['degrade_steps']
INVENTORY = faults.load_inventory(inventory_rows())


def step(step_id):
    return copy.deepcopy(next(s for s in STEPS if s['id'] == step_id))


def argvs(entries):
    return [(e['role'], e['action'], e['argv']) for e in entries]


class Commands(unittest.TestCase):
    def test_d1_stop_restart_golden(self):
        rendered = commands(step('D1'), INVENTORY, UNIT)
        self.assertEqual(argvs(rendered['apply']), [
            ('n1', 'deadman', ['sudo', '-n', 'systemd-run', '--collect', '--on-active=1920',
                               '--timer-property=AccuracySec=1s', '--unit=botho-x-restore-D1-n1',
                               'systemctl', 'start', UNIT]),
            ('n1', 'stop', ['sudo', '-n', 'systemctl', 'stop', UNIT]),
        ])
        # D1 records Result: `timeout` means systemd had to SIGKILL.
        self.assertEqual(argvs(rendered['result']), [
            ('n1', 'result', ['systemctl', 'show', UNIT, '--property=Result', '--value'])])
        # The stop must be confirmed finished before restore_at (#1588 review).
        self.assertEqual(argvs(rendered['stopped']), [('n1', 'stopped', ['systemctl', 'is-active', UNIT])])
        self.assertEqual(rendered['stopped'][0]['expect'], {'one_of': ['inactive', 'failed']})
        self.assertEqual(argvs(rendered['restore']), [
            ('n1', 'restore', ['sudo', '-n', 'systemctl', 'start', UNIT])])
        # The disarm is its own group, run only after a passing restore and verify.
        self.assertEqual(argvs(rendered['disarm']), [
            ('n1', 'disarm', ['sudo', '-n', '/bin/sh', '-c',
                              'systemctl stop botho-x-restore-D1-n1.timer 2>/dev/null || ! '
                              'systemctl is-active --quiet botho-x-restore-D1-n1.timer'])])
        self.assertEqual(argvs(rendered['verify']), [('n1', 'verify', ['systemctl', 'is-active', UNIT])])
        self.assertEqual(rendered['verify'][0]['expect'], {'stdout': 'active'})

    def test_d2_kill_restart_golden(self):
        rendered = commands(step('D2'), INVENTORY, UNIT)
        self.assertEqual(argvs(rendered['apply'])[1:], [
            ('n2', 'kill', ['sudo', '-n', 'systemctl', 'kill', '-s', 'SIGKILL', UNIT])])
        self.assertIn('--on-active=720', rendered['apply'][0]['argv'])
        self.assertEqual(argvs(rendered['restore'])[0], ('n2', 'restore', ['sudo', '-n', 'systemctl', 'start', UNIT]))

    def test_d3_isolate_blocks_only_other_validators_on_gossip(self):
        rendered = commands(step('D3'), INVENTORY, UNIT)
        apply = rendered['apply']
        self.assertEqual(apply[0]['action'], 'deadman')
        rules = [e['argv'] for e in apply[1:]]
        self.assertTrue(all(e['role'] == 'n4' for e in apply))
        self.assertEqual(rules[0], ['sudo', '-n', 'iptables', '-w', '-N', 'botho-x-D3'])
        self.assertEqual(rules[-2:], [
            ['sudo', '-n', 'iptables', '-w', '-I', 'INPUT', '1', '-j', 'botho-x-D3'],
            ['sudo', '-n', 'iptables', '-w', '-I', 'OUTPUT', '1', '-j', 'botho-x-D3']])
        drops = rules[1:-2]
        self.assertEqual(len(drops), 4 * 2 * 2)
        self.assertEqual(drops[0], ['sudo', '-n', 'iptables', '-w', '-A', 'botho-x-D3', '-s', '10.0.0.11',
                                    '-p', 'tcp', '-m', 'multiport', '--ports', '17110', '-j', 'DROP'])
        peers = {r[r.index('-s') + 1] if '-s' in r else r[r.index('-d') + 1] for r in drops}
        self.assertEqual(peers, {IPS['n1'], IPS['n2'], IPS['n3'], IPS['n5']})
        everything = [token for e in apply + rendered['restore'] + rendered['verify'] for token in e['argv']]
        self.assertNotIn(IPS['controller'], ' '.join(everything))
        self.assertNotIn(IPS['n4'], ' '.join(everything))
        for rule in drops:
            self.assertEqual(rule[rule.index('--ports') + 1], '17110')
            self.assertEqual({p for p in rule if p in ('22', '443')}, set())
            self.assertEqual(rule[-2:], ['-j', 'DROP'])
        self.assertEqual({tuple(r[r.index('-p') + 1:r.index('-p') + 2]) for r in drops},
                         {('tcp',), ('udp',)})
        script = rendered['restore'][0]['argv'][-1]
        self.assertEqual(rendered['restore'][0]['argv'][:4], ['sudo', '-n', '/bin/sh', '-c'])
        self.assertEqual(script, (
            'while iptables -w -D INPUT -j botho-x-D3 2>/dev/null; do :; done; '
            'while iptables -w -D OUTPUT -j botho-x-D3 2>/dev/null; do :; done; '
            'iptables -w -F botho-x-D3 2>/dev/null; iptables -w -X botho-x-D3 2>/dev/null; '
            '! iptables -w -n -L botho-x-D3 >/dev/null 2>&1'))
        # The dead-man runs the very same idempotent restore.
        self.assertEqual(apply[0]['argv'][-3:], ['/bin/sh', '-c', script])
        self.assertEqual(rendered['verify'][0]['argv'], ['sudo', '-n', 'iptables', '-w', '-S'])
        self.assertEqual(rendered['verify'][0]['expect'], {'absent': ['botho-x']})

    def test_d4_netem_prio_band_scoped_to_gossip_port_both_directions(self):
        rendered = commands(step('D4'), INVENTORY, UNIT)
        self.assertEqual(argvs(rendered['apply'])[1:], [
            ('n5', 'netem', ['sudo', '-n', 'tc', 'qdisc', 'add', 'dev', 'ens5', 'root', 'handle', '1:',
                             'prio', 'bands', '4']),
            ('n5', 'netem', ['sudo', '-n', 'tc', 'qdisc', 'add', 'dev', 'ens5', 'parent', '1:4',
                             'handle', '40:', 'netem', 'loss', '20%', 'delay', '250ms']),
            ('n5', 'netem', ['sudo', '-n', 'tc', 'filter', 'add', 'dev', 'ens5', 'parent', '1:0',
                             'protocol', 'ip', 'prio', '1', 'u32', 'match', 'ip', 'dport', '17110',
                             '0xffff', 'flowid', '1:4']),
            ('n5', 'netem', ['sudo', '-n', 'tc', 'filter', 'add', 'dev', 'ens5', 'parent', '1:0',
                             'protocol', 'ip', 'prio', '1', 'u32', 'match', 'ip', 'sport', '17110',
                             '0xffff', 'flowid', '1:4']),
        ])
        self.assertEqual(rendered['restore'][0]['argv'], [
            'sudo', '-n', '/bin/sh', '-c',
            "tc qdisc del dev ens5 root 2>/dev/null; ! tc qdisc show dev ens5 | grep -Eq 'qdisc (prio|netem) '"])
        self.assertEqual(rendered['verify'][0]['argv'], ['tc', 'qdisc', 'show', 'dev', 'ens5'])
        self.assertNotIn('result', rendered)

    def test_d5_pair_arms_both_dead_men_before_either_stop(self):
        rendered = commands(step('D5'), INVENTORY, UNIT)
        self.assertEqual([(r, a) for r, a, _ in argvs(rendered['apply'])], [
            ('n1', 'deadman'), ('n2', 'deadman'), ('n1', 'stop_queue'), ('n2', 'stop_queue'),
            ('n1', 'stop'), ('n2', 'stop')])
        self.assertIn('--unit=botho-x-restore-D5-n1', rendered['apply'][0]['argv'])
        self.assertIn('--unit=botho-x-restore-D5-n2', rendered['apply'][1]['argv'])
        self.assertEqual(rendered['apply'][2]['argv'], ['sudo', '-n', 'systemctl', 'stop', '--no-block', UNIT])
        self.assertEqual([e['role'] for e in rendered['result']], ['n1', 'n2'])
        self.assertEqual([(e['role'], e['action']) for e in rendered['restore']],
                         [('n1', 'restore'), ('n2', 'restore')])
        self.assertEqual([(e['role'], e['action']) for e in rendered['disarm']],
                         [('n1', 'disarm'), ('n2', 'disarm')])
        self.assertEqual([e['role'] for e in rendered['stopped']], ['n1', 'n2'])

    def test_default_status_url_uses_shared_rpc_port(self):
        from fleet_provision import RPC_PORT
        self.assertEqual(RPC_PORT, 19090)
        self.assertEqual(faults.DEFAULT_STATUS_URL, 'http://127.0.0.1:19090/rpc')
        self.assertIn('http://127.0.0.1:19090/rpc', faults.status_argv())
        # One definition: faults.py imports the port, never restates it.
        self.assertNotRegex((HERE / 'faults.py').read_text(), r'\b(17101|19090)\b')

    def test_only_isolate_and_netem_lack_stop_confirmation(self):
        for s in STEPS:
            rendered = commands(s, INVENTORY, UNIT)
            self.assertEqual('stopped' in rendered, s['action'] in faults.RESTART_ACTIONS, s['id'])

    def test_every_apply_starts_with_dead_men_sized_from_now(self):
        for s in STEPS:
            rendered = commands(s, INVENTORY, UNIT)
            heads = rendered['apply'][:len(s['targets'])]
            self.assertEqual({e['action'] for e in heads}, {'deadman'})
            self.assertNotIn('deadman', [e['action'] for e in rendered['apply'][len(s['targets']):]])
            for e in heads:
                self.assertEqual(e['argv'][:3], ['sudo', '-n', 'systemd-run'])
        late = commands(step('D1'), INVENTORY, UNIT, now_offset=PHASE + 10)
        self.assertIn('--on-active=1910', late['apply'][0]['argv'])

    def test_rendering_never_takes_raw_inventory_text(self):
        rows = inventory_rows()
        rows[1]['private_ip'] = '10.0.0.11; reboot'
        with self.assertRaises(Refusal):
            faults.load_inventory(rows)
        with self.assertRaises(Refusal):
            commands(step('D1'), INVENTORY, 'botho;reboot.service')
        with self.assertRaises(Refusal):
            commands(step('D4'), INVENTORY, UNIT, dev='ens5;reboot')


class Refusals(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def main(self, plan=None, rows=None, unit=UNIT, extra=()):
        (self.dir / 'plan.json').write_text(json.dumps(PLAN if plan is None else plan))
        (self.dir / 'inventory.json').write_text(json.dumps(inventory_rows() if rows is None else rows))
        out = io.StringIO()
        code = faults.main(['dry-run', '--plan', str(self.dir / 'plan.json'),
                            '--inventory', str(self.dir / 'inventory.json'), '--unit', unit,
                            *extra], transport=lambda *a: (0, '', ''), out=out)
        return code, out.getvalue()

    def refused(self, needle, **kwargs):
        code, text = self.main(**kwargs)
        self.assertEqual(code, 2, text)
        self.assertIn('refused', text)
        self.assertIn(needle, text)

    def test_baseline_accepts(self):
        self.assertEqual(self.main()[0], 0)

    def test_public_hostname(self):
        rows = inventory_rows()
        rows[2]['hostname'] = 'seed2.botho.io'
        self.refused('public hostname', rows=rows)

    def test_public_alias_for_controller_stop(self):
        self.refused('public host alias', extra=('--controller-stop', 'seed.botho.io:/srv/x'))

    def test_unknown_role(self):
        rows = inventory_rows()
        rows.append(dict(rows[1], role='n6', private_ip='10.0.0.16'))
        self.refused('unknown inventory role', rows=rows)

    def test_target_not_in_inventory(self):
        rows = [r for r in inventory_rows() if r['role'] != 'n4']
        self.refused('target role not in inventory: n4', rows=rows)

    def test_producer_target_rechecked(self):
        # The planner already refuses this; the executor checks again.
        with self.assertRaisesRegex(Refusal, 'producer'):
            faults.check_targets([dict(step('D1'), targets=['n3'])], 'n3', INVENTORY)

    def test_missing_opt_in(self):
        plan = json.loads((HERE / 'testnet-discovery-v2.json').read_text())
        self.refused('no degrade_steps', plan=plan)

    def test_empty_schedule_is_treated_as_missing(self):
        plan = copy.deepcopy(PLAN)
        plan.update(execution_mode='rehearsal_only', setup_deadline_hours=1)
        self.refused('no degrade_steps', plan=plan)
        with self.assertRaisesRegex(Refusal, 'no degrade_steps'):
            faults.schedule({'degrade_steps': []}, PLAN)

    def test_malformed_ip(self):
        rows = inventory_rows()
        rows[3]['private_ip'] = '10.0.0'
        self.refused('malformed private_ip', rows=rows)

    def test_public_ip(self):
        rows = inventory_rows()
        rows[3]['private_ip'] = '8.8.8.8'
        self.refused('not RFC1918', rows=rows)
        rows[3]['private_ip'] = '127.0.0.1'
        self.refused('not RFC1918', rows=rows)

    def test_malformed_unit(self):
        self.refused('malformed unit', unit='botho x.service')
        self.refused('malformed unit', unit='botho')

    def test_run_requires_start(self):
        (self.dir / 'plan.json').write_text(json.dumps(PLAN))
        (self.dir / 'inventory.json').write_text(json.dumps(inventory_rows()))
        out = io.StringIO()
        code = faults.main(['run', '--plan', str(self.dir / 'plan.json'), '--inventory',
                            str(self.dir / 'inventory.json'), '--unit', UNIT],
                           transport=lambda *a: (0, '', ''), out=out)
        self.assertEqual(code, 2)
        self.assertIn('--start', out.getvalue())


class ManualStep(unittest.TestCase):
    """`--only <step> --at now` for rehearsal-only fleets (#1610)."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.plan = copy.deepcopy(PLAN)
        self.plan.update(execution_mode='rehearsal_only', setup_deadline_hours=1)

    def tearDown(self):
        self.tmp.cleanup()

    def main(self, mode, *extra, plan=None, transport=None, clock=None, sleep=None):
        (self.dir / 'plan.json').write_text(json.dumps(self.plan if plan is None else plan))
        (self.dir / 'inventory.json').write_text(json.dumps(inventory_rows()))
        out = io.StringIO()
        code = faults.main([mode, '--plan', str(self.dir / 'plan.json'),
                            '--inventory', str(self.dir / 'inventory.json'), '--unit', UNIT,
                            '--journal', str(self.dir / 'faults.jsonl'), *extra],
                           transport=transport or (lambda *a: (0, '', '')), out=out,
                           clock=clock, sleep=sleep)
        return code, out.getvalue()

    def test_manual_step_rebased_to_now(self):
        steps, producer = faults.manual_step(expand(copy.deepcopy(self.plan))[0], self.plan, 'D1')
        self.assertEqual(producer, 'n3')
        self.assertEqual(steps, [dict(step('D1'), fault_start_offset=0, restore_offset=1800,
                                      window_end_offset=3600)])
        (d4,), _ = faults.manual_step(expand(copy.deepcopy(self.plan))[0], self.plan, 'D4')
        self.assertEqual((d4['loss_percent'], d4['delay_ms'], d4['fault_start_offset']), (20, 250, 0))

    def test_dry_run_lists_only_that_step(self):
        code, text = self.main('dry-run', '--only', 'D2', '--at', 'now',
                               clock=lambda: START)
        self.assertEqual(code, 0, text)
        heads = [line for line in text.splitlines() if not line.startswith(' ')]
        self.assertEqual(heads, ['D2 kill_restart n2 fault_start=0 restore=600 window_end=2400'])
        # The fault lands PRECHECK_LEAD after launch.
        self.assertIn(faults.utc(START + faults.PRECHECK_LEAD) + ' apply    n2', text)

    def test_run_applies_restores_and_disarms_now(self):
        host = FakeHost(START)
        code, text = self.main('run', '--only', 'D1', '--at', 'now', transport=host,
                               clock=host.clock, sleep=host.sleep)
        self.assertEqual(code, 0, text)
        lines = [json.loads(l) for l in (self.dir / 'faults.jsonl').read_text().splitlines()]
        actions = [(l['action'], l['ok']) for l in lines if l['role'] == 'n1']
        self.assertIn(('stop', True), actions)
        self.assertIn(('verify', True), actions)
        self.assertEqual(actions[-1], ('disarm', True))
        stop = next(l for l in lines if l['action'] == 'stop')
        self.assertEqual(stop['planned_at'], START + faults.PRECHECK_LEAD)
        self.assertLessEqual(stop['at'] - stop['planned_at'], faults.LATE_GRACE)

    def test_refusals(self):
        for extra, needle in ((('--only', 'D1'), '--at now'),
                              (('--at', 'now'), 'only used with --only'),
                              (('--only', 'D9', '--at', 'now'), "no degrade step 'D9'"),
                              (('--only', 'D1', '--at', 'now', '--start', str(START)), 'do not pass --start')):
            with self.subTest(extra=extra):
                code, text = self.main('dry-run', *extra, clock=lambda: START)
                self.assertEqual(code, 2, text)
                self.assertIn(needle, text)
        # A campaign plan runs its schedule; it never accepts a manual step.
        code, text = self.main('dry-run', '--only', 'D1', '--at', 'now', plan=PLAN, clock=lambda: START)
        self.assertEqual(code, 2)
        self.assertIn('rehearsal-only', text)
        # Without the degrade opt-in there is nothing to pick from.
        bare = json.loads((HERE / 'testnet-discovery-v2.json').read_text())
        bare.update(execution_mode='rehearsal_only', setup_deadline_hours=1)
        code, text = self.main('dry-run', '--only', 'D1', '--at', 'now', plan=bare, clock=lambda: START)
        self.assertEqual(code, 2, text)
        self.assertIn('no degrade block', text)


class FakeHost:
    """Fake clock + transport; records every (alias, argv) call."""

    def __init__(self, now):
        self.now = now
        self.calls = []
        self.minting = {}
        self.fail = lambda alias, argv: False
        self.skew = 0
        self.latency = 0
        self.override = lambda alias, argv: None
        self.units = {}          # alias -> ActiveState of UNIT
        self.stop_lag = 0        # is-active polls that still report `deactivating`
        self.lagged = 0

    def clock(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds

    def __call__(self, alias, argv):
        self.calls.append((alias, list(argv)))
        response = self.override(alias, argv)
        if response is not None:
            return response
        if self.fail(alias, argv):
            return 1, '', 'boom'
        if argv[0] == 'curl':
            return 0, json.dumps({'result': {'mintingActive': self.minting.get(alias, False)}}), ''
        if argv == ['date', '+%s']:
            self.now += self.latency
            return 0, '%d\n' % (self.now + self.skew), ''
        if argv[2:5] == ['systemctl', 'stop', UNIT]:
            self.units[alias], self.lagged = 'inactive', 0
        elif argv[2:5] == ['systemctl', 'kill', '-s']:
            self.units[alias], self.lagged = 'failed', 0  # Restart=no (#1589)
        elif argv[2:5] == ['systemctl', 'start', UNIT]:
            self.units[alias] = 'active'
        if argv[:2] == ['systemctl', 'is-active']:
            state = self.units.get(alias, 'active')
            if state != 'active' and self.lagged < self.stop_lag:
                self.lagged += 1
                state = 'deactivating'
            return (0 if state == 'active' else 3), state + '\n', ''
        if argv[:2] == ['systemctl', 'show']:
            return 0, 'success\n', ''
        if argv[-1] == '-S':
            return 0, '-P INPUT ACCEPT\n-P OUTPUT ACCEPT\n', ''
        if argv[:2] == ['tc', 'qdisc']:
            return 0, 'qdisc mq 0: root\nqdisc fq_codel 0: parent :1 limit 10240p\n', ''
        return 0, '', ''


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.journal = Path(self.tmp.name) / 'faults.jsonl'
        self.out = io.StringIO()

    def tearDown(self):
        self.tmp.cleanup()

    def runner(self, host, steps=None, stop=('controller', '/home/ubuntu/.local/share/botho-x'),
               disk=1 << 40):
        return faults.Runner(steps if steps is not None else [step('D1')], INVENTORY, UNIT, START,
                             host, self.journal, clock=host.clock, sleep=host.sleep,
                             controller_stop=stop, out=self.out,
                             disk_usage=lambda path: type('U', (), {'free': disk})())

    def lines(self):
        return [json.loads(line) for line in self.journal.read_text().splitlines()]

    def test_order_and_journal(self):
        host = FakeHost(START + PHASE - 100)
        self.assertEqual(self.runner(host).run(), 0)
        n1 = [(argv[0] if argv[0] != 'sudo' else argv[2], argv) for alias, argv in host.calls if alias == 'n1']
        kinds = [k for k, _ in n1]
        self.assertEqual(kinds, ['curl', 'curl', 'systemd-run', 'systemctl', 'systemctl',
                                 'systemctl', 'systemctl', 'systemctl', '/bin/sh'])
        self.assertEqual(n1[3][1], ['sudo', '-n', 'systemctl', 'stop', UNIT])
        self.assertEqual(n1[4][1], ['systemctl', 'is-active', UNIT])
        self.assertEqual(n1[6][1], ['sudo', '-n', 'systemctl', 'start', UNIT])
        lines = [l for l in self.lines() if l['phase'] != 'warn']
        # restore -> verify -> disarm: the dead-man outlives every unverified restore.
        self.assertEqual([(l['action'], l['phase']) for l in lines], [
            ('minting_check', 'precheck'), ('minting_check', 'precheck'),
            ('deadman', 'apply'), ('stop', 'apply'), ('stopped', 'apply'), ('result', 'apply'),
            ('restore', 'restore'), ('verify', 'verify'), ('disarm', 'disarm')])
        for line in lines:
            self.assertEqual(line['step'], 'D1')
            self.assertEqual(line['role'], 'n1')
            self.assertIs(line['ok'], True)
            self.assertEqual(line['stderr_tail'], '')
            self.assertTrue({'step', 'role', 'action', 'phase', 'planned_at', 'at', 'ok',
                             'stderr_tail'} <= set(line))
        self.assertEqual(lines[4]['active_state'], 'inactive')
        self.assertEqual(lines[4]['polls'], 1)
        self.assertEqual(lines[5]['unit_result'], 'success')
        # The per-step precheck runs PRECHECK_LEAD before the fault, not at it.
        self.assertEqual(lines[1]['planned_at'], START + PHASE)
        self.assertEqual(lines[1]['at'], START + PHASE - faults.PRECHECK_LEAD)
        self.assertEqual(lines[2]['planned_at'], START + PHASE)
        self.assertEqual(lines[2]['at'], START + PHASE)
        self.assertEqual(lines[6]['planned_at'], START + PHASE + 1800)
        self.assertEqual(lines[6]['at'], START + PHASE + 1800)
        self.assertNotIn(('controller', faults.stop_argv('/home/ubuntu/.local/share/botho-x')), host.calls)

    def test_full_schedule_runs_all_five(self):
        host = FakeHost(START + PHASE - 5)
        self.assertEqual(self.runner(host, steps=copy.deepcopy(STEPS)).run(), 0)
        applied = [(l['step'], l['role']) for l in self.lines() if l['action'] == 'deadman']
        self.assertEqual(applied, [('D1', 'n1'), ('D2', 'n2'), ('D3', 'n4'), ('D4', 'n5'),
                                   ('D5', 'n1'), ('D5', 'n2')])
        self.assertFalse([c for c in host.calls if c[0] == 'n3' and c[1][0] != 'curl'])
        self.assertTrue(all(l['ok'] for l in self.lines() if l['phase'] != 'warn'))

    def test_late_start_skips_and_never_runs_late(self):
        host = FakeHost(START + PHASE + 120)
        code = self.runner(host, steps=[step('D1'), step('D2')]).run()
        self.assertEqual(code, 1)  # D1 not exercised
        lines = [l for l in self.lines() if l['phase'] != 'warn']
        skipped = [l for l in lines if l['action'] == 'skipped_late']
        self.assertEqual([(l['step'], l['ok'], l['planned_at']) for l in skipped],
                         [('D1', False, START + PHASE)])
        self.assertFalse([l for l in lines if l['step'] == 'D1' and l['action'] != 'skipped_late'])
        self.assertIn(('D2', 'deadman'), [(l['step'], l['action']) for l in lines])
        self.assertFalse([c for c in host.calls if c[0] == 'n1'])

    def test_restore_failure_writes_stop_through_transport(self):
        host = FakeHost(START + PHASE - 10)
        host.fail = lambda alias, argv: argv[-1:] == [UNIT] and 'start' in argv
        code = self.runner(host).run()
        self.assertEqual(code, 3)
        self.assertEqual(host.calls[-1], ('controller', ['sudo', '-n', 'touch', '/home/ubuntu/.local/share/botho-x/STOP']))
        restore = [l for l in self.lines() if l['action'] == 'restore'][0]
        self.assertEqual((restore['ok'], restore['stderr_tail']), (False, 'boom'))
        # Verify still ran after the failed restore.
        self.assertIn('verify', [l['action'] for l in self.lines()])
        self.assertIn('RESTORE FAILED', self.out.getvalue())
        # The dead-man stays armed: no disarm was sent or journalled.
        self.assertNotIn('disarm', [l['action'] for l in self.lines()])
        self.assertFalse([c for c in host.calls if 'botho-x-restore-D1-n1.timer' in ' '.join(c[1])])
        self.assertIn('dead-man for n1 is still armed', self.out.getvalue())

    def test_verify_failure_keeps_dead_man_armed(self):
        host = FakeHost(START + PHASE - 100)
        # The restore reports success but the unit never comes back.
        host.override = lambda alias, argv: ((0, 'inactive\n', '')
                                             if argv == ['systemctl', 'is-active', UNIT]
                                             and host.now >= START + PHASE + 1800 else None)
        self.assertEqual(self.runner(host).run(), 3)
        actions = [(l['action'], l['ok']) for l in self.lines()
                   if l['phase'] != 'warn' and l['role'] != 'controller']
        self.assertEqual(actions[-2:], [('restore', True), ('verify', False)])
        self.assertNotIn('disarm', [a for a, _ in actions])
        self.assertFalse([c for c in host.calls if '.timer' in ' '.join(c[1])])

    def test_d5_disarms_only_targets_that_verified(self):
        host = FakeHost(START + PHASE + 13500 - 100)
        host.fail = lambda alias, argv: alias == 'n2' and argv[2:4] == ['systemctl', 'start']
        self.assertEqual(self.runner(host, steps=[step('D5')]).run(), 3)
        phases = [(l['role'], l['action'], l['phase']) for l in self.lines()
                  if l['phase'] != 'warn' and l['role'] != 'controller']
        tail = phases[phases.index(('n1', 'restore', 'restore')):]
        self.assertEqual(tail, [('n1', 'restore', 'restore'), ('n2', 'restore', 'restore'),
                                ('n1', 'verify', 'verify'), ('n2', 'verify', 'verify'),
                                ('n1', 'disarm', 'disarm')])
        self.assertIn('dead-man for n2 is still armed', self.out.getvalue())

    def test_stop_confirmation_polls_until_inactive(self):
        host = FakeHost(START + PHASE - 100)
        host.stop_lag = 3  # SIGTERM drain: three polls still report `deactivating`.
        self.assertEqual(self.runner(host).run(), 0)
        stopped = [l for l in self.lines() if l['action'] == 'stopped']
        self.assertEqual(len(stopped), 1)
        self.assertEqual((stopped[0]['ok'], stopped[0]['active_state'], stopped[0]['polls']),
                         (True, 'inactive', 4))
        self.assertLessEqual(stopped[0]['at'], START + PHASE + 1800 - faults.STOP_MARGIN)

    def test_d2_kill_is_confirmed_stopped_as_failed(self):
        host = FakeHost(START + PHASE + 3600 - 100)
        self.assertEqual(self.runner(host, steps=[step('D2')]).run(), 0)
        stopped = [l for l in self.lines() if l['action'] == 'stopped'][0]
        self.assertEqual((stopped['role'], stopped['ok'], stopped['active_state']), ('n2', True, 'failed'))

    def test_unconfirmed_stop_restores_before_restore_at(self):
        host = FakeHost(START + PHASE - 100)
        host.stop_lag = 10 ** 6  # Never finishes stopping.
        self.assertEqual(self.runner(host).run(), 1)
        lines = [l for l in self.lines() if l['phase'] != 'warn']
        stopped = [l for l in lines if l['action'] == 'stopped'][0]
        self.assertEqual((stopped['ok'], stopped['active_state']), (False, 'deactivating'))
        deadline = START + PHASE + 1800 - faults.STOP_MARGIN
        self.assertLessEqual(stopped['at'], deadline)
        restore = [l for l in lines if l['action'] == 'restore'][0]
        self.assertLess(restore['at'], START + PHASE + 1800)
        self.assertNotIn('result', [l['action'] for l in lines])

    def test_verify_failure_without_controller_stop_prints_manual_restore(self):
        host = FakeHost(START + PHASE + 6000 - 10)
        host.override = lambda alias, argv: (0, '-N botho-x-D3\n', '') if argv[-1] == '-S' else None
        self.assertEqual(self.runner(host, steps=[step('D3')], stop=None).run(), 3)
        verify = [l for l in self.lines() if l['action'] == 'verify'][0]
        self.assertEqual((verify['ok'], verify['stderr_tail']), (False, '-N botho-x-D3\n'))
        text = self.out.getvalue()
        self.assertIn('ssh n4 -- sudo -n /bin/sh -c', text)
        self.assertFalse([c for c in host.calls if c[1][-1].endswith('/STOP')])

    def test_apply_failure_is_not_retried_and_restores_at_once(self):
        host = FakeHost(START + PHASE - 10)
        host.fail = lambda alias, argv: argv[:5] == ['sudo', '-n', 'systemctl', 'stop', UNIT]
        self.assertEqual(self.runner(host).run(), 1)
        stops = [c for c in host.calls if c[1][:4] == ['sudo', '-n', 'systemctl', 'stop']]
        self.assertEqual(len(stops), 1)
        restore = [l for l in self.lines() if l['action'] == 'restore'][0]
        self.assertLess(restore['at'], START + PHASE + 1800)

    def test_laptop_warnings_never_stop_the_schedule(self):
        host = FakeHost(START + PHASE - 10)
        host.skew, host.latency = 30, 9
        self.assertEqual(self.runner(host, disk=1 << 20).run(), 0)
        warns = {l['action'] for l in self.lines() if l['phase'] == 'warn'}
        self.assertEqual(warns, {'laptop_disk', 'clock_skew', 'ssh_latency'})
        self.assertIn('verify', [l['action'] for l in self.lines()])

    def test_unreachable_controller_is_only_a_warning(self):
        host = FakeHost(START + PHASE - 10)
        host.fail = lambda alias, argv: alias == 'controller'
        self.assertEqual(self.runner(host).run(), 0)
        self.assertIn('clock_skew', [l['action'] for l in self.lines() if l['phase'] == 'warn'])

    def test_refuses_minting_host_regardless_of_plan(self):
        # The plan says the producer is n3, but n1 reports mintingActive.
        host = FakeHost(START + PHASE - 10)
        host.minting['n1'] = True
        self.assertEqual(self.runner(host).run(), 2)
        self.assertEqual({argv[0] for alias, argv in host.calls if alias == 'n1'}, {'curl'})
        self.assertIn('REFUSED', self.out.getvalue())

    def test_refuses_mid_run_when_target_starts_minting(self):
        host = FakeHost(START + PHASE - 10)
        checks = []

        def flip(alias, argv):
            if argv[0] == 'curl' and alias == 'n2':
                checks.append(alias)
                host.minting['n2'] = len(checks) > 1
        host.override = flip
        runner = self.runner(host, steps=[step('D1'), step('D2')])
        self.assertEqual(runner.run(), 2)
        self.assertFalse([c for c in host.calls if c[0] == 'n2' and c[1][0] != 'curl'])
        self.assertIn(('D1', 'verify'), [(l['step'], l['action']) for l in self.lines()])

    def test_unreadable_status_fails_closed(self):
        host = FakeHost(START + PHASE - 10)
        host.fail = lambda alias, argv: argv[0] == 'curl'
        self.assertEqual(self.runner(host).run(), 2)
        self.assertFalse([c for c in host.calls if c[1][:3] == ['sudo', '-n', 'systemd-run']])


class DryRun(unittest.TestCase):
    def run_dry(self, *extra):
        out = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp:
            inventory = Path(tmp) / 'inventory.json'
            inventory.write_text(json.dumps(inventory_rows()))
            code = faults.main(['dry-run', '--plan', str(HERE / 'x-series-plan.json'),
                                '--inventory', str(inventory), '--unit', UNIT, *extra], out=out)
        self.assertEqual(code, 0)
        return out.getvalue()

    def test_lists_d1_to_d5_at_planned_offsets(self):
        text = self.run_dry()
        heads = [line for line in text.splitlines() if not line.startswith(' ')]
        expected = [('D1', 'stop_restart', 'n1', 0, 1800, 3600),
                    ('D2', 'kill_restart', 'n2', 3600, 4200, 6000),
                    ('D3', 'isolate', 'n4', 6000, 7800, 9600),
                    ('D4', 'netem', 'n5', 9600, 11400, 13200),
                    ('D5', 'stop_restart_pair', 'n1,n2', 13500, 14100, 15900)]
        self.assertEqual(heads, [
            '%s %s %s fault_start=%d restore=%d window_end=%d%s' % (
                i, a, t, PHASE + f, PHASE + r, PHASE + w,
                ' loss=20% delay=250ms' if a == 'netem' else '')
            for i, a, t, f, r, w in expected])
        self.assertIn('T0+172800s apply    n1   sudo -n systemd-run', text)

    def test_absolute_utc_times_with_start(self):
        text = self.run_dry('--start', str(START), '--controller-stop', 'controller:/srv/state')
        # START + 48h = 2027-01-17T08:00:00Z
        self.assertIn('2027-01-17T08:00:00Z apply    n1   ' + shlex.join(
            commands(step('D1'), INVENTORY, UNIT)['apply'][0]['argv']), text)
        self.assertIn('controller sudo -n touch /srv/state/STOP', text)


if __name__ == '__main__':
    unittest.main()
