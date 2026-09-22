#!/usr/bin/env python3
"""Install a versioned experiment using the operator's LOCAL SSH access only.

Explicit commands: observers (read-only collection) and controller (activation).
Private materials are staged in a protected directory outside the repository.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import tarfile
import time
from runtime import HOSTS

SOURCE=Path(__file__).parent
LOCAL=Path.home()/'.local/state/botho-stress-1404'
STATE='/home/ubuntu/.local/share/botho-stress-1404'
OBS='/var/lib/botho-stress-observer-1404'


def ssh(host, script, node=False, output=True):
    args=['ssh','-o','BatchMode=yes','-o','StrictHostKeyChecking=yes','-o','ConnectTimeout=15']
    if node:args+=['-i',str(Path.home()/'.ssh/botho-nodes.pem')]
    args += [('ubuntu@'+host) if node else host,'bash','-s']
    result=subprocess.run(args,input=script.encode(),stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=90)
    if result.returncode:
        raise RuntimeError(host+': '+result.stderr.decode()[-2000:])
    if output:print(host,result.stdout.decode().strip())
    return result.stdout


def upload(host, archive, node=False):
    args=['scp','-q','-o','BatchMode=yes','-o','StrictHostKeyChecking=yes','-o','ConnectTimeout=15']
    if node:args+=['-i',str(Path.home()/'.ssh/botho-nodes.pem')]
    args += [str(archive),(('ubuntu@'+host) if node else host)+':/home/ubuntu/'+archive.name]
    subprocess.run(args,check=True,timeout=90)


def tar_add(archive, name, data, mode=0o600):
    info=tarfile.TarInfo(name)
    info.size=len(data); info.mode=mode
    archive.addfile(info,io.BytesIO(data))


def observers():
    end=time.time()+82*3600
    key=(LOCAL/'observer_key.pub').read_text().strip()
    restriction='restrict,command="/usr/bin/python3 /opt/botho-stress-observer-1404/observer.py export" '+key
    archive=LOCAL/'observer-package.tar'
    with tarfile.open(archive,'w') as tar:
        for name in ('observer.py','runtime.py'):tar_add(tar,name,(SOURCE/name).read_bytes(),0o644)
    unit='''[Unit]
Description=Bounded Botho stress resource observer 1404
After=network.target
[Service]
Type=simple
User=ubuntu
Group=ubuntu
UMask=0077
ExecStart=/usr/bin/python3 /opt/botho-stress-observer-1404/observer.py run
Restart=on-failure
RestartSec=10
RuntimeMaxSec=295200
CPUQuota=5%
MemoryMax=64M
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=read-only
PrivateTmp=yes
ReadWritePaths=/var/lib/botho-stress-observer-1404
[Install]
WantedBy=multi-user.target
'''
    for host in HOSTS:
        upload(host,archive,node=True)
        script=f'''set -euo pipefail
sudo install -d -m 755 /opt/botho-stress-observer-1404
sudo tar -xf /home/ubuntu/observer-package.tar -C /opt/botho-stress-observer-1404
sudo chown -R root:root /opt/botho-stress-observer-1404
sudo install -d -o ubuntu -g ubuntu -m 700 {OBS}
python3 - <<'PY'
from pathlib import Path
import json,os
os.umask(0o077)
config=Path({(OBS+'/config.json')!r})
if not config.exists():config.write_text(json.dumps({{'end':{end!r}}}))
auth=Path.home()/'.ssh/authorized_keys'
line={restriction!r}
text=auth.read_text() if auth.exists() else ''
if line not in text:
    with auth.open('a') as output:output.write(chr(10)+line+chr(10))
PY
sudo tee /etc/systemd/system/botho-stress-observer-1404.service >/dev/null <<'UNIT'
{unit}UNIT
sudo systemctl daemon-reload
sudo systemctl enable botho-stress-observer-1404.service
sudo systemctl restart botho-stress-observer-1404.service
rm /home/ubuntu/observer-package.tar
systemctl is-active botho-stress-observer-1404.service
'''
        ssh(host,script,node=True)
    known=[]
    for host in HOSTS:
        result=subprocess.run(['ssh-keygen','-F',host,'-f',str(Path.home()/'.ssh/known_hosts')],
                              check=True,stdout=subprocess.PIPE)
        known.extend(line for line in result.stdout.decode().splitlines() if not line.startswith('#'))
    (LOCAL/'known_hosts').write_text('\n'.join(known)+'\n')
    (LOCAL/'observers.json').write_text(json.dumps({'installed_at':time.time(),'end':end}))


def controller(binary,source_commit):
    binary=Path(binary)
    sha=hashlib.sha256(binary.read_bytes()).hexdigest()
    # Unique package, read-only inside the service's private mount namespace.
    package='/home/ubuntu/.local/lib/botho-stress-1404-'+source_commit[:12]
    start=time.time()+60
    config={'run_id':'stress-1404-'+time.strftime('%Y%m%dT%H%M%SZ',time.gmtime(start)),
        'setup_start':start,'signer':package+'/botho-stress-wallet','signer_sha256':sha,
        'adapter_source':source_commit,'node_sha256':'88c169423fdc003a5dbc6d688852fdbcda26b78f85cd371de29d9192571c674f',
        'controller_files':{name:hashlib.sha256((SOURCE/name).read_bytes()).hexdigest()
                            for name in ('controller.py','runtime.py','plan.py')},
        'observer_key':STATE+'/observer_key','known_hosts':STATE+'/known_hosts',
        'wallets':[{'address':address,'key':STATE+f'/wallets/wallet-{i}.mnemonic'}
            for i,address in enumerate(json.loads((LOCAL/'addresses.json').read_text()))]}
    (LOCAL/'launch.json').write_text(json.dumps(config,indent=2)+'\n')
    archive=LOCAL/'controller-package.tar'
    with tarfile.open(archive,'w') as tar:
        for name in ('controller.py','runtime.py','plan.py'):
            tar_add(tar,'code/'+name,(SOURCE/name).read_bytes(),0o644)
        tar_add(tar,'code/botho-stress-wallet',binary.read_bytes(),0o755)
        tar_add(tar,'state/plan.json',(SOURCE/'testnet-72h-plan.json').read_bytes())
        for name in ('launch.json','observer_key','known_hosts'):
            tar_add(tar,'state/'+name,(LOCAL/name).read_bytes())
        for path in (LOCAL/'wallets').glob('wallet-*.mnemonic'):
            tar_add(tar,'state/wallets/'+path.name,path.read_bytes())
    unit=f'''[Unit]
Description=Bounded Botho native stress campaign 1404
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=600
StartLimitBurst=3
[Service]
Type=simple
UMask=0077
ExecStart=/usr/bin/python3 {package}/controller.py run --state {STATE}
Restart=on-failure
RestartSec=5
RuntimeMaxSec=295200
CPUQuota=100%
MemoryMax=1G
TasksMax=64
LimitCORE=0
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=tmpfs
PrivateUsers=yes
PrivateTmp=yes
BindReadOnlyPaths={package}
BindPaths={STATE}
ReadWritePaths={STATE}
UnsetEnvironment=SSH_AUTH_SOCK SSH_AGENT_PID
[Install]
WantedBy=default.target
'''
    (LOCAL/'controller.service').write_text(unit)
    upload('loom-worker-1',archive)
    script=f'''set -euo pipefail
umask 077
if test -e {STATE}/journal.sqlite; then
    echo 'Existing experiment journal: refuse overwrite' >&2
    exit 1
fi
install -d -m 700 {STATE}-staging
tar -xf /home/ubuntu/controller-package.tar -C {STATE}-staging
install -d -m 700 {package}
cp -a {STATE}-staging/code/. {package}/
install -d -m 700 {STATE}
cp -a {STATE}-staging/state/. {STATE}/
install -d -m 700 {STATE}/wallets {STATE}/artifacts {STATE}/probes {STATE}/reports
chmod 700 {STATE}/wallets
rm -rf {STATE}-staging
rm /home/ubuntu/controller-package.tar
install -d -m 700 /home/ubuntu/.config/systemd/user
cat > /home/ubuntu/.config/systemd/user/botho-stress-1404.service <<'UNIT'
{unit}UNIT
systemctl --user daemon-reload
systemctl --user enable --now botho-stress-1404.service
systemctl --user is-active botho-stress-1404.service
'''
    ssh('loom-worker-1',script)
    print(json.dumps({'run_id':config['run_id'],'setup_start':start,'signer_sha256':sha}))


if __name__=='__main__':
    os.umask(0o077)
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode',choices=['observers','controller'])
    parser.add_argument('--binary')
    parser.add_argument('--source-commit')
    args=parser.parse_args()
    if args.mode=='observers':observers()
    elif args.binary and args.source_commit:controller(args.binary,args.source_commit)
    else:parser.error('controller requires --binary and --source-commit')
