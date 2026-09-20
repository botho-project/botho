#!/usr/bin/env python3
"""Collect selected CI metadata and current Criterion files, never infer coverage."""
import hashlib
import json
import os
import platform
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUTCOMES = {'success', 'failure', 'cancelled', 'skipped', 'unknown'}


def inventory(directory):
    files=[]
    if not directory.exists():
        return files
    if directory.is_symlink():
        raise ValueError('Criterion directory cannot be a symlink')
    for path in sorted(directory.rglob('*.json')):
        if path.is_symlink() or not path.is_file():
            raise ValueError('Criterion JSON must be regular files')
        raw=path.read_bytes()
        try:
            json.loads(raw)
            valid=True
        except (ValueError,UnicodeDecodeError):
            valid=False
        files.append(dict(path=str(path.relative_to(directory)),bytes=len(raw),
                          sha256=hashlib.sha256(raw).hexdigest(),valid_json=valid))
    return files


def manifest(outcomes, files):
    if not isinstance(outcomes,dict) or not outcomes:
        raise ValueError('nonempty outcome mapping required')
    normalized={name:(outcome or 'unknown') for name,outcome in outcomes.items()}
    if any(not isinstance(name,str) or outcome not in OUTCOMES for name,outcome in normalized.items()):
        raise ValueError('invalid step outcome')
    selected=[v for v in normalized.values() if v!='skipped']
    estimates=[f for f in files if f['path'].endswith('/new/estimates.json') and f['valid_json']]
    samples=[f for f in files if f['path'].endswith('/new/sample.json') and f['valid_json']]
    if not files:
        status='missing_measurements'
    elif not selected or any(v!='success' for v in selected) or any(not f['valid_json'] for f in files):
        status='partial_or_unsuccessful_measurements'
    elif not estimates or not samples:
        status='incomplete_criterion_files'
    else:
        status='successful_steps_with_measurements_not_coverage_attestation'
    return dict(status=status,step_outcomes=normalized,criterion_json=files,
                current_estimate_files=len(estimates),current_sample_files=len(samples),
                coverage_note='Counts do not prove every intended benchmark ran; inspect benchmark identities and outcomes. No CLSAG or full transaction coverage inferred.')


def command(args):
    result=subprocess.run(args,cwd=ROOT,capture_output=True,text=True,check=False)
    return dict(exit_code=result.returncode,stdout=result.stdout.strip(),stderr=result.stderr.strip())


def main():
    outcomes=json.loads(os.environ['BENCH_OUTCOMES'])
    result=manifest(outcomes,inventory(ROOT/'target/criterion'))
    result.update(checkout=command(['git','rev-parse','HEAD']),
                  rustc=command(['rustc','-vV']),cargo=command(['cargo','-V']),
                  platform=dict(system=platform.system(),release=platform.release(),machine=platform.machine()),
                  metadata={name:os.environ.get(name,'') for name in [
                      'GITHUB_SHA','GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT','GITHUB_JOB',
                      'GITHUB_EVENT_NAME','RUNNER_OS','RUNNER_ARCH','BENCH_PR_HEAD','BENCH_MODE']})
    sources=[ROOT/'Cargo.lock',ROOT/'Cargo.toml',ROOT/'.github/workflows/benchmarks.yml']
    tracked=command(['git','ls-files','*/benches/*.rs'])
    if tracked['exit_code'] != 0:
        raise ValueError('cannot inventory tracked benchmark source')
    sources.extend(ROOT/p for p in tracked['stdout'].splitlines())
    result['source_sha256']={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest()
                              for p in sources if '.loom' not in p.parts and 'target' not in p.parts}
    out=ROOT/'benchmark-evidence';out.mkdir(exist_ok=True)
    (out/'manifest.json').write_text(json.dumps(result,indent=2)+'\n')
    summary=os.environ.get('GITHUB_STEP_SUMMARY')
    if summary:
        with open(summary,'a') as stream:
            stream.write('\n### Measurement evidence\n\n'+result['status']+'\n\n')
            for name,status in result['step_outcomes'].items(): stream.write(f'- {name}: {status}\n')
            stream.write('\nArtifact manifest records raw outcomes; tolerated failures remain failures.\n')


if __name__=='__main__':main()
