"""Prepare and optionally install a fresh campaign INACTIVE; never activates units.

All profile validation and local artifact reads complete before SSH. Operators
supply fresh wallets/accounts and an independent absolute host shutdown policy.
"""
import hashlib
import io
import ipaddress
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tarfile

from observer import absolute_path, node_config
from plan import expand
from runtime import Gate, HOSTS, campaign_targets, campaign_tls_context, validate_ssh_target

SOURCE = Path(__file__).parent
CODE = ('controller.py', 'runtime.py', 'plan.py', 'bounds.py', 'discovery.py', 'discovery_plan.py', 'fee_evidence.py', 'selection.py', 'reporting.py')


def need(condition, message):
    if not condition:
        raise Gate(message)


def read_file(path, private=False):
    path = Path(absolute_path(path))
    info = path.lstat()
    need(stat.S_ISREG(info.st_mode), 'input must be a regular file, not a symlink')
    if private:
        need(info.st_mode & 0o077 == 0, 'private input must be owner-only')
    return path.read_bytes()


def user(value):
    need(isinstance(value, str) and re.fullmatch(r'[a-z_][a-z0-9_-]*', value), 'invalid service user')
    need(value != 'root', 'campaign services require an unprivileged account')
    return value


def component(value, run_id):
    need(isinstance(value, dict), 'deployment component required')
    validate_ssh_target(value['deploy_ssh_target'])
    user(value['user'])
    for field in ('state', 'package'):
        absolute_path(value[field])
        need(run_id in Path(value[field]).name, 'fresh leaf paths must include run_id')
    need(value['state'] != value['package'] and Path(value['state']) not in Path(value['package']).parents
         and Path(value['package']) not in Path(value['state']).parents, 'state and package must not overlap')
    need(value['package'].startswith(('/opt/', '/usr/local/lib/')), 'package must be outside user homes')
    need(re.fullmatch(r'[a-zA-Z0-9_-][a-zA-Z0-9_.-]*\.service', value['unit'])
         and run_id in value['unit'], 'fresh service name must include run_id')


def unit_text(item, observer=False):
    package, state = item['package'], item['state']
    command = (f'{package}/observer.py run --state {state}' if observer else
               f'{package}/controller.py run --state {state}')
    return f'''[Unit]
Description=Bounded isolated Botho {'observer' if observer else 'campaign'}
After=network-online.target
[Service]
Type=simple
User={item['user']}
Group={item['user']}
UMask=0077
ExecStart=/usr/bin/python3 {command}
Restart=on-failure
RestartSec=5
RuntimeMaxSec=295200
CPUQuota={'5%' if observer else '100%'}
MemoryMax={'64M' if observer else '1G'}
MemorySwapMax=0
TasksMax=64
KillMode=control-group
LimitCORE=0
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome={'read-only' if observer else 'tmpfs'}
PrivateTmp=yes
BindReadOnlyPaths={package}{'' if observer else ' '+package+'/hosts:/etc/hosts'}
BindPaths={state}
ReadWritePaths={state}
UnsetEnvironment=SSH_AUTH_SOCK SSH_AGENT_PID
'''


