import copy
import json
import os
from pathlib import Path
import re
import tempfile
import tomllib
import unittest
from unittest import mock

import fleet_nodes as fn
import fleet_provision as fp
from observer import node_config
from runtime import HOSTS, campaign_targets

RUN = 'q20261101-x1-endurance'
NODE_END = 1793491200 + fp.NODE_LIFETIME
PROFILE = {'run_id': RUN, 'producer_role': 'n3', 'node_end': NODE_END, 'rpc_requests_per_minute': 300}
MNEMONIC = ' '.join(['abandon'] * 11 + ['about'])  # BIP-39 test vector; never a real wallet.
IPS = {'controller': '172.31.20.10', 'n1': '172.31.20.11', 'n2': '172.31.20.12',
       'n3': '172.31.20.13', 'n4': '172.31.20.14', 'n5': '172.31.20.15'}
INVENTORY = [{'role': r, 'private_ip': ip, 'hostname': '%s.%s.internal' % (r, RUN)} for r, ip in IPS.items()]
# Launch instant: node_end is NODE_LIFETIME later, inside the 82 h mint window.
NOW = NODE_END - fp.NODE_LIFETIME
BASE58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'


def b58(data):
    """Independent base58btc encoder, so the tests do not trust fn.b58encode."""
    number, out = int.from_bytes(data, 'big'), ''
    while number:
        number, digit = divmod(number, 58)
        out = BASE58[digit] + out
    return '1' * (len(data) - len(data.lstrip(b'\0'))) + out


def ed25519_peer_id(key):
    """A real libp2p Ed25519 PeerId: identity multihash of the protobuf public key."""
    assert len(key) == 32
    return b58(bytes([0x00, 0x24, 0x08, 0x01, 0x12, 0x20]) + key)


IDENTITIES = {r: {'peer_id': ed25519_peer_id(bytes([0x40 + i]) * 31 + bytes([i]))}
              for i, r in enumerate(fn.VALIDATORS)}
TOP = {'network_type', 'network', 'minting', 'faucet', 'telemetry', 'rpc'}


def config(role, **kw):
    args = dict(inventory=INVENTORY, identities=IDENTITIES, profile=PROFILE,
                producer_mnemonic=MNEMONIC if role == 'n3' else None)
    args.update(kw)
    return fn.render_config(role, **args)


def peer(role):
    return IDENTITIES[role]['peer_id']


