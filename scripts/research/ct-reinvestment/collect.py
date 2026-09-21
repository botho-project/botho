#!/usr/bin/env python3
"""One bounded candidate-model capture; no retries or schedule changes."""
import argparse
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import time
from check import COMMAND, ROOT, SOURCES, digest, select_artifact, validate_data


def bounded(command, stdout, stderr, env, seconds):
    with stdout.open('w') as out, stderr.open('w') as err:
        p = subprocess.Popen(command, cwd=ROOT, env=env, stdout=out, stderr=err, start_new_session=True)
        try:
            return p.wait(timeout=seconds)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
            return 124


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--target-dir', required=True, type=Path)
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, RUSTC_WRAPPER='', CARGO_TARGET_DIR=str(args.target_dir.resolve()),
               CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS='true', CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS='true')
    m = {'schema': 2, 'status': 'incomplete', 'sources': {p: digest(ROOT / p) for p in SOURCES},
         'checkout_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
         'dirty_status': subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True),
         'platform': platform.platform(), 'machine': platform.machine(),
         'rustc': subprocess.check_output(['rustc', '-vV'], text=True),
         'profile_overrides': {k: env[k] for k in sorted(env) if k.startswith('CARGO_PROFILE_')},
         'rust_flags': {k: env.get(k) for k in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS')},
         'ci': {k: env.get(k) for k in ('GITHUB_SHA', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT')},
         'compile_command': COMMAND, 'compile_bound_seconds': 900, 'matrix_bound_seconds': 180}
    def save(): (out / 'manifest.json').write_text(json.dumps(m, indent=2)+'\n')
    save()
    try:
        start = time.monotonic()
        m['compile_exit'] = bounded(COMMAND, out/'cargo.jsonl', out/'compile.log', env, 900)
        m['compile_seconds'] = time.monotonic()-start
        save()
        if m['compile_exit']: raise RuntimeError('compile failed or exceeded 900 seconds')
        a = select_artifact(out/'cargo.jsonl'); m['artifact'] = a
        binary = Path(a['executable']); m['executable_sha256'] = digest(binary)
        command = [str(binary), 'reinvestment::', '--nocapture']; m['matrix_command'] = command
        env['CT_REINVESTMENT_OUTPUT'] = str(out/'raw.json')
        start = time.monotonic()
        m['matrix_exit'] = bounded(command, out/'matrix.log', out/'matrix-stderr.log', env, 180)
        m['matrix_seconds'] = time.monotonic()-start
        save()
        if m['matrix_exit']: raise RuntimeError('matrix failed or exceeded 180 seconds; no retry')
        assert digest(binary) == m['executable_sha256']
        assert m['sources'] == {p: digest(ROOT/p) for p in SOURCES}, 'source changed during capture'
        rows = validate_data(json.loads((out/'raw.json').read_text()))
        m['histories'] = len(rows); m['deterministic_replays'] = 1
        m['raw_hashes'] = {p: digest(out/p) for p in ['cargo.jsonl', 'compile.log', 'matrix.log', 'matrix-stderr.log', 'raw.json']}
        m['status'] = 'complete'
        save()
    except Exception as error:
        m['status'] = 'failed_or_incomplete'; m['error'] = str(error); save()
        raise


if __name__ == '__main__':
    main()