def prepare(profile):
    """Return sealed component bundles in memory; no files, SSH or units written."""
    need(profile.get('schema_version') == 1, 'unsupported deployment profile')
    run_id = profile['run_id']
    need(isinstance(run_id, str) and re.fullmatch(r'[a-z][a-z0-9-]{7,63}', run_id), 'invalid run_id')
    plan = json.loads(read_file(profile['plan_file']))
    expand(plan)
    mapping = campaign_targets({'targets': profile['targets']}, plan)
    need(all(t['rpc_url'] != 'https://'+role+'/rpc' for role,t in mapping.items()),
         'fresh profile must not target historical public ingresses')
    for field in ('adapter_source', 'node_sha256'):
        need(re.fullmatch(r'[0-9a-f]{'+('40' if field=='adapter_source' else '64')+'}', profile[field]), 'invalid source/artifact pin')
    need(type(profile['setup_start']) in (int,float) and type(profile['end']) in (int,float)
         and 0 < profile['setup_start'] < profile['end']
         and 80.5*3600 <= profile['end']-profile['setup_start'] <= 82*3600,
         'fixed lifetime must cover setup, 72 hours and drain within 82 hours')
    controller = profile['controller']
    component(controller, run_id)
    need(controller['deploy_ssh_target'] != 'loom-worker-1', 'dedicated deployment target required')
    observers = profile['observers']
    need(len(observers)==5 and [o['role'] for o in observers]==list(HOSTS), 'five ordered observer deployments required')
    for item in observers:
        component(item, run_id)
        node_config(item['node'])
        need(set(item['node']) == {'node_unit','binary_path','data_dir','config_path','node_key_path'}, 'complete explicit observer node paths required')
        need(mapping[item['role']]['observer_ssh_target'].split('@')[0] == item['user'], 'observer account differs from target')
        for field in ('export_wrapper','authorized_keys_file'):
            absolute_path(item[field])
        need(item['export_wrapper'].startswith('/usr/local/libexec/') and item['authorized_keys_file'].startswith('/etc/ssh/authorized_keys/'), 'observer control files require root-owned locations')
    need(len({o['deploy_ssh_target'] for o in observers})==5, 'five distinct node deployment targets required')
    # All operator access remains local. These are never included in any bundle.
    read_file(profile['operator_identity'], private=True)
    read_file(profile['operator_known_hosts'])
    public_key = read_file(profile['observer_public_key']).decode().strip()
    need(re.fullmatch(r'ssh-ed25519 [A-Za-z0-9+/]+={0,2}(?: [^\r\n]*)?', public_key), 'one Ed25519 observer public key required')
    observer_key = read_file(profile['observer_key'], private=True)
    known_hosts = read_file(profile['observer_known_hosts'])
    hosts = read_file(profile['hosts_file'])
    # Unit-scoped hosts must resolve every configured RPC/SSH hostname privately.
    resolved = {}
    for line in hosts.decode().splitlines():
        parts = line.split('#',1)[0].split()
        if not parts:
            continue
        address = ipaddress.ip_address(parts[0])
        need(address.is_private, 'campaign hosts file requires private addresses')
        for name in parts[1:]:
            need(name not in resolved, 'duplicate hosts-file hostname')
            resolved[name] = str(address)
    from urllib.parse import urlsplit
    for target in mapping.values():
        for name in (urlsplit(target['rpc_url']).hostname, target['observer_ssh_target'].split('@')[1]):
            need(name in resolved, 'target absent from unit-scoped hosts file')
    need(ipaddress.ip_address(profile['controller_ip']).is_private, 'private controller address required')
    signer = read_file(profile['signer'])
    wallets = profile['wallets']
    wallet_count = plan['wallets']['count']
    need(len(wallets)==wallet_count and len({w['address'] for w in wallets})==wallet_count and len({w['key'] for w in wallets})==wallet_count, 'manifest requires distinct wallet addresses and files')
    wallet_files = [read_file(w['key'], private=True) for w in wallets]
    need(len(set(wallet_files))==wallet_count, 'wallet files must be distinct')
    code = {name: (SOURCE/name).read_bytes() for name in CODE}
    package, state = controller['package'], controller['state']
    launch = {'run_id':run_id, 'setup_start':profile['setup_start'], 'infrastructure_end':profile['end'],
              'targets':profile['targets'], 'signer':package+'/botho-stress-wallet',
              'signer_sha256':hashlib.sha256(signer).hexdigest(), 'adapter_source':profile['adapter_source'],
              'node_sha256':profile['node_sha256'], 'controller_files':{n:hashlib.sha256(b).hexdigest() for n,b in code.items()},
              'observer_key':state+'/observer_key', 'known_hosts':package+'/known_hosts',
              'known_hosts_sha256':hashlib.sha256(known_hosts).hexdigest(),
              'hosts_file':package+'/hosts', 'hosts_sha256':hashlib.sha256(hosts).hexdigest(),
              'wallets':[{'address':w['address'], 'key':state+f'/wallets/wallet-{i}.mnemonic'} for i,w in enumerate(wallets)]}
    if plan.get('schema_version') == 2:
        need(profile.get('isolated_discovery') is True, 'explicit isolated discovery opt-in required')
        balances = profile.get('opening_balances')
        need(isinstance(balances,list) and len(balances)==wallet_count and all(type(x) is int and x>0 for x in balances), 'exact opening balances required')
        need(sum(balances)<=int(plan['funding']['max_principal_picocredits']), 'opening principal cap')
        launch.update(isolated_discovery=True, opening_balances=balances)
    files = {'package/'+n:(b,0o644) for n,b in code.items()}
    files.update({'package/botho-stress-wallet':(signer,0o755), 'package/hosts':(hosts,0o644),
                  'package/known_hosts':(known_hosts,0o644), 'state/observer_key':(observer_key,0o600),
                  'state/plan.json':(json.dumps(plan).encode(),0o600)})
    if 'tls_ca_file' in profile or 'tls_ca_sha256' in profile:
        campaign_tls_context(profile)  # Checks digest and parses CA; verification never disabled.
        ca = read_file(profile['tls_ca_file'])
        launch.update(tls_ca_file=package+'/ca.pem', tls_ca_sha256=profile['tls_ca_sha256'])
        files['package/ca.pem'] = (ca,0o644)
    for i,data in enumerate(wallet_files):
        files[f'state/wallets/wallet-{i}.mnemonic'] = (data,0o600)
    files['state/launch.json'] = (json.dumps(launch).encode(),0o600)
    bundles = [(controller, files, False)]
    for item in observers:
        config = dict(item['node'], end=profile['end'])
        wrapper = f"#!/bin/sh\nexec /usr/bin/python3 {item['package']}/observer.py export --state {item['state']}\n"
        auth = f'restrict,from="{profile["controller_ip"]}",command="{item["export_wrapper"]}" {public_key}\n'
        files = {'package/observer.py':((SOURCE/'observer.py').read_bytes(),0o644),
                 'package/runtime.py':(code['runtime.py'],0o644),
                 'state/config.json':(json.dumps(config).encode(),0o600),
                 'export_wrapper':(wrapper.encode(),0o755), 'authorized_keys_file':(auth.encode(),0o644)}
        bundles.append((item,files,True))
    return bundles


