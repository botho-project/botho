#!/usr/bin/env python3
"""Resource-only local observer and bounded forced-command export; makes no RPC calls."""
import argparse
import collections
import re
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from runtime import Gate, atomic

STATE = Path('/var/lib/botho-stress-observer-1404')


DEFAULT_NODE = {'node_unit': 'botho.service', 'binary_path': '/usr/local/bin/botho',
                'data_dir': '/home/ubuntu/.botho',
                'config_path': '/home/ubuntu/.botho/testnet/config.toml',
                'node_key_path': '/home/ubuntu/.botho/testnet/node_key'}


def absolute_path(value):
    if (not isinstance(value, str) or not re.fullmatch(r'/[a-zA-Z0-9_./-]+', value)
            or '..' in Path(value).parts or value == '/' or str(Path(value)) != value):
        raise Gate('explicit normalized absolute path required')
    return value


def node_config(config):
    fields = set(DEFAULT_NODE)
    if not fields.intersection(config):
        return dict(DEFAULT_NODE)
    if not fields.issubset(config):
        raise Gate('complete observer node configuration required')
    node = {key: config[key] for key in fields}
    if not isinstance(node['node_unit'], str) or not re.fullmatch(r'[a-zA-Z0-9_-][a-zA-Z0-9_.-]*\.service', node['node_unit']):
        raise Gate('invalid observer node service')
    for key in fields - {'node_unit'}:
        absolute_path(node[key])
    return node


def sample(binary, node=None):
    at = time.time()
    node = DEFAULT_NODE if node is None else node
    try:
        unit = subprocess.check_output(['systemctl','show',node['node_unit'],'-p','MainPID',
            '-p','NRestarts','-p','ExecMainStartTimestampMonotonic','-p','MemoryPeak'],
            text=True, timeout=3)
        fields = dict(line.split('=',1) for line in unit.strip().splitlines())
        pid = int(fields['MainPID'])
        process = dict(line.split(':',1) for line in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in line)
        mem = dict(line.split(':',1) for line in Path('/proc/meminfo').read_text().splitlines())
        disk = shutil.disk_usage(node['data_dir'])
        return {'at':at, 'pid':pid, 'start':fields['ExecMainStartTimestampMonotonic'],
                'restarts':int(fields['NRestarts']), 'binary':binary,
                'config':hashlib.sha256(Path(node['config_path']).read_bytes()).hexdigest(),
                'node_key':hashlib.sha256(Path(node['node_key_path']).read_bytes()).hexdigest(),
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


def run(state=STATE):
    os.umask(0o077)
    state.mkdir(mode=0o700,exist_ok=True)
    config = json.loads((state/'config.json').read_text())
    end = config['end']
    node = node_config(config)
    records = collections.deque(maxlen=30)
    binary_path = Path(node['binary_path'])
    last_stat = None
    binary = None
    while time.time() < end:
        started = time.monotonic()
        stat = binary_path.stat()
        identity = (stat.st_ino,stat.st_mtime_ns,stat.st_size)
        if identity != last_stat:
            binary = hashlib.sha256(binary_path.read_bytes()).hexdigest()
            last_stat = identity
        records.append(sample(binary, node))
        atomic(state/'latest.json', list(records))
        log = state/'resources.jsonl'
        if log.exists() and log.stat().st_size > 64*1024*1024:
            os.replace(log,state/'resources.previous.jsonl')
        with log.open('a') as output:
            output.write(json.dumps(records[-1])+'\n')
        time.sleep(max(.1,5-(time.monotonic()-started)))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['run', 'export'])
    parser.add_argument('--state', type=absolute_path, default=str(STATE))
    args = parser.parse_args()
    state = Path(args.state)
    if args.mode == 'export':
        # No caller-controlled paths, command parsing, arbitrary arguments or shell.
        data = (state/'latest.json').read_bytes()
        if len(data) > 128*1024:
            raise SystemExit('oversized observer export')
        sys.stdout.buffer.write(data)
    else:
        run(state)
