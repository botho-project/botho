"""New campaigns must use only their immutable RPC and observer destinations."""
import asyncio
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

from controller import Controller
from runtime import Gate, HOSTS


def targets():
    return [{'role': role, 'rpc_url': f'https://node{i+1}.fresh.internal/rpc',
             'observer_ssh_target': f'ubuntu@node{i+1}.fresh.internal'}
            for i, role in enumerate(HOSTS)]


class TargetTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.state = Path(self.tmp.name)
        (self.state/'signer').write_bytes(b'test signer, never executed')
        (self.state/'known-hosts').write_bytes(b'test pinned hosts')
        self.plan = json.loads(Path(__file__).with_name('testnet-72h-plan.json').read_text())
        self.plan['endpoints'] = [t['rpc_url'] for t in targets()]
        self.config = {'targets': targets(), 'controller_files': {},
                       'signer': str(self.state/'signer'),
                       'signer_sha256': hashlib.sha256((self.state/'signer').read_bytes()).hexdigest(),
                       'observer_key': str(self.state/'observer-key'), 'known_hosts': str(self.state/'known-hosts'),
                       'known_hosts_sha256': hashlib.sha256((self.state/'known-hosts').read_bytes()).hexdigest(),
                       'wallets': [{'address': str(i), 'key': str(self.state/f'wallet-{i}')} for i in range(8)]}

    def controller(self):
        (self.state/'plan.json').write_text(json.dumps(self.plan))
        (self.state/'launch.json').write_text(json.dumps(self.config))
        c = Controller(self.state)
        self.addCleanup(c.j.db.close)
        return c

    async def test_actual_rpc_and_observer_use_only_configured_targets(self):
        c = self.controller()
        response = Mock()
        response.__enter__ = Mock(return_value=response)
        response.__exit__ = Mock(return_value=False)
        response.read.return_value = b'{"jsonrpc":"2.0","id":1404,"result":{}}'
        response.headers = {}
        seen = []
        c.rpc.opener.open = Mock(side_effect=lambda request, **kw: (seen.append(request.full_url) or response))
        proc = Mock(returncode=0, communicate=AsyncMock(return_value=(b'[]', b'')))
        with patch('controller.asyncio.create_subprocess_exec', AsyncMock(return_value=proc)) as ssh:
            for role in HOSTS:
                await c.rpc.call(role, 'node_getStatus')
                await c.observer(role)
        self.assertEqual(seen, self.plan['endpoints'])
        for call, target in zip(ssh.call_args_list, targets()):
            self.assertEqual(call.args[-1], target['observer_ssh_target'])
            self.assertIn('IdentitiesOnly=yes', call.args)
            self.assertIn('StrictHostKeyChecking=yes', call.args)
            self.assertIn('UserKnownHostsFile='+self.config['known_hosts'], call.args)
            self.assertEqual(call.args[1:3], ('-F', '/dev/null'))

    async def test_new_endpoints_without_complete_mapping_fail_before_io(self):
        for value in [None, [], targets()[:4], list(reversed(targets()))]:
            with self.subTest(value=value):
                self.config['targets'] = value
                with patch('urllib.request.OpenerDirector.open') as net, patch('controller.Journal') as journal:
                    with self.assertRaises((Gate, ValueError)):
                        self.controller()
                    net.assert_not_called()
                    journal.assert_not_called()
        del self.config['targets']
        with self.assertRaises((Gate, ValueError)):
            self.controller()

    async def test_legacy_launch_keeps_original_defaults(self):
        del self.config['targets']
        self.plan['endpoints'] = ['https://'+h+'/rpc' for h in HOSTS]
        c = self.controller()
        self.assertEqual(set(c.rpc.locks), set(HOSTS))

    async def test_insecure_duplicate_credentials_and_mismatched_targets_are_rejected(self):
        from runtime import campaign_targets
        for url in ['http://node1.fresh.internal/rpc', 'https://u:p@node1.fresh.internal/rpc',
                    'https://node1.fresh.internal/rpc?q=1', 'https://node1.fresh.internal/rpc#x',
                    'https://node1.fresh.internal:65536/rpc']:
            bad = copy.deepcopy(self.config)
            bad['targets'][0]['rpc_url'] = url
            with self.subTest(url=url), self.assertRaises(Gate):
                campaign_targets(bad,self.plan)
        for mutate in [lambda rows: rows[0].update(observer_ssh_target='-oProxyCommand=bad'),
                       lambda rows: rows[0].update(observer_ssh_target=rows[1]['observer_ssh_target']),
                       lambda rows: rows[0].update(rpc_url=rows[1]['rpc_url'])]:
            bad = copy.deepcopy(self.config)
            mutate(bad['targets'])
            with self.assertRaises(Gate):
                campaign_targets(bad,self.plan)

    async def test_private_ca_is_pinned_and_tls_verification_stays_enabled(self):
        import ssl
        from runtime import campaign_tls_context
        # Use a real local trust bundle; this test never connects to a server.
        cafile = ssl.get_default_verify_paths().cafile
        if cafile is None:
            self.skipTest('platform has no PEM default trust bundle')
        data = Path(cafile).read_bytes()
        ca = self.state/'ca.pem'
        ca.write_bytes(data)
        config = {'tls_ca_file':str(ca),'tls_ca_sha256':hashlib.sha256(data).hexdigest()}
        context = campaign_tls_context(config)
        self.assertTrue(context.check_hostname)
        self.assertEqual(context.verify_mode,ssl.CERT_REQUIRED)
        for bad in [{'tls_ca_file':str(ca)}, {'tls_ca_sha256':config['tls_ca_sha256']},
                    dict(config,tls_ca_sha256='0'*64)]:
            with self.assertRaises(Gate):
                campaign_tls_context(bad)
        ca.write_bytes(b'changed CA')
        with self.assertRaises(Gate):
            campaign_tls_context(config)

    async def test_profile_and_known_hosts_cannot_change_after_journal_initialization(self):
        c = self.controller()
        c.j.db.close()
        self.config['targets'][0]['observer_ssh_target'] = 'other@node1.fresh.internal'
        with self.assertRaisesRegex(Gate,'launch configuration changed'):
            self.controller()
        self.config['targets'] = targets()
        (self.state/'known-hosts').write_bytes(b'changed key')
        with self.assertRaisesRegex(Gate,'known_hosts differs'):
            self.controller()

    async def test_explicit_profile_ignores_environment_http_proxies(self):
        import urllib.request
        with patch.dict('os.environ', {'https_proxy':'http://unlisted-proxy.invalid:8888'}):
            c = self.controller()
        self.assertFalse(any(isinstance(h,urllib.request.ProxyHandler) and h.proxies for h in c.rpc.opener.handlers))
