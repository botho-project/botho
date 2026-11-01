import base64
import datetime
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

import fleet_provision as fp
import host_maintenance

RUN = 'q20261101-x1-endurance'
CREATED = 1793491200  # 2026-11-01T00:00:00Z
HOST_END = CREATED + 82 * 3600
SG = 'sg-0123456789abcdef0'
PLAN_ARGS = dict(region='us-west-2', subnet='subnet-08f2db3700f3c18a5', vpc='vpc-0d71f2eb3650855c4',
                 image='ami-07a134137a631b892', cidrs=['104.7.12.215/32', '52.119.115.124/32'],
                 scope='x1 endurance: 72h with degrade phase')


def as_dict(pairs):
    return {p['Key']: p['Value'] for p in pairs}


class RunIdTests(unittest.TestCase):
    def test_builds_x_series_id(self):
        self.assertEqual(fp.run_id('20261101', 1, 'endurance'), RUN)
        self.assertEqual(fp.run_id(datetime.date(2026, 11, 1), 1, 'endurance'), RUN)
        self.assertEqual(fp.validate_run_id(RUN), RUN)
        self.assertEqual(fp.run_id('20261101', 12, 'degrade-soak'), 'q20261101-x12-degrade-soak')

    def test_rejects_w_series_token(self):
        for args in (('20261101', 1, 'w12'), ('20261101', 1, 'w12-endurance'), ('20261101', 1, 'endurance-w12')):
            with self.subTest(args=args), self.assertRaisesRegex(ValueError, 'w-series'):
                fp.run_id(*args)
        for value in ('q20261022-w12-endurance', 'q20261101-x1-w12', 'q20261101-x1-endurance-w3'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                fp.validate_run_id(value)
        # Labels that merely contain a w are fine.
        self.assertEqual(fp.run_id('20261101', 1, 'wide-w'), 'q20261101-x1-wide-w')

    def test_rejects_bad_date(self):
        for date in ('20261301', '20260230', '2026111', '2026-11-01', 20261101):
            with self.subTest(date=date), self.assertRaises(ValueError):
                fp.run_id(date, 1, 'endurance')
        with self.assertRaisesRegex(ValueError, 'calendar'):
            fp.validate_run_id('q20261332-x1-endurance')

    def test_rejects_uppercase_and_malformed(self):
        for args in (('20261101', 1, 'Endurance'), ('20261101', 0, 'endurance'), ('20261101', True, 'endurance'),
                     ('20261101', 1, '1endurance'), ('20261101', 1, 'endurance-'), ('20261101', 1, 'a--b'),
                     ('20261101', 1, 'a' * 40)):
            with self.subTest(args=args), self.assertRaises(ValueError):
                fp.run_id(*args)
        for value in ('Q20261101-x1-endurance', 'q20261101-X1-endurance', 'q20261101-x1-Endurance',
                      'q20261101-x01-endurance', 'q20261101-endurance', RUN + '\n'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                fp.validate_run_id(value)


class TagTests(unittest.TestCase):
    def test_golden_tags(self):
        pairs = fp.tags(RUN, 'n2', HOST_END)
        self.assertEqual(pairs, [
            {'Key': 'Name', 'Value': 'botho-q20261101-x1-endurance-n2'},
            {'Key': 'project', 'Value': 'botho'},
            {'Key': 'Purpose', 'Value': 'qualification'},
            {'Key': 'SideProject', 'Value': 'botho'},
            {'Key': 'Run', 'Value': RUN},
            {'Key': 'Role', 'Value': 'n2'},
            {'Key': 'ExpiresAt', 'Value': str(HOST_END)},
        ])
        keys = [p['Key'] for p in pairs]
        self.assertEqual(set(keys), {'Name', 'project', 'Purpose', 'SideProject', 'Run', 'Role', 'ExpiresAt'})
        self.assertEqual(as_dict(pairs)['SideProject'], 'botho')
        self.assertEqual([k for k in keys if k.lower() == 'project'], ['project'])

    def test_every_role_never_emits_capital_project(self):
        for role in fp.ROLES:
            for spec in fp.tag_specifications(RUN, role, HOST_END):
                keys = [p['Key'] for p in spec['Tags']]
                self.assertEqual([k for k in keys if k.lower() == 'project'], ['project'])
                self.assertEqual(len({k.lower() for k in keys}), len(keys))

    def test_instance_and_volume_tags_identical(self):
        specs = fp.tag_specifications(RUN, 'controller', HOST_END)
        self.assertEqual([s['ResourceType'] for s in specs], ['instance', 'volume'])
        self.assertEqual(specs[0]['Tags'], specs[1]['Tags'])

    def test_guard_refuses_case_insensitive_project(self):
        for key in ('Project', 'PROJECT', 'pRoject'):
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'case-insensitively'):
                fp._pairs([('project', 'botho'), (key, 'botho-qualification')])
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            fp._pairs([('Run', 'a'), ('run', 'b')])

    def test_tag_inputs_validated(self):
        for args in ((RUN, 'n6', HOST_END), (RUN, 'Controller', HOST_END), (RUN, 'n1', str(HOST_END)),
                     (RUN, 'n1', 0), ('q20261022-w12-endurance', 'n1', HOST_END)):
            with self.subTest(args=args), self.assertRaises(ValueError):
                fp.tags(*args)

    def test_security_group_tags(self):
        self.assertEqual(as_dict(fp.security_group_tags(RUN)),
                         {'project': 'botho', 'Purpose': 'qualification', 'SideProject': 'botho', 'Run': RUN})
        self.assertEqual(fp.security_group_specifications(RUN)[0]['ResourceType'], 'security-group')
        self.assertEqual(fp.security_group_name(RUN), 'botho-' + RUN)


class SecurityGroupRuleTests(unittest.TestCase):
    def test_golden_rules(self):
        self.assertEqual(fp.security_group_rules(SG, ['104.7.12.215/32', '52.119.115.124/32']), [
            {'IpProtocol': 'tcp', 'FromPort': 22, 'ToPort': 22,
             'IpRanges': [{'CidrIp': '104.7.12.215/32'}, {'CidrIp': '52.119.115.124/32'}]},
            {'IpProtocol': 'tcp', 'FromPort': 22, 'ToPort': 22, 'UserIdGroupPairs': [{'GroupId': SG}]},
            {'IpProtocol': 'tcp', 'FromPort': 443, 'ToPort': 443, 'UserIdGroupPairs': [{'GroupId': SG}]},
            {'IpProtocol': 'tcp', 'FromPort': 17110, 'ToPort': 17110, 'UserIdGroupPairs': [{'GroupId': SG}]},
            {'IpProtocol': 'udp', 'FromPort': 17110, 'ToPort': 17110, 'UserIdGroupPairs': [{'GroupId': SG}]},
        ])
        self.assertEqual(fp.GOSSIP_PORT, 17110)

    def test_refuses_world_open(self):
        with self.assertRaisesRegex(ValueError, 'world-open'):
            fp.security_group_rules(SG, ['0.0.0.0/0'])
        with self.assertRaisesRegex(ValueError, 'world-open'):
            fp.security_group_rules(SG, ['104.7.12.215/32', '0.0.0.0/0'])

    def test_refuses_non_host_and_malformed_sources(self):
        for cidrs in (['104.7.12.0/24'], ['104.7.12.215'], ['::/0'], ['2001:db8::1/128'], ['0.0.0.0/32'],
                      ['104.7.12.215/32', '104.7.12.215/32'], [], 'x', ['104.7.12.216/31']):
            with self.subTest(cidrs=cidrs), self.assertRaises(ValueError):
                fp.security_group_rules(SG, cidrs)
        for group in ('sg-XYZ', 'vpc-0123456789abcdef0', ''):
            with self.subTest(group=group), self.assertRaises(ValueError):
                fp.security_group_rules(group, ['104.7.12.215/32'])


class BootstrapTests(unittest.TestCase):
    def script(self, role='n1'):
        return fp.bootstrap(role, RUN, CREATED, HOST_END)

    def test_golden_bootstrap(self):
        script = self.script()
        subprocess.run(['bash', '-n'], input=script.encode(), check=True)
        self.assertTrue(script.startswith('#!/bin/bash\nset -euo pipefail\n'))
        calendar = re.findall(r'^OnCalendar=(.*)$', script, re.M)
        self.assertEqual(calendar, ['2026-11-04 10:00:00 UTC'])
        self.assertEqual(calendar[0], datetime.datetime.fromtimestamp(HOST_END, datetime.timezone.utc)
                         .strftime('%Y-%m-%d %H:%M:%S UTC'))
        self.assertIn("printf '%%s\\n' '%d' > /etc/botho-qualification/absolute-stop" % HOST_END, script)
        self.assertIn('if [ "$(date +%%s)" -ge %d ]; then systemctl poweroff; exit 1; fi' % HOST_END, script)
        maintenance = host_maintenance.bootstrap(RUN, CREATED, HOST_END)
        self.assertIn(maintenance, script)
        # Order: absolute stop timer, then maintenance exclusion, then packages, then markers.
        timer = script.index('systemctl enable --now botho-qualification-stop.timer')
        block = script.index(maintenance)
        install = script.index('apt-get update')
        markers = script.index('/etc/botho-qualification/role')
        self.assertLess(timer, block)
        self.assertLess(block, install)
        self.assertLess(install, markers)
        self.assertIn("printf '%s\\n' 'n1' > /etc/botho-qualification/role", script)
        self.assertIn("printf '%s\\n' '" + RUN + "' > /etc/botho-qualification/run-id", script)

    def test_no_ledger_rules_or_w_series_tokens(self):
        for role in fp.ROLES:
            script = self.script(role)
            self.assertNotIn('ledger_rules', script)
            self.assertNotIn('qualification_v3', script)
            self.assertIsNone(re.search(r'(?<![a-z0-9])w[0-9]+(?![a-z0-9])', script), role)

    def test_lifetime_bounds(self):
        for created, end in ((CREATED, CREATED), (CREATED, CREATED + 82 * 3600 + 1), (CREATED, CREATED - 1),
                             (str(CREATED), HOST_END)):
            with self.subTest(end=end), self.assertRaises(ValueError):
                fp.bootstrap('n1', RUN, created, end)
        with self.assertRaises(ValueError):
            fp.bootstrap('n9', RUN, CREATED, HOST_END)


class HardeningTests(unittest.TestCase):
    """#1610 items 2, 5 and 8: netem prerequisites, observer sshd and the keys directory."""

    WRAPPER = '/usr/local/libexec/botho-observer-' + RUN + '-b'

    def test_authorized_keys_directory_is_traversable(self):
        for role in fp.ROLES:
            script = fp.bootstrap(role, RUN, CREATED, HOST_END)
            self.assertIn('install -d -m 755 -o root -g root /etc/ssh/authorized_keys\n', script)
            self.assertNotIn('-m 700 /etc/ssh/authorized_keys', script)

    def test_netem_probe_on_nodes_only(self):
        script = fp.bootstrap('n5', RUN, CREATED, HOST_END)
        self.assertIn('netem_probe() { ' + fp.NETEM_PROBE + '; }', script)
        # The probe is the D4 stack: prio root, netem on band 4, u32 gossip filter.
        for token in ('prio bands 4', 'parent 1:4 handle 40: netem', 'match ip dport 17110 0xffff flowid 1:4'):
            self.assertIn(token, fp.NETEM_PROBE)
        self.assertIn('install -y -qq "linux-modules-extra-$(uname -r)"', script)
        self.assertIn('iproute2 iptables', script)
        probe = script.index('netem_probe() {')
        self.assertLess(script.index('apt-get update'), probe)
        self.assertLess(probe, script.index('/etc/botho-qualification/role'))
        # The extra modules are installed only when the first probe fails, and the second is fatal.
        block = script[probe:script.index('/etc/botho-qualification/netem-ok')]
        self.assertRegex(block, r'if ! netem_probe; then\n(?:.*\n){2}  modprobe sch_netem\n  netem_probe\nfi\n')
        self.assertIn("printf 'sch_netem\\nsch_prio\\ncls_u32\\n' > /etc/modules-load.d/botho-x-faults.conf", script)
        controller = fp.bootstrap('controller', RUN, CREATED, HOST_END)
        self.assertNotIn('netem', controller)

    def test_bootstrap_hardens_observer_account(self):
        script = fp.bootstrap('n1', RUN, CREATED, HOST_END)
        conf = script[script.index('40-botho-qualification.conf <<'):]
        conf = conf[:conf.index('\nEOF\n')]
        self.assertIn('\nMatch User botho\n', conf)
        for key, value in fp.OBSERVER_SSHD_OPTIONS:
            self.assertIn('\n    %s %s' % (key, value), conf)
        self.assertNotIn('ForceCommand', conf)  # The wrapper path is fixed at staging.
        # The Match block ends the drop-in, so no global option falls inside it.
        self.assertLess(conf.index('AuthorizedKeysFile .ssh/'), conf.index('Match User'))

    def test_observer_sshd_config_forces_wrapper(self):
        text = fp.observer_sshd_config(self.WRAPPER)
        self.assertIn('Match User botho\n    ForceCommand %s\n' % self.WRAPPER, text)
        for key in ('AllowTcpForwarding', 'AllowAgentForwarding', 'AllowStreamLocalForwarding',
                    'X11Forwarding', 'PermitTTY', 'PermitTunnel'):
            self.assertIn('    %s no\n' % key, text)
        for wrapper in ('/usr/local/libexec/../bin/sh', '/tmp/x', 'botho-observer', '/usr/local/libexec/a b',
                        "/usr/local/libexec/x'y", '/usr/local/libexec/a/b'):
            with self.subTest(wrapper=wrapper), self.assertRaises(ValueError):
                fp.observer_sshd_config(wrapper)
        for user in ('root', 'Botho', 'a b', ''):
            with self.subTest(user=user), self.assertRaises(ValueError):
                fp.observer_sshd_config(self.WRAPPER, user)

    def test_observer_sshd_install_script(self):
        script = fp.observer_sshd_install(self.WRAPPER)
        subprocess.run(['bash', '-n'], input=script.encode(), check=True)
        self.assertIn(fp.observer_sshd_config(self.WRAPPER), script)
        self.assertIn("test -x '%s'" % self.WRAPPER, script)
        self.assertIn('chmod 755 /etc/ssh/authorized_keys', script)
        # Validate before reload, and roll back on a failed validation.
        self.assertLess(script.index('/usr/sbin/sshd -t'), script.index('systemctl reload ssh'))
        self.assertIn("mv -f '%s.previous' '%s'" % ((fp.OBSERVER_SSHD_DROPIN,) * 2), script)
        # Neither the temp file nor the backup matches sshd's `*.conf` include.
        self.assertTrue(fp.OBSERVER_SSHD_DROPIN.endswith('.conf'))
        self.assertIn('mktemp /etc/ssh/sshd_config.d/.botho-observer.XXXXXX', script)

    @unittest.skipUnless(Path('/usr/sbin/sshd').exists() and shutil.which('ssh-keygen'), 'needs OpenSSH')
    def test_sshd_accepts_rendered_blocks(self):
        bootstrap = fp.bootstrap('n1', RUN, CREATED, HOST_END)
        conf = bootstrap[bootstrap.index('40-botho-qualification.conf <<') :]
        conf = conf[conf.index('\n') + 1:conf.index('\nEOF\n') + 1]
        with tempfile.TemporaryDirectory() as tmp:
            key = Path(tmp) / 'host'
            subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(key)], check=True)
            path = Path(tmp) / 'sshd_config'
            path.write_text(conf + fp.observer_sshd_config(self.WRAPPER))
            done = subprocess.run(['/usr/sbin/sshd', '-t', '-f', str(path), '-h', str(key)],
                                  capture_output=True, text=True)
            if done.returncode and str(path) not in done.stderr:
                # Host-environment failures (e.g. no /run/sshd) are not config errors.
                self.skipTest('sshd -t cannot run here: ' + done.stderr.strip())
            self.assertEqual(done.returncode, 0, done.stderr)


