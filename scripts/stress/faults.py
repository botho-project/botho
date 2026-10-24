#!/usr/bin/env python3
"""Tracked fault executor for the x-series degrade phase (D1-D5, #1588).

Applies the `degrade_steps` schedule from `plan.expand()` (#1585) to the
isolated qualification fleet over SSH, restores each fault, verifies the
restore and journals every action to a local `faults.jsonl`. Stdlib only.

Safety properties:

- Rendering is pure: `commands()` returns argv lists built only from validated
  tokens (role names, step ids, the unit name, `ipaddress`-checked private IPs
  and integers). Inventory text never reaches a shell unvalidated.
- Before every fault an on-host transient systemd timer (the dead-man restore)
  is armed, so the node is restored even if the laptop, its SSH session or this
  runner dies. The normal restore disarms it first. Every restore is
  idempotent; the verify commands are the authoritative check.
- A host whose RPC reports `mintingActive` (or whose status cannot be read) is
  never faulted, whatever the plan's `producer_role` says.
- Laptop-side conditions (disk, clock skew, SSH latency) are journalled as
  `warn` and never stop the schedule. A failed restore or verify writes `STOP`
  on the controller host and exits non-zero.
"""
import argparse
import datetime
import ipaddress
import json
import math
import re
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path

import observer
from runtime import HOSTS

ROLES = ('controller', 'n1', 'n2', 'n3', 'n4', 'n5')
NODE_ROLES = ROLES[1:]
GOSSIP_PORT = 17110
DEADMAN_GRACE = 120        # Dead-man fires this long after the planned restore.
LATE_GRACE = 30            # A step starting later than this after fault_start is skipped.
DEFAULT_DEV = 'ens5'
DEFAULT_STATUS_URL = 'http://127.0.0.1:17101/rpc'
STATUS_BODY = '{"jsonrpc":"2.0","id":1588,"method":"node_getStatus","params":{}}'
RFC1918 = tuple(ipaddress.ip_network(n) for n in ('10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16'))
PUBLIC_NAMES = frozenset(h.lower() for h in HOSTS)
ALIAS = re.compile(r'[a-z][a-z0-9-]{0,62}')
DEV = re.compile(r'[a-z][a-z0-9]{0,14}')
STATUS_URL = re.compile(r'https?://(?:127\.0\.0\.1|localhost):[1-9][0-9]{0,4}/rpc')
WARN_DISK_BYTES = 1 << 30
WARN_SKEW_SECONDS = 2
WARN_LATENCY_SECONDS = 5
SSH_TIMEOUT = 180
RESTART_ACTIONS = ('stop_restart', 'kill_restart', 'stop_restart_pair')
TAIL = 400


class Refusal(Exception):
    """The executor will not run against this plan, inventory or host."""


def _need(condition, message):
    if not condition:
        raise Refusal(message)


# -- input validation ----------------------------------------------------------

def private_ip(value):
    try:
        address = ipaddress.ip_address(value)
    except (TypeError, ValueError):
        raise Refusal('malformed private_ip: %r' % (value,)) from None
    _need(address.version == 4 and any(address in net for net in RFC1918),
          'private_ip is not RFC1918: %s' % address)
    return str(address)


def validate_unit(unit):
    try:
        return observer.node_config(dict(observer.DEFAULT_NODE, node_unit=unit))['node_unit']
    except Exception:
        raise Refusal('malformed unit name: %r' % (unit,)) from None


def validate_alias(alias):
    _need(isinstance(alias, str), 'malformed SSH alias: %r' % (alias,))
    _need(alias.lower() not in PUBLIC_NAMES, 'refusing public host alias: ' + alias)
    _need(ALIAS.fullmatch(alias), 'malformed SSH alias: %r' % (alias,))
    return alias


def load_inventory(rows):
    """`connect-fleet.py` rows -> {role: row}; the role name is the SSH alias."""
    _need(isinstance(rows, list) and rows, 'inventory must be a non-empty list')
    inventory = {}
    for row in rows:
        _need(isinstance(row, dict) and {'role', 'hostname', 'private_ip'} <= set(row),
              'inventory rows need role, hostname and private_ip')
        role = row['role']
        _need(role in ROLES, 'unknown inventory role: %r' % (role,))
        _need(role not in inventory, 'duplicate inventory role: ' + role)
        validate_alias(role)
        hostname = row['hostname']
        _need(isinstance(hostname, str) and hostname, 'inventory hostname required')
        _need(hostname.lower().rstrip('.') not in PUBLIC_NAMES,
              'refusing public hostname: ' + hostname)
        inventory[role] = dict(row, private_ip=private_ip(row['private_ip']))
    ips = [r['private_ip'] for r in inventory.values()]
    _need(len(set(ips)) == len(ips), 'inventory private IPs must be distinct')
    return inventory


