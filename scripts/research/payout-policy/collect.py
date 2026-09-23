#!/usr/bin/env python3
"""One bounded inactive capture. Source review required before invocation."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import time
from check import validate_data
ROOT=Path(__file__).resolve().parents[3]
HERE=Path(__file__).resolve().parent
COMMAND=['cargo','test','--locked','--release','-p','botho','--test','ct_payout_policy','--no-run','--message-format=json']
SOURCES=['Cargo.toml','Cargo.lock','rust-toolchain','botho/Cargo.toml',
 'botho/tests/ct_payout_policy.rs','botho/tests/common/reinvestment_model.rs','botho/tests/common/funded_model.rs',
 'botho/tests/ct_economics_reinvestment.rs','scripts/research/ct-economics/reference.rs','scripts/research/ct-reinvestment/check.py',
 'scripts/research/ct-reinvestment/evidence/raw.json.gz','botho/src/decoy_selection.rs','botho/src/block.rs','botho/src/monetary.rs',
 'botho/src/consensus/lottery.rs','cluster-tax/src/lottery.rs','cluster-tax/src/monetary.rs','cluster-tax/src/demurrage.rs',
 *['scripts/research/payout-policy/'+p for p in ['policy.rs','config.json','check.py','collect.py','test_check.py','source_regression.py']]]
def digest(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def bounded(command,out,err,env,limit):
    with out.open('w') as stdout,err.open('w') as stderr:
        p=subprocess.Popen(command,cwd=ROOT,env=env,stdout=stdout,stderr=stderr,start_new_session=True)
        try:return p.wait(timeout=limit)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid,signal.SIGKILL);p.wait();return 124

def artifact(path):
    messages=[json.loads(s) for s in path.read_text().splitlines() if s.startswith('{')]
    found=[m for m in messages if m.get('reason')=='compiler-artifact' and m['target']['name']=='ct_payout_policy' and m.get('executable')]
    assert len(found)==1
    a=found[0];assert a['target']['kind']==['test'] and a['profile']['test']
    assert a['profile']['opt_level']=='3' and a['profile']['debug_assertions'] and a['profile']['overflow_checks']
    assert any(m.get('reason')=='build-finished' and m['success'] for m in messages)
    return a

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--output',type=Path,required=True);parser.add_argument('--target-dir',type=Path,required=True);args=parser.parse_args()
    out=args.output.resolve();out.mkdir(parents=True,exist_ok=False)
    env=dict(os.environ,RUSTC_WRAPPER='',CARGO_TARGET_DIR=str(args.target_dir.resolve()),CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS='true',CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS='true')
    m=dict(schema=3,status='incomplete',sources={p:digest(ROOT/p) for p in SOURCES},compile_command=COMMAND,
           checkout_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
           dirty_status=subprocess.check_output(['git','status','--porcelain'],cwd=ROOT,text=True),platform=platform.platform(),
           rustc=subprocess.check_output(['rustc','-vV'],cwd=ROOT,text=True),
           environment={k:env[k] for k in sorted(env) if k.startswith('CARGO_PROFILE_') or k in ['RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','CARGO_TARGET_DIR','RUSTC_WRAPPER']},
           ci={k:env.get(k) for k in ['GITHUB_SHA','GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT']},compile_bound_seconds=900,matrix_bound_seconds=180)
    def save():(out/'manifest.json').write_text(json.dumps(m,indent=2)+'\n')
    save()
    try:
        (out/'baseline.json').write_bytes(gzip.decompress((ROOT/'scripts/research/ct-reinvestment/evidence/raw.json.gz').read_bytes()))
        start=time.monotonic();m['compile_exit']=bounded(COMMAND,out/'cargo.jsonl',out/'compile.log',env,900);m['compile_seconds']=time.monotonic()-start;save();assert m['compile_exit']==0
        a=artifact(out/'cargo.jsonl');m['artifact']=a;m['executable_sha256']=digest(a['executable'])
        command=[a['executable'],'fixed_six_histories_source_review_draft','--exact','--nocapture'];m['matrix_command']=command
        env.update(PAYOUT_BASELINE_JSON=str(out/'baseline.json'),PAYOUT_POLICY_OUTPUT=str(out/'raw.json'))
        start=time.monotonic();m['matrix_exit']=bounded(command,out/'matrix.log',out/'matrix-stderr.log',env,180);m['matrix_seconds']=time.monotonic()-start;save();assert m['matrix_exit']==0
        assert '1 passed; 0 failed; 0 ignored' in (out/'matrix.log').read_text()
        summary=validate_data(json.loads((out/'raw.json').read_text()),json.loads((out/'baseline.json').read_text()))
        (out/'fifo-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
        assert m['sources']=={p:digest(ROOT/p) for p in SOURCES};assert digest(a['executable'])==m['executable_sha256']
        m.update(histories=6,replays=1,raw_hashes={p.name:digest(p) for p in out.iterdir() if p.name!='manifest.json'},status='complete');save()
    except Exception as error:
        m.update(status='failed_or_incomplete',error=repr(error));save();raise
if __name__=='__main__':main()