class KeyscanTests(unittest.TestCase):
    IPS = ['10.0.1.11', '10.0.1.12']
    KEY = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOQ1fXv2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0'

    def test_argv(self):
        self.assertEqual(fp.keyscan_argv(self.IPS),
                         ['ssh-keyscan', '-T', '10', '-t', 'ed25519', '10.0.1.11', '10.0.1.12'])
        for ips in ([], ['8.8.8.8'], ['10.0.1.11', '10.0.1.11'], ['10.0.1'], ['10.0.1.11; id']):
            with self.subTest(ips=ips), self.assertRaises(ValueError):
                fp.keyscan_argv(ips)

    def test_parse(self):
        text = '# 10.0.1.11:22 SSH-2.0-OpenSSH_9.6\n10.0.1.11 %s\n10.0.1.12 %s\n' % (self.KEY, self.KEY)
        self.assertEqual(fp.parse_keyscan(text, self.IPS), {ip: self.KEY for ip in self.IPS})
        for bad, needle in (('10.0.1.11 %s\n' % self.KEY, 'no key scanned'),
                            (text + '10.0.1.13 %s\n' % self.KEY, 'unexpected host'),
                            (text + '10.0.1.11 %s\n' % self.KEY, 'more than one'),
                            (text.replace('ssh-ed25519', 'ssh-rsa', 1), 'not one ed25519')):
            with self.subTest(needle=needle), self.assertRaisesRegex(ValueError, needle):
                fp.parse_keyscan(bad, self.IPS)


