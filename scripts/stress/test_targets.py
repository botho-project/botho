"""New campaigns must use only their immutable RPC and observer destinations."""
import asyncio
import copy
import hashlib
import json
import os
from pathlib import Path
import sys
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
        proc = Mock(returncode=0, communicate=AsyncMock(return_value=(b'[{"at":1}]', b'')))
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

    async def test_controller_waits_for_real_observer_first_export(self):
        c = self.controller()
        c.j.set('status', 'setup')
        sample = [{'at': 1, 'pid': 123}]
        export = Path(__file__).with_name('observer.py')
        spawn = asyncio.create_subprocess_exec

        async def local_export(*args, **kwargs):
            self.assertEqual(args[-1], self.config['targets'][0]['observer_ssh_target'])
            return await spawn(sys.executable, str(export), 'export', '--state', str(self.state), **kwargs)

        def publish_first_sample(delay):
            self.assertEqual(delay, 10)
            (self.state/'latest.json').write_text(json.dumps(sample))

        with patch('controller.asyncio.create_subprocess_exec', side_effect=local_export) as ssh, \
                patch('controller.asyncio.sleep', side_effect=publish_first_sample):
            self.assertEqual(await c.observer(HOSTS[0]), sample)
        self.assertEqual(ssh.await_count, 2)
        row, = c.j.db.execute("SELECT detail FROM events WHERE state='observer_export_failed'")
        self.assertEqual(json.loads(row[0]), {
            'attempt': 1, 'host': HOSTS[0], 'returncode': os.EX_TEMPFAIL,
            'retry': True, 'stderr_tail': 'observer latest.json missing',
        })

    async def test_observer_persistent_and_midrun_failures_retain_bounded_diagnostics(self):
        c = self.controller()
        host = HOSTS[0]
        err = b'discard-this-prefix' + b'x' * 500 + b' final error'
        for status, baseline, rc, expected_attempts in [
                ('setup', None, os.EX_TEMPFAIL, 3), ('setup', {'pid': 123}, os.EX_TEMPFAIL, 1),
                ('running', None, os.EX_TEMPFAIL, 1), ('setup', None, 255, 1)]:
            with self.subTest(status=status, baseline=baseline, rc=rc):
                c.j.set('status', status)
                c.j.set('baseline:'+host, baseline)
                c.j.db.execute('DELETE FROM events')
                proc = Mock(returncode=rc, communicate=AsyncMock(return_value=(b'', err)))
                with patch('controller.asyncio.create_subprocess_exec', return_value=proc) as ssh, \
                        patch('controller.asyncio.sleep', new_callable=AsyncMock) as sleep:
                    with self.assertRaises(Gate) as caught:
                        await c.observer(host)
                self.assertIn(f'{host}, attempt {expected_attempts}, rc={rc}', str(caught.exception))
                self.assertIn('final error', str(caught.exception))
                self.assertNotIn('discard-this-prefix', str(caught.exception))
                self.assertEqual(ssh.await_count, expected_attempts)
                self.assertEqual(sleep.await_count, expected_attempts - 1)
                events = [json.loads(row[0]) for row in c.j.db.execute(
                    "SELECT detail FROM events WHERE state='observer_export_failed' ORDER BY rowid")]
                self.assertEqual([e['retry'] for e in events], [True]*(expected_attempts - 1) + [False])
                self.assertEqual(events[-1]['stderr_tail'], err[-512:].decode())

    async def test_invalid_observer_output_fails_without_retry(self):
        c = self.controller()
        c.j.set('status', 'setup')
        for out in (b'null', b'[]', b'{}', b'[1]', b'bad json', b'x' * (128*1024 + 1)):
            with self.subTest(sample=out[:10]):
                proc = Mock(returncode=0, communicate=AsyncMock(return_value=(out, b'')))
                with patch('controller.asyncio.create_subprocess_exec', return_value=proc) as ssh:
                    with self.assertRaises(Gate):
                        await c.observer(HOSTS[0])
                ssh.assert_awaited_once()

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
        data = (Path(__file__).resolve().parent / 'fixtures' / 'campaign-test-ca.pem').read_bytes()
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