# Installed through sudo with a digest-checked in-memory archive. No shell, tar
# extraction paths, service activation, existing-file replacement or migration.
INSTALL = '''import hashlib,io,json,os,pathlib,pwd,subprocess,tarfile
os.umask(0o022)
archive=pathlib.Path(ARCHIVE)
data=archive.read_bytes()
assert hashlib.sha256(data).hexdigest()==SHA
with tarfile.open(fileobj=io.BytesIO(data)) as tar:
 meta=json.load(tar.extractfile('install.json'))
 item=meta['item']; account=pwd.getpwnam(item['user'])
 state=pathlib.Path(item['state']);package=pathlib.Path(item['package'])
 unit=pathlib.Path('/etc/systemd/system')/item['unit']
 destinations={n:(state/n[6:] if n.startswith('state/') else package/n[8:] if n.startswith('package/') else pathlib.Path(item[n])) for n in meta['files']}
 for path in [state,package,unit,*[p for n,p in destinations.items() if not n.startswith(('state/','package/'))]]:
  assert not path.exists() and not path.is_symlink(), 'destination exists: '+str(path)
  assert not any(p.is_symlink() for p in path.parents), 'symlink parent'
 for path in [package,unit,*[p for n,p in destinations.items() if not n.startswith(('state/','package/'))]]:
  for parent in path.parents:
   if parent.exists():assert parent.stat().st_uid==0 and parent.stat().st_mode & 0o022==0, 'control parent writable by non-root'
 status=subprocess.run(['systemctl','show',item['unit'],'-p','LoadState'],capture_output=True,text=True)
 assert status.stdout.strip()=='LoadState=not-found', 'service name already exists'
 package.mkdir(parents=True,mode=0o755)
 state.mkdir(parents=True,mode=0o700);os.chown(state,account.pw_uid,account.pw_gid)
 for name,mode in meta['files'].items():
  path=destinations[name];path.parent.mkdir(parents=True,exist_ok=True)
  if name.startswith('state/'):
   for parent in path.parents:
    if parent==state.parent:break
    os.chmod(parent,0o700);os.chown(parent,account.pw_uid,account.pw_gid)
  with path.open('xb') as output:output.write(tar.extractfile(name).read());output.flush();os.fsync(output.fileno())
  os.chmod(path,mode)
  if name.startswith('state/'):os.chown(path,account.pw_uid,account.pw_gid)
 for name in ('artifacts','probes','reports') if not meta['observer'] else ():
  path=state/name;path.mkdir(mode=0o700);os.chown(path,account.pw_uid,account.pw_gid)
 with unit.open('x') as output:output.write(meta['unit']);output.flush();os.fsync(output.fileno())
 os.chmod(unit,0o644)
subprocess.run(['systemctl','daemon-reload'],check=True)
result=subprocess.run(['systemctl','is-active',item['unit']],capture_output=True,text=True)
assert result.stdout.strip() in ('inactive','unknown'), 'staged unit unexpectedly active'
enabled=subprocess.run(['systemctl','is-enabled',item['unit']],capture_output=True,text=True)
assert enabled.stdout.strip() in ('disabled','static'), 'staged unit unexpectedly enabled'
archive.unlink()
print(json.dumps({'unit':item['unit'],'status':'staged-inactive','archive_sha256':SHA}))
'''