class PlanTests(unittest.TestCase):
    def test_golden_plan(self):
        plan = fp.infrastructure_plan(RUN, CREATED, **PLAN_ARGS)
        self.assertEqual(plan['host_end'], CREATED + 82 * 3600)
        self.assertEqual(plan['node_end'], CREATED + 81 * 3600 + 1800)
        self.assertLess(plan['node_end'], plan['host_end'])
        self.assertEqual(plan['prefund_end'], CREATED + 6 * 3600)
        self.assertEqual(plan['run_id'], RUN)
        self.assertEqual(plan['operator_cidrs'], PLAN_ARGS['cidrs'])
        self.assertEqual(plan['roles'], [
            {'role': 'controller', 'type': 'm6i.2xlarge', 'disk_gib': 80},
            {'role': 'n1', 'type': 'c6i.large', 'disk_gib': 40},
            {'role': 'n2', 'type': 'c6i.large', 'disk_gib': 40},
            {'role': 'n3', 'type': 'm6i.large', 'disk_gib': 40},
            {'role': 'n4', 'type': 'c6i.large', 'disk_gib': 40},
            {'role': 'n5', 'type': 'c6i.large', 'disk_gib': 40},
        ])

    def test_plan_refusals(self):
        for key, value in (('cidrs', ['0.0.0.0/0']), ('subnet', 'subnet-x'), ('vpc', 'sg-0123456789abcdef0'),
                           ('image', 'ami-'), ('region', 'US-WEST-2'), ('scope', ' ')):
            with self.subTest(key=key), self.assertRaises(ValueError):
                fp.infrastructure_plan(RUN, CREATED, **dict(PLAN_ARGS, **{key: value}))
        with self.assertRaises(ValueError):
            fp.infrastructure_plan('q20261022-w12-endurance', CREATED, **PLAN_ARGS)

    def test_rehearsal_lifetime(self):
        life = 6 * 3600
        plan = fp.infrastructure_plan(RUN, CREATED, execution_mode='rehearsal_only', lifetime=life, **PLAN_ARGS)
        self.assertEqual(plan['host_end'], CREATED + life)
        self.assertEqual(plan['node_end'], CREATED + life - 1800)
        self.assertEqual(plan['prefund_end'], CREATED + life - 1800)  # Never past the nodes.
        self.assertEqual((plan['execution_mode'], plan['host_lifetime']), ('rehearsal_only', life))
        launch = fp.launch_request(plan, 'n1', SG)
        script = base64.b64decode(launch['UserData']).decode()
        self.assertIn('OnCalendar=2026-11-01 06:00:00 UTC', script)
        self.assertEqual(fp.tags(RUN, 'n1', CREATED + life)[-1], {'Key': 'ExpiresAt', 'Value': str(CREATED + life)})
        # A rehearsal without an override keeps 82 h, and its plan is the campaign plan.
        self.assertEqual(fp.infrastructure_plan(RUN, CREATED, execution_mode='rehearsal_only', **PLAN_ARGS),
                         fp.infrastructure_plan(RUN, CREATED, **PLAN_ARGS))
        for mode, value in (('campaign', life), ('rehearsal_only', fp.MIN_REHEARSAL_LIFETIME - 1),
                            ('rehearsal_only', 82 * 3600 + 1), ('rehearsal_only', float(life)),
                            ('rehearsal_only', True), ('later', None)):
            with self.subTest(mode=mode, value=value), self.assertRaises(ValueError):
                fp.infrastructure_plan(RUN, CREATED, execution_mode=mode, lifetime=value, **PLAN_ARGS)
        # A frozen plan cannot be edited into another lifetime, or into a campaign.
        for tampered in (dict(plan, host_end=plan['host_end'] + 3600), dict(plan, execution_mode='campaign'),
                         dict(plan, host_lifetime=life + 3600)):
            with self.assertRaises(ValueError):
                fp.launch_request(tampered, 'n1', SG)

    def test_launch_request_tags_at_creation(self):
        plan = fp.infrastructure_plan(RUN, CREATED, **PLAN_ARGS)
        launch = fp.launch_request(plan, 'n3', SG)
        self.assertEqual(launch['InstanceType'], 'm6i.large')
        self.assertEqual(launch['ClientToken'], 'botho-' + RUN + '-n3')
        self.assertEqual(launch['TagSpecifications'], fp.tag_specifications(RUN, 'n3', plan['host_end']))
        self.assertEqual(launch['NetworkInterfaces'][0]['Groups'], [SG])
        self.assertEqual(launch['MetadataOptions']['HttpTokens'], 'required')
        self.assertEqual(launch['InstanceInitiatedShutdownBehavior'], 'stop')
        self.assertEqual(base64.b64decode(launch['UserData']).decode(), fp.bootstrap('n3', RUN, CREATED, plan['host_end']))
        tampered = dict(plan, host_end=plan['host_end'] + 3600)
        with self.assertRaises(ValueError):
            fp.launch_request(tampered, 'n3', SG)


if __name__ == '__main__':
    unittest.main()