def schedule(summary, plan):
    """The validated degrade schedule; an empty or absent one is refused."""
    steps = summary.get('degrade_steps')
    # `degrade_steps: []` (rehearsal_only) is treated exactly like a missing
    # opt-in: there is nothing to execute and an empty run must never start.
    _need(isinstance(steps, list) and steps,
          'plan has no degrade_steps (validator_degrade_phase opt-in missing or rehearsal-only)')
    producer = plan.get('degrade', {}).get('producer_role')
    _need(producer in NODE_ROLES, 'plan names no producer role')
    return steps, producer


def check_targets(steps, producer, inventory):
    for step in steps:
        for role in step['targets']:
            _need(role in inventory, 'target role not in inventory: %s (%s)' % (role, step['id']))
            _need(role != producer, 'refusing to target the producer %s (%s)' % (role, step['id']))
    missing = [r for r in NODE_ROLES if r not in inventory]
    _need(not missing, 'inventory lacks node roles %s; isolate needs every peer' % missing)


def parse_controller_stop(value):
    alias, sep, path = (value or '').partition(':')
    _need(sep, '--controller-stop must be <alias>:<absolute path>')
    validate_alias(alias)
    try:
        observer.absolute_path(path)
    except Exception:
        raise Refusal('--controller-stop path must be a normalized absolute path') from None
    return alias, path


# -- pure rendering ------------------------------------------------------------

def timer_unit(step_id, role):
    return 'botho-x-restore-%s-%s' % (step_id, role)


def chain(step_id):
    return 'botho-x-' + step_id


def _cmd(role, action, argv):
    return {'role': role, 'action': action, 'argv': list(argv)}


def _sh(*commands):
    """One `/bin/sh -c` script from argv lists of validated tokens."""
    return ['/bin/sh', '-c', '; '.join(c if isinstance(c, str) else shlex.join(c) for c in commands)]


def restore_argv(step, role, unit, dev=DEFAULT_DEV):
    """The idempotent on-host restore, shared by the normal path and the dead-man."""
    action = step['action']
    if action in RESTART_ACTIONS:
        return ['systemctl', 'start', unit]  # Starting an active unit succeeds.
    if action == 'isolate':
        name = chain(step['id'])
        unhook = ['while %s 2>/dev/null; do :; done' % shlex.join(
            ['iptables', '-w', '-D', hook, '-j', name]) for hook in ('INPUT', 'OUTPUT')]
        return _sh(*unhook,
                   shlex.join(['iptables', '-w', '-F', name]) + ' 2>/dev/null',
                   shlex.join(['iptables', '-w', '-X', name]) + ' 2>/dev/null',
                   '! ' + shlex.join(['iptables', '-w', '-n', '-L', name]) + ' >/dev/null 2>&1')
    if action == 'netem':
        return _sh(shlex.join(['tc', 'qdisc', 'del', 'dev', dev, 'root']) + ' 2>/dev/null',
                   '! ' + shlex.join(['tc', 'qdisc', 'show', 'dev', dev])
                   + " | grep -Eq 'qdisc (prio|netem) '")
    raise Refusal('unknown action: %r' % (action,))


def deadman_argv(step, role, unit, on_active, dev=DEFAULT_DEV):
    return (['sudo', '-n', 'systemd-run', '--collect', '--on-active=%d' % on_active,
             '--timer-property=AccuracySec=1s', '--unit=' + timer_unit(step['id'], role)]
            + restore_argv(step, role, unit, dev))


def disarm_argv(step, role):
    timer = timer_unit(step['id'], role) + '.timer'
    # Succeeds when the timer is stopped now or has already fired.
    return ['sudo', '-n'] + _sh(shlex.join(['systemctl', 'stop', timer]) + ' 2>/dev/null'
                                + ' || ! ' + shlex.join(['systemctl', 'is-active', '--quiet', timer]))


