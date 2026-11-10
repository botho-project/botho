"""Preflight refuses any node that does not report V2 lottery rules on PROTOCOL_VERSION (#1643)."""
import asyncio
import re
from pathlib import Path
import tempfile
import unittest

from controller import Controller
from lottery import PROTOCOL_VERSION, check_lottery_rules
from runtime import Gate, HOSTS, Journal

V2 = {'lotteryRules': 2, 'protocolVersion': PROTOCOL_VERSION, 'height': 5}
DISCOVERY_RS = Path(__file__).resolve().parents[2] / 'botho/src/network/discovery.rs'


class FakeRpc:
    def __init__(self, infos):
        self.infos, self.calls = infos, []

    async def call(self, host, method, params=None, **kwargs):
        self.calls.append((host, method))
        assert method == 'getChainInfo', method
        return self.infos.get(host, V2)


class CheckLotteryRulesTests(unittest.TestCase):
    def test_accepts_v2_on_current_protocol(self):
        self.assertEqual(check_lottery_rules(dict(V2), 'h'),
                         {'lotteryRules': 2, 'protocolVersion': PROTOCOL_VERSION})

    def test_pin_matches_node_protocol_version(self):
        # #1933: 8.0.0 and 9.0.0 bumped the node without the harness, and
        # every campaign then held at this preflight.
        source = DISCOVERY_RS.read_text()
        node = re.search(r'^pub const PROTOCOL_VERSION: &str = "([^"]+)";$', source, re.M)
        self.assertIsNotNone(node, 'PROTOCOL_VERSION not found in ' + str(DISCOVERY_RS))
        self.assertEqual(PROTOCOL_VERSION, node.group(1))

    def test_rejects_v1_and_malformed_rules(self):
        for rules in (1, None, '2', 2.0, True, 3):
            with self.subTest(rules=rules):
                info = dict(V2, lotteryRules=rules)
                with self.assertRaisesRegex(Gate, 'V2 lottery rules'):
                    check_lottery_rules(info, 'h')
        info = dict(V2)
        del info['lotteryRules']  # a pre-P4 node omits the field
        with self.assertRaisesRegex(Gate, 'V2 lottery rules'):
            check_lottery_rules(info, 'h')

    def test_rejects_other_protocol_versions(self):
        for version in ('6.0.0', '7.0.0', '8.0.0', '9.0.0', '10.0.1', None, 10):
            with self.subTest(version=version):
                with self.assertRaisesRegex(Gate, 'protocol is not ' + re.escape(PROTOCOL_VERSION)):
                    check_lottery_rules(dict(V2, protocolVersion=version), 'h')
        with self.assertRaisesRegex(Gate, 'malformed chain info'):
            check_lottery_rules(None, 'h')


class ControllerPreflightTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.c = Controller.__new__(Controller)
        self.c.state = Path(self.tmp.name)
        self.c.j = Journal(self.c.state / 'journal.sqlite', lambda: 1000)
        self.addCleanup(self.c.j.db.close)

    def test_every_node_v2_is_recorded_once(self):
        self.c.rpc = FakeRpc({})
        asyncio.run(self.c.lottery_preflight())
        self.assertEqual(sorted(h for h, _ in self.c.rpc.calls), sorted(HOSTS))
        self.assertEqual(set(self.c.j.get('lottery_preflight')['hosts']), set(HOSTS))
        asyncio.run(self.c.lottery_preflight())
        self.assertEqual(len(self.c.rpc.calls), len(HOSTS))

    def test_one_v1_node_refuses_the_start(self):
        self.c.rpc = FakeRpc({HOSTS[3]: dict(V2, lotteryRules=1)})
        with self.assertRaisesRegex(Gate, 'V2 lottery rules.*' + HOSTS[3]):
            asyncio.run(self.c.run())
        # Refused before the campaign started: nothing written.
        self.assertIsNone(self.c.j.get('lottery_preflight'))
        self.assertIsNone(self.c.j.get('status'))
        self.assertIsNone(self.c.j.get('setup_start'))

    def test_old_protocol_node_refuses_the_start(self):
        self.c.rpc = FakeRpc({HOSTS[0]: dict(V2, protocolVersion='6.0.0')})
        with self.assertRaisesRegex(Gate, 'protocol is not ' + re.escape(PROTOCOL_VERSION)):
            asyncio.run(self.c.run())
        self.assertIsNone(self.c.j.get('status'))


if __name__ == '__main__':
    unittest.main()