def stage(profile_path, output, execute=False):
    profile = json.loads(Path(profile_path).read_text())
    bundles = prepare(profile)
    output = Path(output)
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    results = []
    for index,(item,files,observer) in enumerate(bundles):
        archive = output/f'{profile["run_id"]}-{index}.tar'
        with archive.open('xb') as stream:
            os.chmod(archive,0o600)
            with tarfile.open(fileobj=stream,mode='w') as tar:
                meta = {'item':item,'observer':observer,'files':{n:m for n,(_,m) in files.items()},'unit':unit_text(item,observer)}
                for name,(data,mode) in dict(files, **{'install.json':(json.dumps(meta).encode(),0o600)}).items():
                    info=tarfile.TarInfo(name);info.size=len(data);info.mode=mode
                    tar.addfile(info,io.BytesIO(data))
        sha=hashlib.sha256(archive.read_bytes()).hexdigest()
        results.append({'archive':str(archive),'sha256':sha,'target':item['deploy_ssh_target'],'unit':item['unit']})
        if execute:
            options=['-F','/dev/null','-i',profile['operator_identity'],'-o','IdentitiesOnly=yes','-o','BatchMode=yes',
                     '-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+profile['operator_known_hosts'],'-o','ConnectTimeout=15']
            operator=item['deploy_ssh_target'].split('@')[0]
            remote=('/root' if operator=='root' else '/home/'+operator)+'/.'+archive.name
            subprocess.run(['ssh',*options,item['deploy_ssh_target'],'test','!','-e',remote],check=True,timeout=30)
            subprocess.run(['scp',*options,str(archive),item['deploy_ssh_target']+':'+remote],check=True,timeout=90)
            script='ARCHIVE='+repr(remote)+'\nSHA='+repr(sha)+'\n'+INSTALL
            subprocess.run(['ssh',*options,item['deploy_ssh_target'],'sudo','-n','python3','-'],input=script.encode(),check=True,timeout=90)
    (output/'staging-manifest.json').write_text(json.dumps(results,indent=2)+'\n')
    os.chmod(output/'staging-manifest.json',0o600)
    return results