class ConfigGoldenTests(unittest.TestCase):
    def test_n1_golden(self):
        text = config('n1')
        others = [r for r in fn.VALIDATORS if r != 'n1']
        expected = (
            'network_type = "testnet"\n\n'
            '[network]\n'
            'gossip_port = 17110\nrpc_port = 19090\nmetrics_port = 19100\n'
            'node_key_path = "/var/lib/botho-%s/node_key"\n' % RUN
            + 'bootstrap_peers = [\n'
            + ''.join('    "/ip4/%s/tcp/17110/p2p/%s",\n' % (IPS[r], peer(r)) for r in others)
            + ']\nfallback_seeds = false\n\n'
            '[network.dns_seeds]\nenabled = false\n\n'
            '[network.quorum]\nmode = "explicit"\nthreshold = 4\nmin_peers = 4\nmax_auto_members = 0\n'
            'members = [\n' + ''.join('    "%s",\n' % peer(r) for r in others) + ']\n\n'
            '[minting]\nenabled = false\nthreads = 1\n\n'
            '[faucet]\nenabled = false\n\n'
            '[telemetry]\nenabled = false\n\n'
            '[rpc]\nrequests_per_minute = 300\n\n')
        self.assertEqual(text, expected)
        doc = tomllib.loads(text)
        quorum = doc['network']['quorum']
        self.assertEqual(quorum['threshold'], 4)
        self.assertEqual(len(quorum['members']), 4)
        self.assertNotIn(peer('n1'), quorum['members'])
        self.assertIs(doc['network']['fallback_seeds'], False)
        self.assertIs(doc['network']['dns_seeds']['enabled'], False)
        self.assertNotEqual(doc['network']['rpc_port'], doc['network']['metrics_port'])
        self.assertNotIn('wallet', doc)
        self.assertIs(doc['minting']['enabled'], False)
        self.assertNotIn('ledger_rules', text)

    def test_ports_come_from_fleet_provision(self):
        net = tomllib.loads(config('n2'))['network']
        self.assertEqual((net['gossip_port'], net['rpc_port'], net['metrics_port']),
                         (fp.GOSSIP_PORT, fp.RPC_PORT, fp.METRICS_PORT))
        with mock.patch.object(fp, 'GOSSIP_PORT', 17111):
            self.assertEqual(tomllib.loads(config('n2'))['network']['gossip_port'], 17111)
        self.assertNotRegex(Path(fn.__file__).read_text(), r'\b(17110|19090|19100)\b')

    def test_only_producer_mints_and_holds_wallet(self):
        for role in fn.VALIDATORS:
            doc = tomllib.loads(config(role))
            unit = fn.render_node_unit(role, INVENTORY, PROFILE, NOW)
            flags = '--mint --mint-threads 1 --testnet-mint-until %d' % NODE_END
            if role == 'n3':
                self.assertEqual(doc['wallet'], {'mnemonic': MNEMONIC})
                self.assertIs(doc['minting']['enabled'], True)
                self.assertIn(' run ' + flags + '\n', unit)
            else:
                self.assertNotIn('wallet', doc)
                self.assertIs(doc['minting']['enabled'], False)
                self.assertNotIn('--mint', unit)
                self.assertNotIn('--testnet-mint-until', unit)

    def test_exact_key_set_at_every_level(self):
        expected = {
            (): TOP,
            ('network',): {'gossip_port', 'rpc_port', 'metrics_port', 'node_key_path', 'bootstrap_peers',
                           'fallback_seeds', 'dns_seeds', 'quorum'},
            ('network', 'dns_seeds'): {'enabled'},
            ('network', 'quorum'): {'mode', 'threshold', 'min_peers', 'max_auto_members', 'members'},
            ('minting',): {'enabled', 'threads'},
            ('faucet',): {'enabled'},
            ('telemetry',): {'enabled'},
            ('rpc',): {'requests_per_minute'},
        }
        for role in fn.VALIDATORS:
            want = dict(expected)
            if role == 'n3':
                want[()] = TOP | {'wallet'}
                want[('wallet',)] = {'mnemonic'}
            self.assertEqual(dict(fn._keys(tomllib.loads(config(role)))), want, role)

    def test_unknown_key_is_refused(self):
        text = config('n1')
        for typo in (text.replace('fallback_seeds', 'fallback_seed'),
                     text.replace('[faucet]\n', '[faucet]\namount = 1\n'),
                     'ledger_rules = "qualification_v3"\n' + text,
                     text + '[extra]\nenabled = false\n'):
            with self.assertRaises(ValueError):
                fn.check_config('n1', typo, INVENTORY, IDENTITIES, PROFILE)

    def test_byzantine_tolerance_is_one_for_every_role(self):
        for role in fn.VALIDATORS:
            quorum = tomllib.loads(config(role))['network']['quorum']
            n = len([m for m in quorum['members'] if m != peer(role)]) + 1
            t = quorum['threshold']
            self.assertEqual((n, t), (5, 4))
            self.assertEqual(min(n - t, 2 * t - n - 1), 1, role)

    def test_no_public_seed_or_hosts_name(self):
        for role in fn.VALIDATORS:
            for text in (config(role), fn.render_node_unit(role, INVENTORY, PROFILE, NOW),
                         fn.render_nginx(role, INVENTORY, PROFILE)):
                self.assertNotIn('botho.io', text)
                for host in HOSTS:
                    self.assertNotIn(host, text)

    def test_peers_are_private_on_gossip_port(self):
        for role in fn.VALIDATORS:
            for addr in tomllib.loads(config(role))['network']['bootstrap_peers']:
                match = fn.MULTIADDR.fullmatch(addr)
                self.assertTrue(match['ip'].startswith('172.31.'))
                self.assertEqual(int(match['port']), fp.GOSSIP_PORT)
                self.assertNotEqual(match['ip'], IPS[role])


