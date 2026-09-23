#!/usr/bin/env python3
"""One bounded native smoke invocation. Never compiles or starts the ordinary app."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import socket
import subprocess
import time
import urllib.error
import urllib.request
from build_smoke import validate_manifest

ELEMENT = 'element-6066-11e4-a52e-4f735466cecf'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    build = validate_manifest(args.build_manifest.resolve(strict=True))
    binary = Path(build['binary']).resolve(strict=True)
    cargo_json = Path(build['cargo_json'])
    records = [json.loads(line) for line in cargo_json.read_text().splitlines()]
    artifacts = [r for r in records if r.get('reason') == 'compiler-artifact'
                 and r.get('target', {}).get('name') == 'botho-runtime-smoke'
                 and r.get('executable') and Path(r['executable']).resolve() == binary]
    if len(artifacts) != 1 or 'runtime-smoke' not in artifacts[0]['features']:
        raise ValueError('require one exact smoke executable/feature Cargo artifact')
    if not any(r.get('reason') == 'build-finished' and r.get('success') for r in records):
        raise ValueError('Cargo build did not finish successfully')
    if platform.system() != 'Darwin':
        raise ValueError('this initial harness is current-host macOS only')
    args.output.mkdir(mode=0o700)  # Existing output is never overwritten.
    desktop = Path(__file__).resolve().parents[1]
    repo = desktop.parents[2]
    sources = sorted([*desktop.joinpath('smoke').glob('*'),
                      desktop / 'vite.config.ts', desktop / 'vite.smoke.config.ts',
                      desktop / 'src-tauri/Cargo.toml', desktop / 'src-tauri/src/lib.rs',
                      desktop / 'src-tauri/src/wallet.rs', desktop / 'src-tauri/src/bin/runtime_smoke.rs',
                      desktop / 'src-tauri/tauri.smoke.conf.json', repo / 'Cargo.lock',
                      repo / 'web/pnpm-lock.yaml',
                      desktop / 'src/components/network/network-graph.tsx',
                      desktop / 'src/components/network/peer-node.tsx',
                      repo / 'web/packages/features/src/wallet/components/send-modal.tsx',
                      repo / 'web/packages/core/src/format.ts',
                      repo / 'web/packages/ui/src/styles/theme.css'])
    assets = sorted(desktop.joinpath('smoke-dist').rglob('*'))
    if not assets:
        raise ValueError('packaged frontend is absent')
    with open('/System/Library/Frameworks/WebKit.framework/Resources/Info.plist', 'rb') as f:
        webkit = plistlib.load(f)
    report = {
        'build_manifest_sha256': sha(args.build_manifest),
        'scope': 'current-host native fixture; no oldest-host or signing/broadcast claim',
        'binary': str(binary), 'binary_sha256': sha(binary),
        'cargo_json_sha256': sha(cargo_json), 'artifact': artifacts[0],
        'os': platform.platform(), 'architecture': platform.machine(),
        'webkit_version': webkit.get('CFBundleVersion'),
        'checkout': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
        'dirty': subprocess.check_output(['git', 'status', '--porcelain'], cwd=repo, text=True),
        'sources': {str(p.relative_to(repo)): sha(p) for p in sources if p.is_file()},
        'frontend': {str(p.relative_to(desktop)): sha(p) for p in assets if p.is_file()},
        'assertions': [], 'passed': False,
    }
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    deadline = time.monotonic() + 90
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(method, path, payload=None):
        if time.monotonic() >= deadline:
            raise TimeoutError('90-second smoke deadline')
        data = None if payload is None else json.dumps(payload).encode()
        req = urllib.request.Request(f'http://127.0.0.1:{port}{path}', data=data,
                                     headers={'Content-Type': 'application/json'}, method=method)
        with opener.open(req, timeout=min(5, max(.1, deadline - time.monotonic()))) as response:
            value = json.load(response)['value']
        if isinstance(value, dict) and 'error' in value:
            raise RuntimeError(value)
        return value

    env = os.environ.copy()
    env.update(BOTHO_RUNTIME_SMOKE='owned-fixture-only', BOTHO_SMOKE_PORT=str(port))
    log_path = args.output / 'native.log'
    process = None
    try:
        with log_path.open('xb') as log:
            process = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
            # Startup readiness polling only; no process or assertion retries.
            while True:
                if process.poll() is not None:
                    raise RuntimeError('native smoke exited before driver readiness')
                try:
                    request('GET', '/status')
                    break
                except (urllib.error.URLError, ConnectionError):
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(.1)
            session = request('POST', '/session', {'capabilities': {'alwaysMatch': {}}})['sessionId']
            prefix = f'/session/{session}'

            def execute(script):
                return request('POST', prefix + '/execute/sync', {'script': script, 'args': []})

            def wait(script):
                while not execute('return ' + script):
                    time.sleep(.05)

            def element(selector):
                return request('POST', prefix + '/element', {'using': 'css selector', 'value': selector})[ELEMENT]

            def click(selector):
                request('POST', prefix + '/element/' + element(selector) + '/click', {})

            def type_text(selector, text):
                request('POST', prefix + '/element/' + element(selector) + '/value', {'text': text})

            def check(name, condition):
                if not condition:
                    raise AssertionError(name)
                report['assertions'].append(name)

            wait("document.querySelector('#status')?.textContent === 'ready'")
            report['user_agent'] = execute('return navigator.userAgent')
            check('packaged origin', execute("return location.protocol === 'tauri:' && location.hostname === 'localhost'"))
            check('exact BigInt parse', execute("return document.querySelector('#bigint').textContent === '9007199254740993'"))
            check('exact balance-minus-amount-minus-fee', execute("return document.querySelector('#remaining').textContent === '9007198254740993'"))
            report['theme_style'] = execute("const s=getComputedStyle(document.querySelector('#theme-probe')); return {color:s.color,fontSize:s.fontSize,fontWeight:s.fontWeight,pulse:getComputedStyle(document.documentElement).getPropertyValue('--color-pulse')}")
            check('actual Tailwind theme/layout', report['theme_style']['fontSize'] == '18px' and bool(report['theme_style']['pulse'].strip()))
            wait("document.querySelectorAll('.react-flow__node').length === 2")
            click('.react-flow__node[data-id="fixture-b"]')
            wait("document.querySelector('#selected').textContent === 'fixture-b'")
            check('graph selection callback', True)
            click('.react-flow__node[data-id="fixture-a"] .react-flow__handle.source')
            wait("document.querySelector('.react-flow__connection-path') !== null")
            click('.react-flow__node[data-id="fixture-b"] .react-flow__handle.target')
            wait("document.querySelector('.react-flow__connection-path') === null")
            check('graph handle interaction completed', True)
            click('#unlock')
            wait("document.querySelector('#status').textContent !== 'ready'")
            check('native unlock/session', execute("return document.querySelector('#status').textContent === 'unlocked'"))
            recipient = execute("return document.querySelector('#recipient').textContent")
            click('#prepare')
            type_text('input[placeholder="tbotho://1/... or search contacts"]', recipient)
            type_text('input[placeholder="0.00"]', '9007.199254740993')
            wait("document.querySelector('#fee-amount').textContent === '9007199254740993'")
            # Locate the actual shared SendModal button, not a replacement form.
            execute("document.querySelectorAll('button').forEach(b=>{if(b.textContent.trim()==='Send Transaction')b.id='smoke-submit-preflight'})")
            click('#smoke-submit-preflight')
            wait("document.querySelector('#preflight').textContent === '9007199254740993'")
            check('shared SendModal to native pure preflight', True)
            click('#lock')
            wait("document.querySelector('#status').textContent === 'locked'")
            check('native lock/session', True)
            check('no frontend errors', execute('return window.__smokeErrors.length === 0'))
            # Close the only native window so run_return releases managed fixture state.
            request('DELETE', prefix + '/window')
            process.wait(timeout=max(.1, min(10, deadline-time.monotonic())))
            check('native process exit', process.returncode == 0)
            report['passed'] = True
    except BaseException as error:
        report['error'] = repr(error)
        raise
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
        report['process_exit'] = process.returncode if process else None
        lines = log_path.read_text(errors='replace').splitlines() if log_path.exists() else []
        owned = [line.split('=', 1)[1] for line in lines if line.startswith('BOTHO_SMOKE_OWNED_DIRECTORY=')]
        report['fixture_cleanup'] = len(owned) == 1 and not Path(owned[0]).exists()
        # Preserve unexpected remnants and report failure; never recursively remove an untrusted path.
        report['passed'] = report['passed'] and report['fixture_cleanup']
        (args.output / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    if not report['passed']:
        raise RuntimeError('smoke/owned-fixture cleanup failed; see retained result.json')


if __name__ == '__main__':
    main()