def commands(step, inventory, unit, dev=DEFAULT_DEV, now_offset=None):
    """Render one step: {apply, restore, verify} lists of {role, action, argv}.

    `apply` starts with one dead-man `systemd-run` per target, before any fault
    command. `now_offset` (seconds after campaign start) sizes the dead-man so
    it fires DEADMAN_GRACE after the planned restore; it defaults to the
    planned fault start. Restart steps also carry `result` commands recording
    the unit's `Result` property after the fault.
    """
    unit = validate_unit(unit)
    _need(DEV.fullmatch(dev or ''), 'malformed interface name: %r' % (dev,))
    action, step_id, targets = step['action'], step['id'], list(step['targets'])
    _need(re.fullmatch(r'[A-Za-z0-9_-]{1,16}', step_id or ''), 'malformed step id')
    for role in targets:
        _need(role in NODE_ROLES and role in inventory, 'target role not in inventory: %r' % (role,))
    now_offset = step['fault_start_offset'] if now_offset is None else now_offset
    on_active = max(1, math.ceil(step['restore_offset'] - now_offset + DEADMAN_GRACE))
    apply = [_cmd(r, 'deadman', deadman_argv(step, r, unit, on_active, dev)) for r in targets]
    restore = []
    verify = []
    result = []
    for role in targets:
        restore.append(_cmd(role, 'disarm', disarm_argv(step, role)))
    if action in ('stop_restart', 'stop_restart_pair'):
        if len(targets) > 1:
            # Queue both stops first so SIGTERM reaches the pair together, then
            # wait on each (a second stop joins the queued job).
            apply += [_cmd(r, 'stop_queue', ['sudo', '-n', 'systemctl', 'stop', '--no-block', unit])
                      for r in targets]
        apply += [_cmd(r, 'stop', ['sudo', '-n', 'systemctl', 'stop', unit]) for r in targets]
    elif action == 'kill_restart':
        apply += [_cmd(r, 'kill', ['sudo', '-n', 'systemctl', 'kill', '-s', 'SIGKILL', unit])
                  for r in targets]
    elif action == 'isolate':
        (role,) = targets
        name = chain(step_id)
        peers = [inventory[r]['private_ip'] for r in NODE_ROLES if r != role]
        ipt = ['sudo', '-n', 'iptables', '-w']
        apply.append(_cmd(role, 'isolate', ipt + ['-N', name]))
        for peer in peers:
            for proto in ('tcp', 'udp'):
                for direction in ('-s', '-d'):
                    apply.append(_cmd(role, 'isolate', ipt + [
                        '-A', name, direction, private_ip(peer), '-p', proto,
                        '-m', 'multiport', '--ports', str(GOSSIP_PORT), '-j', 'DROP']))
        # Hook last, so the partition starts at once.
        apply += [_cmd(role, 'isolate', ipt + ['-I', hook, '1', '-j', name])
                  for hook in ('INPUT', 'OUTPUT')]
    elif action == 'netem':
        (role,) = targets
        loss, delay = step['loss_percent'], step['delay_ms']
        _need(type(loss) is int and type(delay) is int, 'netem parameters must be integers')
        tc = ['sudo', '-n', 'tc']
        # A 4-band prio root: the default priomap only uses bands 1-3, so only
        # the gossip-port filters reach netem on band 4. SSH and 443 are untouched.
        apply.append(_cmd(role, 'netem', tc + ['qdisc', 'add', 'dev', dev, 'root', 'handle', '1:',
                                               'prio', 'bands', '4']))
        apply.append(_cmd(role, 'netem', tc + ['qdisc', 'add', 'dev', dev, 'parent', '1:4',
                                               'handle', '40:', 'netem', 'loss', '%d%%' % loss,
                                               'delay', '%dms' % delay]))
        for match in ('dport', 'sport'):
            apply.append(_cmd(role, 'netem', tc + [
                'filter', 'add', 'dev', dev, 'parent', '1:0', 'protocol', 'ip', 'prio', '1',
                'u32', 'match', 'ip', match, str(GOSSIP_PORT), '0xffff', 'flowid', '1:4']))
    else:
        raise Refusal('unknown action: %r' % (action,))
    for role in targets:
        restore.append(_cmd(role, 'restore', ['sudo', '-n'] + restore_argv(step, role, unit, dev)))
        if action in RESTART_ACTIONS:
            result.append(_cmd(role, 'result', ['systemctl', 'show', unit, '--property=Result', '--value']))
            verify.append(dict(_cmd(role, 'verify', ['systemctl', 'is-active', unit]),
                               expect={'stdout': 'active'}))
        elif action == 'isolate':
            verify.append(dict(_cmd(role, 'verify', ['sudo', '-n', 'iptables', '-w', '-S']),
                               expect={'absent': ['botho-x']}))
        else:
            verify.append(dict(_cmd(role, 'verify', ['tc', 'qdisc', 'show', 'dev', dev]),
                               expect={'absent': ['qdisc prio ', 'qdisc netem ']}))
    rendered = {'apply': apply, 'restore': restore, 'verify': verify}
    if result:
        rendered['result'] = result
    return rendered