class UnitGoldenTests(unittest.TestCase):
    def test_validator_unit(self):
        unit = fn.render_node_unit('n1', INVENTORY, PROFILE, NOW)
        lines = unit.splitlines()
        self.assertIn('Restart=no', lines)
        self.assertIn('TimeoutStopSec=30', lines)
        self.assertIn('IPAddressDeny=any', lines)
        allow = [line.split('=', 1)[1] for line in lines if line.startswith('IPAddressAllow=')]
        self.assertEqual(allow, ['localhost'] + [IPS[r] + '/32' for r in fp.ROLES])
        self.assertIn('ExecStart=/opt/botho-%s/bin/botho --testnet --config /var/lib/botho-%s/config.toml run' % (RUN, RUN), lines)
        self.assertIn('MemoryMax=2G', lines)
        self.assertIn('RuntimeMaxSec=%d' % fp.NODE_LIFETIME, lines)
        self.assertNotRegex(unit, r'(?m)^Restart=(?!no$)')
        self.assertIn('MemoryMax=5G', fn.render_node_unit('n3', INVENTORY, PROFILE, NOW).splitlines())

    def test_node_stop_timer_at_node_end(self):
        service, timer = fn.render_node_stop(PROFILE, NOW)
        self.assertIn('ExecStart=/usr/bin/systemctl stop botho-%s.service' % RUN, service)
        self.assertIn('OnCalendar=%s' % fp.on_calendar(NODE_END), timer.splitlines())
        self.assertIn('OnCalendar=2026-11-04 09:30:00 UTC', timer.splitlines())
        self.assertIn('Unit=botho-%s-node-stop.service' % RUN, timer.splitlines())


class NginxGoldenTests(unittest.TestCase):
    def test_nginx_golden(self):
        self.assertEqual(fn.render_nginx('n2', INVENTORY, PROFILE), f'''server {{
    listen 443 ssl;
    server_name n2.{RUN}.internal;
    ssl_certificate /etc/botho-{RUN}/tls.crt;
    ssl_certificate_key /etc/botho-{RUN}/tls.key;
    client_max_body_size 1m;
    location = /rpc {{ proxy_pass http://127.0.0.1:19090/rpc; proxy_read_timeout 30s; }}
    location = /metrics {{ proxy_pass http://127.0.0.1:19100/metrics; proxy_read_timeout 30s; }}
    location / {{ return 404; }}
}}
''')

    def test_metrics_goes_to_metrics_port_not_rpc(self):
        text = fn.render_nginx('n1', INVENTORY, PROFILE)
        locations = dict(re.findall(r'location (= /\w+|/) \{ ([^;]+);', text))
        self.assertEqual(locations, {'= /rpc': 'proxy_pass http://127.0.0.1:%d/rpc' % fp.RPC_PORT,
                                     '= /metrics': 'proxy_pass http://127.0.0.1:%d/metrics' % fp.METRICS_PORT,
                                     '/': 'return 404'})


class StagingFragmentTests(unittest.TestCase):
    def test_fragment_validates_in_hosts_order(self):
        fragment = fn.render_staging_fragment(INVENTORY, PROFILE)
        self.assertEqual(set(fragment), {'targets', 'observers'})
        urls = ['https://%s.%s.internal/rpc' % (r, RUN) for r in fn.VALIDATORS]
        mapping = campaign_targets({'targets': fragment['targets']}, {'endpoints': urls})
        self.assertEqual(list(mapping), list(HOSTS))
        self.assertEqual([o['role'] for o in fragment['observers']], list(HOSTS))
        for item in fragment['observers']:
            self.assertEqual(node_config(item['node']), item['node'])
            self.assertEqual(set(item['node']), {'node_unit', 'binary_path', 'data_dir', 'config_path', 'node_key_path'})
            self.assertEqual(item['node']['node_unit'], 'botho-%s.service' % RUN)
        self.assertEqual(fragment['targets'][2]['observer_ssh_target'], 'botho@n3.%s.internal' % RUN)


