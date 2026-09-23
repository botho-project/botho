#!/usr/bin/env python3
"""Build-only collector: one frontend invocation, one native compile; never launches."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time

DESKTOP = Path(__file__).resolve().parents[1]
REPO = DESKTOP.parents[2]


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def sources():
    paths = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=REPO).split(b'\0')
    return {os.fsdecode(p): sha(REPO / os.fsdecode(p)) for p in sorted(set(paths))
            if p and (REPO / os.fsdecode(p)).is_file()}


def assets():
    return {str(p.relative_to(REPO)): sha(p) for p in sorted((DESKTOP / 'smoke-dist').rglob('*')) if p.is_file()}


def validate_manifest(path):
    manifest = json.loads(path.read_text())
    if not manifest.get('passed'):
        raise ValueError('build manifest is not successful')
    if sources() != manifest['sources_before'] or manifest['sources_before'] != manifest['sources_after']:
        raise ValueError('source snapshot differs from compiled build')
    if not assets() or assets() != manifest['assets_before_native'] or assets() != manifest['assets_after_native']:
        raise ValueError('packaged assets differ from compiled build')
    for key in ('cargo_json', 'binary'):
        if sha(manifest[key]) != manifest[key + '_sha256']:
            raise ValueError(key + ' hash differs from build manifest')
    return manifest


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--target-dir', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(mode=0o700)
    args.output = args.output.resolve()
    environment = os.environ.copy()
    environment['CARGO_TARGET_DIR'] = str(args.target_dir.resolve())
    # `generate_context!("tauri.smoke.conf.json")` selects the Rust-side
    # context, while tauri-build's build script selects its config through
    # TAURI_CONFIG. Keep both halves on the isolated fixture configuration;
    # otherwise Cargo embeds the ordinary app's dev URL and assets.
    environment['TAURI_CONFIG'] = (DESKTOP / 'src-tauri/tauri.smoke.conf.json').read_text()
    report = {'passed': False, 'sources_before': sources(), 'commands': [],
              'checkout': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
              'dirty': subprocess.check_output(['git', 'status', '--porcelain'], cwd=REPO, text=True),
              'environment': {k: v for k, v in environment.items()
                              if k.startswith(('CARGO_PROFILE_', 'TAURI_', 'CARGO_TARGET_'))
                              or k in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'MACOSX_DEPLOYMENT_TARGET', 'SDKROOT')}}

    def run(command, cwd, seconds, stdout_name, stderr_name):
        record = {'argv': command, 'cwd': str(cwd), 'timeout_seconds': seconds}
        report['commands'].append(record)
        start = time.monotonic()
        with (args.output / stdout_name).open('xb') as stdout, (args.output / stderr_name).open('xb') as stderr:
            process = subprocess.Popen(command, cwd=cwd, env=environment, stdout=stdout, stderr=stderr, start_new_session=True)
            print(f'BUILD_PHASE pid={process.pid} deadline={seconds}s command={command}', flush=True)
            try:
                record['exit'] = process.wait(timeout=seconds)
            except BaseException:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
                record['exit'] = process.returncode
                raise
            finally:
                record['elapsed_seconds'] = time.monotonic()-start
        if record['exit']:
            raise RuntimeError(f'{command[0]} failed with {record["exit"]}; retained logs, no retry')

    try:
        run(['pnpm', '--filter', '@botho/desktop', 'exec', 'vite', 'build', '--config', 'vite.smoke.config.ts'], REPO / 'web', 180, 'frontend.log', 'frontend.stderr')
        report['assets_before_native'] = assets()
        if not report['assets_before_native']:
            raise ValueError('frontend produced no packaged assets')
        run(['cargo', 'build', '--locked', '-p', 'botho-desktop', '--bin', 'botho-runtime-smoke', '--features', 'runtime-smoke', '--message-format=json'], REPO, 900, 'cargo.jsonl', 'cargo.stderr')
        cargo = args.output / 'cargo.jsonl'
        records = [json.loads(line) for line in cargo.read_text().splitlines()]
        selected = [r for r in records if r.get('reason') == 'compiler-artifact'
                    and r.get('target', {}).get('name') == 'botho-runtime-smoke' and r.get('executable')]
        if len(selected) != 1 or selected[0]['profile'].get('test') or 'runtime-smoke' not in selected[0]['features']:
            raise ValueError('expected exactly one ordinary smoke binary artifact')
        if not any(r.get('reason') == 'build-finished' and r.get('success') for r in records):
            raise ValueError('missing successful Cargo completion')
        report.update(artifact=selected[0], binary=selected[0]['executable'], binary_sha256=sha(selected[0]['executable']), cargo_json=str(cargo), cargo_json_sha256=sha(cargo))
        report['sources_after'] = sources()
        report['assets_after_native'] = assets()
        if report['sources_before'] != report['sources_after'] or report['assets_before_native'] != report['assets_after_native']:
            raise ValueError('source/assets changed during build')
        report['passed'] = True
    except BaseException as error:
        report['error'] = repr(error)
        raise
    finally:
        (args.output / 'build.json').write_text(json.dumps(report, indent=2) + '\n')
    print('BUILD_ONLY_COMPLETE ' + str(args.output / 'build.json'), flush=True)


if __name__ == '__main__':
    main()