def status_argv(url=DEFAULT_STATUS_URL):
    return ['curl', '-sS', '--max-time', '5', '-H', 'Content-Type: application/json',
            '--data-binary', STATUS_BODY, url]


def stop_argv(path):
    return ['sudo', '-n', 'touch', path.rstrip('/') + '/STOP']


def verified(entry, rc, stdout):
    if rc != 0 and 'stdout' in entry['expect']:
        return False
    if 'stdout' in entry['expect']:
        return stdout.strip() == entry['expect']['stdout']
    return rc == 0 and not any(token in stdout for token in entry['expect']['absent'])


# -- transport -----------------------------------------------------------------

def ssh_transport(config, timeout=SSH_TIMEOUT):
    """Production transport: `ssh -F <config> <alias> -- <argv>`.

    The remote side runs the command through a shell, so the argv is joined
    with `shlex.join` and parses back to exactly the rendered list.
    """
    def call(alias, argv):
        validate_alias(alias)
        try:
            done = subprocess.run(
                ['ssh', '-F', config, '-o', 'BatchMode=yes', alias, '--', shlex.join(argv)],
                capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return 255, '', 'ssh timeout after %ds' % timeout
        return done.returncode, done.stdout, done.stderr
    return call


# -- runner --------------------------------------------------------------------

class Runner:
    def __init__(self, steps, inventory, unit, start, transport, journal_path,
                 clock=time.time, sleep=time.sleep, controller_stop=None,
                 dev=DEFAULT_DEV, status_url=DEFAULT_STATUS_URL,
                 disk_usage=shutil.disk_usage, out=sys.stdout):
        self.steps, self.inventory, self.unit = steps, inventory, validate_unit(unit)
        _need(type(start) is int and start > 0, '--start must be positive UNIX seconds')
        _need(STATUS_URL.fullmatch(status_url), 'status URL must be loopback /rpc')
        self.start, self.transport, self.clock, self.sleep = start, transport, clock, sleep
        self.journal_path = Path(journal_path)
        self.controller_stop, self.dev, self.status_url = controller_stop, dev, status_url
        self.disk_usage, self.out = disk_usage, out
        self.incomplete = False

    # journal
    def record(self, step, role, action, phase, planned_at, ok, stderr='', **extra):
        line = {'step': step, 'role': role, 'action': action, 'phase': phase,
                'planned_at': planned_at, 'at': self.clock(), 'ok': ok,
                'stderr_tail': (stderr or '')[-TAIL:]}
        line.update(extra)
        with self.journal_path.open('a') as output:
            output.write(json.dumps(line, sort_keys=True) + '\n')
            output.flush()
        return line

    def wait_until(self, at):
        while True:
            remaining = at - self.clock()
            if remaining <= 0:
                return
            self.sleep(min(remaining, 60))

    def call(self, alias, argv):
        try:
            return self.transport(alias, argv)
        except Exception as error:  # A transport failure is a failed command.
            return 255, '', '%s: %s' % (type(error).__name__, error)

    # laptop-side checks: warn, never stop
    def laptop_checks(self, step_id):
        def warn(action, detail):
            self.record(step_id, 'laptop', action, 'warn', None, False, detail=detail)
        try:
            free = self.disk_usage(str(self.journal_path.parent)).free
            if free < WARN_DISK_BYTES:
                warn('laptop_disk', 'free bytes %d' % free)
        except Exception as error:
            warn('laptop_disk', repr(error))
        if 'controller' not in self.inventory:
            warn('clock_skew', 'no controller row in inventory')
            return
        sent = self.clock()
        rc, stdout, stderr = self.call('controller', ['date', '+%s'])
        received = self.clock()
        if received - sent > WARN_LATENCY_SECONDS:
            warn('ssh_latency', 'controller round trip %.1fs' % (received - sent))
        try:
            skew = int(stdout.strip()) - (sent + received) / 2 if rc == 0 else None
        except ValueError:
            skew = None
        if skew is None:
            warn('clock_skew', 'controller clock unreadable: rc %s %s' % (rc, stderr[-TAIL:]))
        elif abs(skew) > WARN_SKEW_SECONDS:
            warn('clock_skew', 'controller minus laptop %.1fs' % skew)

    # producer guard
    def minting_refusals(self, step):
        """Roles that must not be faulted: minting, or status unreadable (fail closed)."""
        refused = []
        planned = self.start + step['fault_start_offset']
        for role in step['targets']:
            rc, stdout, stderr = self.call(role, status_argv(self.status_url))
            minting = None
            try:
                minting = json.loads(stdout)['result']['mintingActive'] if rc == 0 else None
            except (ValueError, KeyError, TypeError):
                pass
            ok = minting is False
            self.record(step['id'], role, 'minting_check', 'precheck', planned, ok,
                        stderr, minting_active=minting)
            if not ok:
                refused.append(role)
        return refused

    def execute(self, entries, step, phase, planned, stop_on_failure):
        ok_all = True
        for entry in entries:
            rc, stdout, stderr = self.call(entry['role'], entry['argv'])
            ok = rc == 0
            extra = {}
            if entry['action'] == 'result':
                extra['unit_result'] = stdout.strip()
            if 'expect' in entry:
                ok = verified(entry, rc, stdout)
                if not ok and not stderr:
                    stderr = stdout
            self.record(step['id'], entry['role'], entry['action'], phase, planned, ok,
                        stderr, **extra)
            ok_all = ok_all and ok
            if not ok and stop_on_failure:
                return False
        return ok_all

    def fail_closed(self, step, rendered):
        if self.controller_stop:
            alias, path = self.controller_stop
            rc, _, stderr = self.call(alias, stop_argv(path))
            self.record(step['id'], alias, 'controller_stop', 'restore', None, rc == 0, stderr)
        print('RESTORE FAILED for step %s. Run on each target, then re-verify:' % step['id'],
              file=self.out)
        for entry in rendered['restore'] + rendered['verify']:
            print('  ssh %s -- %s' % (entry['role'], shlex.join(entry['argv'])), file=self.out)
        return 3

    def restore_step(self, step, rendered):
        planned = self.start + step['restore_offset']
        ok = self.execute(rendered['restore'], step, 'restore', planned, stop_on_failure=False)
        ok = self.execute(rendered['verify'], step, 'verify', planned, stop_on_failure=False) and ok
        return ok

    def run_step(self, step):
        fault_at = self.start + step['fault_start_offset']
        restore_at = self.start + step['restore_offset']
        if self.clock() > fault_at + LATE_GRACE:
            self.record(step['id'], ','.join(step['targets']), 'skipped_late', 'apply', fault_at, False)
            self.incomplete = True
            return 0
        self.wait_until(fault_at)
        self.laptop_checks(step['id'])
        refused = self.minting_refusals(step)
        if refused:
            print('REFUSED step %s: %s reports mintingActive (or no readable status)'
                  % (step['id'], ','.join(refused)), file=self.out)
            return 2
        if self.clock() > fault_at + LATE_GRACE:
            self.record(step['id'], ','.join(step['targets']), 'skipped_late', 'apply', fault_at, False)
            self.incomplete = True
            return 0
        rendered = commands(step, self.inventory, self.unit, self.dev,
                            now_offset=self.clock() - self.start)
        applied = self.execute(rendered['apply'], step, 'apply', fault_at, stop_on_failure=True)
        if applied:
            self.execute(rendered.get('result', []), step, 'apply', fault_at, stop_on_failure=False)
            try:
                self.wait_until(restore_at)
            except KeyboardInterrupt:
                print('interrupted: restoring step %s now' % step['id'], file=self.out)
                if not self.restore_step(step, rendered):
                    return self.fail_closed(step, rendered)
                return 130
        else:
            # Never retried; undo whatever part landed straight away.
            self.incomplete = True
        if not self.restore_step(step, rendered):
            return self.fail_closed(step, rendered)
        return 0

    def run(self):
        self.laptop_checks('preflight')
        for step in self.steps:
            if self.clock() > self.start + step['fault_start_offset'] + LATE_GRACE:
                continue  # Recorded as skipped_late in run_step.
            refused = self.minting_refusals(step)
            if refused:
                print('REFUSED: %s reports mintingActive (or no readable status); no fault applied'
                      % ','.join(refused), file=self.out)
                return 2
        for step in self.steps:
            code = self.run_step(step)
            if code:
                return code
        return 1 if self.incomplete else 0


# -- CLI -----------------------------------------------------------------------

def utc(at):
    return datetime.datetime.fromtimestamp(at, datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')


def dry_run(steps, inventory, unit, start=None, dev=DEFAULT_DEV, controller_stop=None,
            status_url=DEFAULT_STATUS_URL, out=sys.stdout):
    def when(offset):
        return utc(start + offset) if start is not None else 'T0+%ds' % offset
    for step in steps:
        rendered = commands(step, inventory, unit, dev)
        extra = ' loss=%d%% delay=%dms' % (step['loss_percent'], step['delay_ms']) \
            if step['action'] == 'netem' else ''
        print('%s %s %s fault_start=%d restore=%d window_end=%d%s' % (
            step['id'], step['action'], ','.join(step['targets']), step['fault_start_offset'],
            step['restore_offset'], step['window_end_offset'], extra), file=out)
        groups = [('precheck', step['fault_start_offset'],
                   [_cmd(r, 'minting_check', status_argv(status_url)) for r in step['targets']]),
                  ('apply', step['fault_start_offset'], rendered['apply'] + rendered.get('result', [])),
                  ('restore', step['restore_offset'], rendered['restore']),
                  ('verify', step['restore_offset'], rendered['verify'])]
        for phase, offset, entries in groups:
            for entry in entries:
                print('  %s %-8s %-4s %s' % (when(offset), phase, entry['role'],
                                             shlex.join(entry['argv'])), file=out)
    if controller_stop:
        alias, path = controller_stop
        print('on restore/verify failure: %s %s' % (alias, shlex.join(stop_argv(path))), file=out)


def prepare(args):
    from plan import expand
    plan = json.loads(Path(args.plan).read_text())
    summary, _ = expand(plan)
    steps, producer = schedule(summary, plan)
    inventory = load_inventory(json.loads(Path(args.inventory).read_text()))
    check_targets(steps, producer, inventory)
    validate_unit(args.unit)
    _need(DEV.fullmatch(args.dev), 'malformed interface name')
    stop = parse_controller_stop(args.controller_stop) if args.controller_stop else None
    return steps, inventory, stop


def main(argv=None, transport=None, out=sys.stdout):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('mode', choices=['run', 'dry-run'])
    parser.add_argument('--plan', required=True)
    parser.add_argument('--inventory', required=True)
    parser.add_argument('--ssh-config')
    parser.add_argument('--unit', required=True)
    parser.add_argument('--start', type=int, help='campaign start (UNIX s) from activated.json')
    parser.add_argument('--controller-stop', help='<alias>:<controller state dir>')
    parser.add_argument('--dev', default=DEFAULT_DEV, help='egress interface for netem')
    parser.add_argument('--status-url', default=DEFAULT_STATUS_URL)
    parser.add_argument('--journal', default='faults.jsonl')
    args = parser.parse_args(argv)
    try:
        steps, inventory, stop = prepare(args)
        if args.mode == 'dry-run':
            dry_run(steps, inventory, args.unit, args.start, args.dev, stop, args.status_url, out)
            return 0
        _need(args.start is not None, 'run requires --start from activated.json')
        if transport is None:
            _need(args.ssh_config and Path(args.ssh_config).is_file(), 'run requires --ssh-config')
            transport = ssh_transport(args.ssh_config)
        runner = Runner(steps, inventory, args.unit, args.start, transport, args.journal,
                        controller_stop=stop, dev=args.dev, status_url=args.status_url, out=out)
        return runner.run()
    except (Refusal, ValueError) as error:
        print('refused: %s' % error, file=out)
        return 2


if __name__ == '__main__':
    sys.exit(main())
