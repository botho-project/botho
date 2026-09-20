#!/usr/bin/env python3
"""Compile once, execute exactly six bounded positive V1 apply observations."""
import argparse
import json
import hashlib
import os
import platform
import signal
import subprocess
import time
from pathlib import Path
from accepted_v1 import ROOT, SOURCES, TEST, COMPILE_COMMAND, digest, load_evidence, observed_inputs, select_artifact

def cpu_identity():
    if platform.system() == 'Darwin':
        return {'source': 'sysctl machdep.cpu.brand_string',
                'model': command_text(['sysctl', '-n', 'machdep.cpu.brand_string'])}
    if platform.system() == 'Linux':
        models = sorted({line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines()
                         if line.startswith(('model name', 'Hardware')) and ':' in line})
        return {'source': '/proc/cpuinfo model name/Hardware', 'model': models or ['unavailable']}
    return {'source': 'unavailable', 'model': None}


def command_text(args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def bounded(args, stdout, stderr, env, seconds):
    start = time.monotonic()
    p = subprocess.Popen(args, cwd=ROOT, env=env, stdout=stdout, stderr=stderr, start_new_session=True)
    try:
        code = p.wait(timeout=seconds)
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGKILL)
        p.wait()
        code = 124
    return code, time.monotonic() - start


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--target-dir', type=Path, required=True)
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)  # Never consume stale evidence.
    env = os.environ.copy()
    env['CARGO_TARGET_DIR'] = str(args.target_dir.resolve())
    env['RUSTC_WRAPPER'] = ''
    env.pop('RUST_TEST_THREADS', None)
    sources = {p: digest(ROOT / p) for p in SOURCES}
    metadata = {'schema': 1, 'status': 'incomplete', 'sources': sources,
                'checkout_commit': command_text(['git', 'rev-parse', 'HEAD']),
                'dirty_status': command_text(['git', 'status', '--porcelain']),
                'tracked_diff_sha256': __import__('hashlib').sha256(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=ROOT)).hexdigest(),
                'platform': platform.platform(), 'machine': platform.machine(),
                'cpu_identity': cpu_identity(),
                'host_id_sha256': hashlib.sha256(platform.node().encode()).hexdigest(),
                'build_environment': {k: env.get(k) for k in sorted({'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER'} | {k for k in env if k.startswith('CARGO_PROFILE_')})},
                'logical_cpus': os.cpu_count(), 'rustc': command_text(['rustc', '-Vv']),
                'cargo': command_text(['cargo', '-V']), 'target_dir': env['CARGO_TARGET_DIR'],
                'ci': {k: os.environ.get(k) for k in ('GITHUB_SHA', 'GITHUB_HEAD_REF', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT', 'GITHUB_JOB')},
                'runs': [], 'limits': 'Non-isolated host; setup excluded; no energy/full-wire/CT1-cost measurement'}
    def save():
        (out / 'manifest.json').write_text(json.dumps(metadata, indent=2) + '\n')
    save()
    try:
        compile_cmd = COMPILE_COMMAND
        metadata['compile_command'] = compile_cmd
        with (out / 'cargo.jsonl').open('w') as log, (out / 'compile.log').open('w') as err:
            code, elapsed = bounded(compile_cmd, log, err, env, 900)
        metadata.update(compile_exit=code, compile_seconds=elapsed, cargo_json_sha256=digest(out / 'cargo.jsonl'))
        save()
        if code:
            raise RuntimeError('compile failed or exceeded 900 seconds')
        artifact = select_artifact(out / 'cargo.jsonl')
        binary = artifact['executable']
        metadata.update(artifact=artifact, executable_sha256=digest(binary))
        save()
        deadline = time.monotonic() + 180
        for n in range(3):
            for i in range(2):
                case = f'payments-{n}-sample-{i}'
                child = env | {'BOTHO_V1_RESOURCE_PAYMENTS': str(n)}
                cmd = [binary, TEST, '--exact', '--ignored', '--nocapture']
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError('matrix exceeded 180 seconds')
                with (out / (case + '.log')).open('w') as log:
                    code, elapsed = bounded(cmd, log, subprocess.STDOUT, child, min(60, remaining))
                metadata['runs'].append({'case': case, 'command': cmd, 'exit': code, 'seconds': elapsed,
                                         'log': case + '.log', 'sha256': digest(out / (case + '.log'))})
                save()
                if code:
                    raise RuntimeError('sample failed; no retry or further cases')
        if sources != {p: digest(ROOT / p) for p in SOURCES} or digest(binary) != metadata['executable_sha256']:
            raise RuntimeError('source/executable changed during capture')
        metadata['status'] = 'complete'
        save()
        _, samples = load_evidence(out)
        (out / 'observed-inputs.json').write_text(json.dumps(observed_inputs(samples), indent=2) + '\n')
    except Exception as error:
        metadata['status'] = 'failed'
        metadata['error'] = str(error)
        save()
        raise


if __name__ == '__main__':
    main()
