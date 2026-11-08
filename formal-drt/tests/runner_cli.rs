//! Synthetic protocol control of the actual CLI/consumer, not Lean
//! correspondence.
#![cfg(unix)]

use std::process::Command;

#[test]
fn actual_cli_budget_report_and_replay_contract() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new("python3")
        .arg("-c")
        .arg(CONTROL)
        .arg(env!("CARGO_BIN_EXE_botho-formal-drt"))
        .arg(env!("CARGO_MANIFEST_DIR"))
        .arg(dir.path())
        .output()
        .expect("run bounded synthetic CLI control");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "synthetic CLI control failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

const CONTROL: &str = r#"
import hashlib, json, os, pathlib, subprocess, sys, time
binary, crate, root = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])
sys.path.insert(0, str(crate / 'scripts'))
import nightly as n
leg = 'sybil_reward_cap'
model = root / 'model.py'
model.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys, time
p = pathlib.Path(__file__)
p.with_suffix('.pid').write_text(str(os.getpid()))
mode = p.with_suffix('.mode').read_text()
for line in sys.stdin:
    args = json.loads(line)['args']
    if mode == 'exit': sys.exit(2)
    if mode == 'timeout': time.sleep(60)
    if mode == 'malformed': print('not-json', flush=True); continue
    if mode == 'error': print(json.dumps({'err': 'synthetic model error'}), flush=True); continue
    # Only a cheap protocol stub; this is not an independent economic oracle.
    value = int(args['count']) * 250_000_000_000
    print(json.dumps({'ok': str(value + (mode == 'defect'))}), flush=True)
''')
model.chmod(0o755)
fixtures = root / 'fixtures'
fixtures.mkdir()
fixture = fixtures / (leg + '.jsonl')
sha = lambda data: hashlib.sha256(data).hexdigest()
identity = {'kind': 'synthetic harness only', 'binary_sha256': sha(binary.read_bytes()),
            'model_sha256': sha(model.read_bytes()),
            'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=crate, text=True).strip(),
            'tree': subprocess.check_output(['git', 'rev-parse', 'HEAD^{tree}'], cwd=crate, text=True).strip()}

def invoke(name, mode, cases, replayed, expected, extra=()):
    model.with_suffix('.mode').write_text(mode)
    model.with_suffix('.pid').unlink(missing_ok=True)
    report_path, log = root / (name + '.json'), root / (name + '.log')
    command = [str(binary), '--model', str(model), '--legs', leg, '--cases', str(cases),
               '--batch', '16', '--seed', '1707', '--regressions', str(fixtures),
               '--json', str(report_path), *extra]
    status = n.execute(command, log, 0.3 if mode == 'timeout' else 20)
    meta = {'seed': '1707', 'cases': cases, 'replay_counts': {leg: replayed}}
    report = None
    try:
        report = n.read_json(report_path)
        verdict = n.validate_report(report, status, meta, leg)
    except (ValueError, OSError):
        verdict = 'infrastructure'
    assert verdict == expected, (name, verdict, status, report, log.read_text())
    if expected != 'infrastructure':
        assert status == {'exit': int(expected == 'mismatch'), 'timeout': False}
    if mode == 'timeout': assert status['timeout'] is True
    elif mode == 'error': assert status['exit'] == 1 and report is not None
    elif mode in ('exit', 'malformed'): assert status['exit'] != 0 and report is None
    elif name == 'reject-zero-batch': assert status['exit'] == 2 and report is None
    pidfile = model.with_suffix('.pid')
    if pidfile.exists():
        pid = int(pidfile.read_text())
        for _ in range(100):
            try: os.kill(pid, 0)
            except ProcessLookupError: break
            # The killed process-group child may briefly await init's reap.
            stat = pathlib.Path('/proc') / str(pid) / 'stat'
            if stat.exists() and stat.read_text().split()[2] == 'Z': break
            time.sleep(0.01)
        else: raise AssertionError(('model still running', name, pid))
    record = dict(identity, name=name, argv=command, status=status, verdict=verdict,
                  report=report, report_sha256=sha(report_path.read_bytes()) if report_path.exists() else None,
                  log_sha256=sha(log.read_bytes()), fixture=fixture.read_text() if fixture.exists() else None,
                  fixture_sha256=sha(fixture.read_bytes()) if fixture.exists() else None,
                  model_process_settled=True)
    print('runner-cli-evidence: ' + json.dumps(record, sort_keys=True), flush=True)
    return report

r = invoke('generate', 'defect', 64, 0, 'mismatch')
row = r['legs'][0]
assert (row['cases'], row['compared'], row['shrink_attempts']) == (16, 5, 5), row
assert len(row['mismatches']) == len(fixture.read_text().splitlines()) == 1
assert row['mismatches'][0]['input'] == {'count': 0}
original = fixture.read_bytes()
r = invoke('replay-defect', 'defect', 0, 1, 'mismatch')
assert all(r['legs'][0][k] == 0 for k in ['cases', 'compared', 'shrink_attempts'])
assert fixture.read_bytes() == original
invoke('replay-restored', 'agree', 0, 1, 'clean')
assert fixture.read_bytes() == original
r = invoke('clean', 'agree', 19, 1, 'clean')
assert (r['legs'][0]['cases'], r['legs'][0]['compared'], r['legs'][0]['shrink_attempts']) == (19, 19, 0)
r = invoke('no-write', 'defect', 64, 1, 'mismatch', ['--no-write-regressions'])
assert (r['legs'][0]['cases'], r['legs'][0]['compared'], r['legs'][0]['shrink_attempts']) == (16, 4, 4)
assert fixture.read_bytes() == original
for mode in ['error', 'exit', 'malformed', 'timeout']:
    invoke('reject-' + mode, mode, 3, 1, 'infrastructure')
    assert fixture.read_bytes() == original
invoke('reject-zero-batch', 'agree', 3, 1, 'infrastructure', ['--batch', '0'])
"#;
