"""Offline staging and observer tests; no SSH, services or credentials are used."""
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import types
import unittest
from unittest.mock import Mock, patch

import observer
import staging
from runtime import Gate, HOSTS
from test_targets import targets


class StagingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        def file(name,data):
            path=self.root/name;path.write_bytes(data);path.chmod(0o600);return str(path)
        plan=json.loads(Path(__file__).with_name('testnet-72h-plan.json').read_text())
        rows=targets()
        for row in rows:row['observer_ssh_target']=row['observer_ssh_target'].replace('ubuntu@','botho@')
        plan['endpoints']=[r['rpc_url'] for r in rows]
        run='campaign-test'
        self.p={'schema_version':1,'run_id':run,'plan_file':file('plan.json',json.dumps(plan).encode()),
                'signer':file('signer',b'fake executable'),'adapter_source':'a'*40,'node_sha256':'b'*64,
                'setup_start':1000,'end':1000+81*3600,'targets':rows,
                'operator_identity':file('operator-key',b'operator key must never ship'),
                'operator_known_hosts':file('operator-known',b'operator trust must never ship'),
                'observer_key':file('observer-key',b'restricted key'),
                'observer_public_key':file('observer-pub',b'ssh-ed25519 AAAA test'),
                'observer_known_hosts':file('known-hosts',b'only known new hosts'),
                'hosts_file':file('hosts',('\n'.join(f'10.0.0.{i+1} node{i+1}.fresh.internal' for i in range(5))).encode()),
                'controller_ip':'10.0.0.10',
                'wallets':[{'address':str(i),'key':file(f'w{i}',f'new wallet {i}'.encode())} for i in range(8)],
                'controller':{'deploy_ssh_target':'ubuntu@client.internal','user':'botho-client',
                              'state':'/var/lib/'+run,'package':'/opt/'+run,'unit':run+'.service'},
                'observers':[]}
        for i,role in enumerate(HOSTS):
            self.p['observers'].append({'role':role,'deploy_ssh_target':f'ubuntu@node{i+1}.fresh.internal',
                'user':'botho','state':'/var/lib/'+run+'-observer','package':'/opt/'+run+'-observer',
                'unit':run+'-observer.service','export_wrapper':'/usr/local/libexec/botho72-export',
                'authorized_keys_file':'/etc/ssh/authorized_keys/botho',
                'node':{'node_unit':'botho-fresh.service','binary_path':'/usr/local/bin/botho-fresh',
                        'data_dir':'/var/lib/fresh-node','config_path':'/var/lib/fresh-node/config.toml',
                        'node_key_path':'/var/lib/fresh-node/node_key'}})
        self.profile=self.root/'profile.json';self.profile.write_text(json.dumps(self.p))

    def test_prepare_pins_new_targets_private_inputs_and_root_owned_control_paths(self):
        with patch('staging.subprocess.run') as command:
            bundles=staging.prepare(self.p)
        command.assert_not_called()
        self.assertEqual(len(bundles),6)
        files=bundles[0][1]
        launch=json.loads(files['state/launch.json'][0])
        self.assertEqual(launch['targets'],self.p['targets'])
        self.assertEqual(launch['node_sha256'],'b'*64)
        self.assertEqual(launch['known_hosts_sha256'],hashlib.sha256(b'only known new hosts').hexdigest())
        self.assertNotIn(b'operator key must never ship',b''.join(data for data,_ in files.values()))
        for item,files,is_observer in bundles[1:]:
            config=json.loads(files['state/config.json'][0])
            self.assertEqual(config['node_unit'],'botho-fresh.service')
            self.assertEqual(config['end'],self.p['end'])
            self.assertIn(b'command="/usr/local/libexec/botho72-export"',files['authorized_keys_file'][0])
            self.assertIn(b'from="10.0.0.10"',files['authorized_keys_file'][0])
            self.assertIn(b' export --state /var/lib/campaign-test-observer',files['export_wrapper'][0])
        unit=staging.unit_text(bundles[0][0])
        self.assertIn('User=botho-client',unit)
        self.assertIn('MemorySwapMax=0',unit)
        self.assertIn('/opt/campaign-test/hosts:/etc/hosts',unit)
        self.assertNotIn('loom-worker',unit)

    def test_invalid_profile_has_no_output_or_network_side_effects(self):
        self.p['targets'].pop()
        self.profile.write_text(json.dumps(self.p))
        output=self.root/'not-created'
        with patch('staging.subprocess.run') as command:
            with self.assertRaises(Gate):staging.stage(self.profile,output,execute=True)
        self.assertFalse(output.exists())
        command.assert_not_called()

    def test_offline_stage_is_exclusive_and_does_not_execute(self):
        output=self.root/'prepared'
        with patch('staging.subprocess.run') as command:
            results=staging.stage(self.profile,output)
        command.assert_not_called()
        self.assertEqual(len(results),6)
        self.assertEqual(output.stat().st_mode & 0o777,0o700)
        for row in results:
            archive=Path(row['archive'])
            self.assertEqual(archive.stat().st_mode & 0o777,0o600)
            with tarfile.open(archive) as tar:
                meta=json.load(tar.extractfile('install.json'))
                self.assertEqual(meta['item']['deploy_ssh_target'],row['target'])
        with self.assertRaises(FileExistsError):staging.stage(self.profile,output)

    def test_remote_installer_refuses_existing_state_before_any_write_or_reload(self):
        output=self.root/'prepared'
        results=staging.stage(self.profile,output)
        archive=Path(results[0]['archive'])
        with tarfile.open(archive) as tar:
            entries={m.name:tar.extractfile(m).read() for m in tar.getmembers()}
        meta=json.loads(entries['install.json'])
        state=self.root/'existing-state';state.mkdir();(state/'keep').write_text('preserved')
        meta['item']['state']=str(state)
        meta['item']['package']=str(self.root/'new-package')
        entries['install.json']=json.dumps(meta).encode()
        fake=self.root/'existing-test.tar'
        with tarfile.open(fake,'w') as tar:
            for name,data in entries.items():
                info=tarfile.TarInfo(name);info.size=len(data);tar.addfile(info,io.BytesIO(data))
        env={'ARCHIVE':str(fake),'SHA':hashlib.sha256(fake.read_bytes()).hexdigest()}
        account=types.SimpleNamespace(pw_uid=os.getuid(),pw_gid=os.getgid())
        with patch('pwd.getpwnam',return_value=account),patch('subprocess.run') as command:
            with self.assertRaisesRegex(AssertionError,'destination exists'):
                exec(staging.INSTALL,env)
        command.assert_not_called()
        self.assertEqual((state/'keep').read_text(),'preserved')
        self.assertFalse((self.root/'new-package').exists())

    def test_observer_uses_configured_unit_and_paths(self):
        node=self.p['observers'][0]['node']
        def read_text(path):
            if str(path)=='/proc/321/status':return 'VmRSS: 20 kB\nVmSwap: 0 kB\n'
            if str(path)=='/proc/meminfo':return 'MemAvailable: 1000 kB\nMemTotal: 2000 kB\n'
            return 'some=0'
        fields='MainPID=321\nNRestarts=0\nExecMainStartTimestampMonotonic=123\nMemoryPeak=4096\n'
        with patch('observer.subprocess.check_output',return_value=fields) as command, \
             patch('observer.Path.read_text',read_text), patch('observer.Path.read_bytes',return_value=b'newconfig') as read, \
             patch('observer.shutil.disk_usage',return_value=types.SimpleNamespace(free=99,total=100)) as disk:
            result=observer.sample('binaryhash',observer.node_config(node))
        self.assertNotIn('error',result)
        self.assertEqual(command.call_args.args[0][2],'botho-fresh.service')
        disk.assert_called_once_with('/var/lib/fresh-node')
        self.assertEqual(result['config'],hashlib.sha256(b'newconfig').hexdigest())
        with self.assertRaises(Gate):observer.node_config({'node_unit':'only-one-field.service'})
        self.assertEqual(observer.node_config({'end':10}),observer.DEFAULT_NODE)

    def test_installer_under_private_umask_keeps_package_traversable_and_state_private(self):
        results=staging.stage(self.profile,self.root/'prepared')
        archive=Path(results[0]['archive'])
        with tarfile.open(archive) as tar:
            entries={m.name:tar.extractfile(m).read() for m in tar.getmembers()}
        meta=json.loads(entries['install.json'])
        package=self.root.resolve()/'opt'/'campaign-test'
        state=self.root.resolve()/'var'/'campaign-test'
        units=self.root.resolve()/'systemd';units.mkdir()
        meta['item'].update(package=str(package),state=str(state))
        entries['install.json']=json.dumps(meta).encode()
        with tarfile.open(archive,'w') as tar:
            for name,data in entries.items():
                info=tarfile.TarInfo(name);info.size=len(data);tar.addfile(info,io.BytesIO(data))
        env={'ARCHIVE':str(archive),'SHA':hashlib.sha256(archive.read_bytes()).hexdigest()}
        installer=staging.INSTALL.replace("pathlib.Path('/etc/systemd/system')",'pathlib.Path('+repr(str(units))+')')
        account=types.SimpleNamespace(pw_uid=12345,pw_gid=12345)
        real_stat=Path.stat
        def root_owned_parent(path,*args,**kwargs):
            # Model protected root-owned system parents without requiring root.
            fields=list(real_stat(path,*args,**kwargs));fields[4]=0;fields[0]&=~0o022
            return os.stat_result(fields)
        def systemctl(args,**kwargs):
            output={'show':'LoadState=not-found','is-active':'inactive','is-enabled':'static'}
            return types.SimpleNamespace(stdout=output.get(args[1],''),returncode=0)
        inherited=os.umask(0o077)
        try:
            with patch('pwd.getpwnam',return_value=account),patch('os.chown') as chown, \
                 patch.object(Path,'stat',root_owned_parent),patch('subprocess.run',side_effect=systemctl) as command, \
                 patch('builtins.print'):
                exec(installer,env)
        finally:
            os.umask(inherited)
        # The controller is not the root owner of package paths: other-user
        # read/search bits must permit traversal to its source and executable.
        for path in (package.parent,package):
            self.assertEqual(path.stat().st_mode & 0o777,0o755,str(path))
        for path in package.rglob('*'):
            self.assertEqual(path.stat().st_mode & 0o004,0o004,str(path))
        self.assertEqual((package/'botho-stress-wallet').stat().st_mode & 0o777,0o755)
        for path in (state,*state.rglob('*')):
            self.assertEqual(path.stat().st_mode & 0o777,0o700 if path.is_dir() else 0o600,str(path))
        self.assertIn(unittest.mock.call(state,12345,12345),chown.call_args_list)
        self.assertEqual([call.args[0][1] for call in command.call_args_list],
                         ['show','daemon-reload','is-active','is-enabled'])
        self.assertFalse(archive.exists())