class RefusalTests(unittest.TestCase):
    def test_public_ip_in_inventory(self):
        inventory = copy.deepcopy(INVENTORY)
        inventory[2]['private_ip'] = '52.119.115.124'
        with self.assertRaisesRegex(ValueError, 'RFC1918'):
            config('n1', inventory=inventory)
        inventory[2]['private_ip'] = '100.64.0.5'  # CGNAT: private-ish, not RFC1918.
        with self.assertRaisesRegex(ValueError, 'RFC1918'):
            fn.render_node_unit('n1', inventory, PROFILE, NOW)

    def test_malformed_run_id(self):
        for bad in ('q20261101-w12-endurance', 'Q20261101-x1-endurance', 'q20261341-x1-endurance', 'x1-endurance'):
            with self.assertRaisesRegex(ValueError, 'run_id'):
                config('n1', profile=dict(PROFILE, run_id=bad))

    def test_self_in_members(self):
        identities = copy.deepcopy(IDENTITIES)
        identities['n2'] = dict(identities['n1'])
        with self.assertRaisesRegex(ValueError, 'self'):
            config('n1', identities=identities)
        text = config('n1').replace(peer('n2'), peer('n1'))
        with self.assertRaisesRegex(ValueError, 'self'):
            fn.check_config('n1', text, INVENTORY, IDENTITIES, PROFILE)

    def test_equal_rpc_and_metrics_ports(self):
        with mock.patch.object(fp, 'METRICS_PORT', fp.RPC_PORT):
            with self.assertRaisesRegex(ValueError, 'differ'):
                config('n1')
            with self.assertRaisesRegex(ValueError, 'differ'):
                fn.render_nginx('n1', INVENTORY, PROFILE)

    def test_mnemonic_file_not_owner_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'producer.mnemonic'
            path.write_text(MNEMONIC + '\n')
            for mode in (0o644, 0o640, 0o604, 0o660):
                os.chmod(path, mode)
                with self.assertRaisesRegex(ValueError, 'owner-only'):
                    fn.read_private(path)
            os.chmod(path, 0o600)
            self.assertEqual(fn.read_private(path), MNEMONIC)
            link = Path(tmp) / 'link'
            link.symlink_to(path)
            with self.assertRaisesRegex(ValueError, 'symlink'):
                fn.read_private(link)

    def test_mnemonic_file_checks_run_on_the_opened_fd(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'producer.mnemonic'
            path.write_text(MNEMONIC + '\n')
            os.chmod(path, 0o600)
            # Not owned by the effective user.
            with mock.patch.object(fn.os, 'geteuid', return_value=os.geteuid() + 1):
                with self.assertRaisesRegex(ValueError, 'owned'):
                    fn.read_private(path)
            # A directory is not a regular file.
            with self.assertRaisesRegex(ValueError, 'regular file'):
                fn.read_private(Path(tmp))
            # The open is O_NOFOLLOW | O_CLOEXEC, and lstat is never consulted.
            real_open = os.open
            flags = []
            with mock.patch.object(fn.os, 'open', side_effect=lambda p, f, *a: flags.append(f) or real_open(p, f, *a)), \
                    mock.patch.object(fn.Path, 'lstat', side_effect=AssertionError('lstat used')):
                self.assertEqual(fn.read_private(path), MNEMONIC)
            self.assertTrue(flags[0] & os.O_NOFOLLOW and flags[0] & os.O_CLOEXEC)
            # Missing file: a ValueError, not a traceback.
            with self.assertRaisesRegex(ValueError, 'cannot open'):
                fn.read_private(Path(tmp) / 'absent')

    def test_mnemonic_only_on_producer(self):
        with self.assertRaisesRegex(ValueError, 'producer'):
            config('n1', producer_mnemonic=MNEMONIC)
        with self.assertRaisesRegex(ValueError, 'producer'):
            config('n3', producer_mnemonic=None)

    def test_threshold_is_not_a_profile_knob(self):
        with self.assertRaisesRegex(ValueError, 'exactly'):
            config('n1', profile=dict(PROFILE, threshold=5))
        text = config('n1').replace('threshold = 4', 'threshold = 5')
        with self.assertRaises(ValueError):
            fn.check_config('n1', text, INVENTORY, IDENTITIES, PROFILE)

    def test_inventory_shape(self):
        with self.assertRaisesRegex(ValueError, 'exactly'):
            config('n1', inventory=INVENTORY[1:])
        dup = copy.deepcopy(INVENTORY)
        dup[1]['private_ip'] = dup[2]['private_ip']
        with self.assertRaisesRegex(ValueError, 'distinct'):
            config('n1', inventory=dup)
        renamed = copy.deepcopy(INVENTORY)
        renamed[1]['hostname'] = 'seed.botho.io'
        with self.assertRaisesRegex(ValueError, 'hostname'):
            config('n1', inventory=renamed)


class PeerIdTests(unittest.TestCase):
    def test_real_ids_parse_and_round_trip(self):
        for i, role in enumerate(fn.VALIDATORS):
            self.assertEqual(fn.parse_peer_id(peer(role)), bytes([0x40 + i]) * 31 + bytes([i]))
        # A real node id from the repo's docs/config, generated by libp2p.
        self.assertEqual(len(fn.parse_peer_id('12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ')), 32)

    def test_regex_valid_garbage_is_refused(self):
        for bad in ('12D3KooW' + 'r' * 44, '12D3KooW' + 'n1' * 22, '12D3KooW' + '1' * 44):
            self.assertIsNotNone(re.fullmatch(r'(?:12D3KooW|Qm)[1-9A-HJ-NP-Za-km-z]{36,52}', bad))
            with self.assertRaises(ValueError):
                fn.parse_peer_id(bad)
            identities = copy.deepcopy(IDENTITIES)
            identities['n4'] = {'peer_id': bad}
            with self.assertRaisesRegex(ValueError, 'invalid peer_id for n4'):
                config('n1', identities=identities)

    def test_bad_prefix_byte_is_refused(self):
        key = bytes(range(32))
        good = bytes([0x00, 0x24, 0x08, 0x01, 0x12, 0x20])
        for index, value in ((2, 0x08), (3, 0x00), (3, 0x02), (5, 0x21)):  # key type / data length.
            prefix = bytearray(good)
            if prefix[index] == value:
                continue
            prefix[index] = value
            with self.assertRaises(ValueError):
                fn.parse_peer_id(b58(bytes(prefix) + key))
        # sha2-256 multihash (RSA-style Qm id) and a secp256k1 key type.
        with self.assertRaises(ValueError):
            fn.parse_peer_id(b58(bytes([0x12, 0x20]) + key))
        with self.assertRaises(ValueError):
            fn.parse_peer_id(b58(bytes([0x00, 0x25, 0x08, 0x02, 0x12, 0x21, 0x02]) + key))

    def test_wrong_length_is_refused(self):
        prefix = bytes([0x00, 0x24, 0x08, 0x01, 0x12, 0x20])
        for key in (bytes(range(31)), bytes(range(33))):
            with self.assertRaises(ValueError):
                fn.parse_peer_id(b58(prefix + key))
        with self.assertRaises(ValueError):
            fn.parse_peer_id(peer('n1')[:-1])

    def test_non_round_tripping_is_refused(self):
        # An extra leading '1' decodes to an extra zero byte; never canonical.
        with self.assertRaises(ValueError):
            fn.parse_peer_id('1' + peer('n1')[:-1])
        with mock.patch.object(fn, 'b58encode', return_value='12D3KooW' + 'x' * 44):
            with self.assertRaisesRegex(ValueError, 'round-trip'):
                fn.parse_peer_id(peer('n1'))


class NodeEndTests(unittest.TestCase):
    def test_window_bounds(self):
        limit = 82 * 3600
        self.assertEqual(fn.MAX_TESTNET_MINT_WINDOW_SECS, limit)
        self.assertEqual(fn.check_node_end(PROFILE, NODE_END - limit), NODE_END)
        self.assertEqual(fn.check_node_end(PROFILE, NODE_END - 1), NODE_END)
        for now, why in ((NODE_END - limit - 1, 'ahead'), (NODE_END, 'future'), (NODE_END + 3600, 'future')):
            with self.assertRaisesRegex(ValueError, why):
                fn.check_node_end(PROFILE, now)
            with self.assertRaisesRegex(ValueError, why):
                fn.render_node_unit('n3', INVENTORY, PROFILE, now)
            with self.assertRaisesRegex(ValueError, why):
                fn.render_node_stop(PROFILE, now)
            with self.assertRaisesRegex(ValueError, why):
                fn.render_payloads(INVENTORY, IDENTITIES, PROFILE, MNEMONIC, now)

    def test_wall_clock_is_the_default(self):
        with mock.patch.object(fn.time, 'time', return_value=float(NOW)):
            self.assertEqual(fn.check_node_end(PROFILE), NODE_END)
        with mock.patch.object(fn.time, 'time', return_value=float(NODE_END + 1)):
            with self.assertRaisesRegex(ValueError, 'future'):
                fn.render_node_unit('n1', INVENTORY, PROFILE)

    def test_constant_mirrors_rust(self):
        run_rs = Path(__file__).resolve().parents[2] / 'botho' / 'src' / 'commands' / 'run.rs'
        self.assertIn('const MAX_TESTNET_MINT_WINDOW_SECS: u64 = 82 * 60 * 60;', run_rs.read_text())


class CliTests(unittest.TestCase):
    def test_render_writes_private_payloads(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            (tmp / 'profile.json').write_text(json.dumps(PROFILE))
            (tmp / 'inventory.json').write_text(json.dumps(INVENTORY))
            (tmp / 'identity').mkdir()
            (tmp / 'identity' / 'nodes.json').write_text(json.dumps(IDENTITIES))
            mnemonic = tmp / 'producer.mnemonic'
            mnemonic.write_text(MNEMONIC + '\n')
            os.chmod(mnemonic, 0o600)
            out = tmp / 'out'
            argv = ['render', '--profile', str(tmp / 'profile.json'), '--inventory', str(tmp / 'inventory.json'),
                    '--identities', str(tmp / 'identity'), '--producer-mnemonic', str(mnemonic), '--output', str(out)]
            with mock.patch('sys.stdout'), mock.patch.object(fn, '_now', return_value=NOW):
                self.assertEqual(fn.main(argv), 0)
            self.assertEqual(sorted(p.name for p in out.iterdir()),
                             ['payload-' + r for r in fn.VALIDATORS] + ['staging-fragment.json'])
            for role in fn.VALIDATORS:
                directory = out / ('payload-' + role)
                self.assertEqual(sorted(p.name for p in directory.iterdir()), sorted(fn.PAYLOAD_FILES))
                for path in directory.iterdir():
                    self.assertEqual(path.stat().st_mode & 0o777, 0o600, path)
                self.assertEqual((directory / 'config.toml').read_text(), config(role))
            self.assertEqual((out / 'staging-fragment.json').stat().st_mode & 0o777, 0o600)
            self.assertEqual(json.loads((out / 'staging-fragment.json').read_text()),
                             fn.render_staging_fragment(INVENTORY, PROFILE))
            # A second render never overwrites.
            with mock.patch('sys.stderr'), mock.patch.object(fn, '_now', return_value=NOW):
                self.assertEqual(fn.main(argv), 2)
            os.chmod(mnemonic, 0o640)
            argv[-1] = str(tmp / 'out2')
            with mock.patch('sys.stderr'), mock.patch.object(fn, '_now', return_value=NOW):
                self.assertEqual(fn.main(argv), 2)
            self.assertFalse((tmp / 'out2').exists())


if __name__ == '__main__':
    unittest.main()
