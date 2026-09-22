#!/usr/bin/env python3
"""Resource-only local observer and bounded forced-command export; makes no RPC calls."""
import collections
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from runtime import atomic

STATE = Path('/var/lib/botho-stress-observer-1404')


def sample(binary):
    at = time.time()
    try:
        unit = subprocess.check_output(['systemctl','show','botho.service','-p','MainPID',
            '-p','NRestarts','-p','ExecMainStartTimestampMonotonic','-p','MemoryPeak'],
            text=True, timeout=3)
        fields = dict(line.split('=',1) for line in unit.strip().splitlines())
        pid = int(fields['MainPID'])
        process = dict(line.split(':',1) for line in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in line)
        mem = dict(line.split(':',1) for line in Path('/proc/meminfo').read_text().splitlines())
        disk = shutil.disk_usage('/home/ubuntu/.botho')
        return {'at':at, 'pid':pid, 'start':fields['ExecMainStartTimestampMonotonic'],
                'restarts':int(fields['NRestarts']), 'binary':binary,
                'config':hashlib.sha256(Path('/home/ubuntu/.botho/testnet/config.toml').read_bytes()).hexdigest(),
                'node_key':hashlib.sha256(Path('/home/ubuntu/.botho/testnet/node_key').read_bytes()).hexdigest(),
                'rss':int(process['VmRSS'].split()[0])*1024,
                'swap':int(process['VmSwap'].split()[0])*1024,
                'service_peak':fields['MemoryPeak'],
                'mem_available':int(mem['MemAvailable'].split()[0])*1024,
                'mem_total':int(mem['MemTotal'].split()[0])*1024,
                'disk_free':disk.free, 'disk_total':disk.total,
                'load':os.getloadavg(),
                'cpu_pressure':Path('/proc/pressure/cpu').read_text().strip(),
                'memory_pressure':Path('/proc/pressure/memory').read_text().strip()}
    except Exception as error:
        return {'at':at,'error':type(error).__name__+': '+str(error)}


def run():
    os.umask(0o077)
    STATE.mkdir(mode=0o700,exist_ok=True)
    end = json.loads((STATE/'config.json').read_text())['end']
    records = collections.deque(maxlen=30)
    binary_path = Path('/usr/local/bin/botho')
    last_stat = None
    binary = None
    while time.time() < end:
        started = time.monotonic()
        stat = binary_path.stat()
        identity = (stat.st_ino,stat.st_mtime_ns,stat.st_size)
        if identity != last_stat:
            binary = hashlib.sha256(binary_path.read_bytes()).hexdigest()
            last_stat = identity
        records.append(sample(binary))
        atomic(STATE/'latest.json', list(records))
        log = STATE/'resources.jsonl'
        if log.exists() and log.stat().st_size > 64*1024*1024:
            os.replace(log,STATE/'resources.previous.jsonl')
        with log.open('a') as output:
            output.write(json.dumps(records[-1])+'\n')
        time.sleep(max(.1,5-(time.monotonic()-started)))


if __name__ == '__main__':
    if sys.argv[1:] == ['export']:
        # No caller-controlled paths, command parsing, arbitrary arguments or shell.
        data = (STATE/'latest.json').read_bytes()
        if len(data) > 128*1024:
            raise SystemExit('oversized observer export')
        sys.stdout.buffer.write(data)
    elif sys.argv[1:] == ['run']:
        run()
    else:
        raise SystemExit('expected run or export')
