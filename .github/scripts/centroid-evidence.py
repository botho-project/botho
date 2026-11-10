import difflib, hashlib, json, os, shutil, signal, subprocess, time, traceback
from pathlib import Path
ROOT=Path.cwd(); OUT=Path(os.environ['RUNNER_TEMP'])/'centroid-evidence'; OUT.mkdir(exist_ok=True)
DRT=ROOT/'formal-drt'; SOURCE=ROOT/'cluster-tax/src/demurrage.rs'; ORIGINAL=SOURCE.read_bytes()
RUNNER=DRT/'target/release/botho-formal-drt'; MODEL=DRT/'lean/.lake/build/bin/botho-model'
def sha(p): return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def git(*a): return subprocess.check_output(['git',*a],text=True).strip()
receipt=dict(checkout_head=git('rev-parse','HEAD'),checkout_tree=git('rev-parse','HEAD^{tree}'),workflow_head=os.environ['GITHUB_SHA'],workflow_ref=os.environ['GITHUB_REF'],mode=os.environ['CENTROID_MODE'],run_id=os.environ['GITHUB_RUN_ID'],commands=[],controls={},all_commands_settled=False)
def save(): (OUT/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
def run(name,args,expected=0,cap=900,cwd=ROOT,extra=None):
    log=OUT/(name+'.log'); start=time.monotonic(); env=os.environ.copy(); env.update(extra or {})
    with log.open('wb') as f:
        p=subprocess.Popen(args,cwd=cwd,stdout=f,stderr=subprocess.STDOUT,env=env,start_new_session=True)
        try: rc=p.wait(timeout=cap)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid,signal.SIGTERM)
            try: p.wait(timeout=10)
            except subprocess.TimeoutExpired: os.killpg(p.pid,signal.SIGKILL); p.wait()
            rc=124
    cmd=dict(name=name,args=list(map(str,args)),cwd=str(cwd),environment=extra or {},cap_seconds=cap,expected_exit=expected,exit=rc,seconds=time.monotonic()-start,log_sha256=sha(log)); receipt['commands'].append(cmd); save(); assert rc==expected,cmd
    return log.read_text()
def report(name,directory,cases,expected):
    p=OUT/(name+'.json')
    run(name,[str(RUNNER),'--legs','ring_elapsed_centroid','--cases',str(cases),'--seed','1998','--regressions',str(directory),'--json',str(p)],expected,180,DRT)
    data=json.loads(p.read_text()); assert len(data['legs'])==1
    return data['legs'][0]
def semantic(leg):
    assert leg['mismatches']
    for m in leg['mismatches']:
        assert all(isinstance(m[k],str) and m[k].isdecimal() for k in ['rust','lean']),m
        assert m['rust']!=m['lean'],m
try:
    paths=[p for p in git('ls-files').splitlines() if p.startswith(('cluster-tax/','formal/','formal-drt/','.github/workflows/formal')) or p in ['Cargo.toml','Cargo.lock','rust-toolchain.toml']]
    hashes={p:sha(ROOT/p) for p in paths if (ROOT/p).is_file()}; receipt['source_hashes']=hashes
    (OUT/'source-original.rs').write_bytes(ORIGINAL); shutil.copy(__file__,OUT/'control.py')
    for p in ['Cargo.toml','Cargo.lock','cluster-tax/Cargo.toml','formal-drt/Cargo.toml','formal/Botho/Demurrage/Age.lean']:
        shutil.copy(ROOT/p,OUT/p.replace('/','-'))
    run('rust-toolchain',['bash','-c','rustc -vV && cargo -V'],cwd=DRT)
    probe=ROOT/'centroid-probe'; (probe/'src').mkdir(parents=True)
    (probe/'Cargo.toml').write_text('[package]\nname="centroid-probe"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nbth-cluster-tax={path="../cluster-tax"}\n[profile.release]\noverflow-checks=true\n')
    (probe/'src/main.rs').write_text('fn main() { let m=std::hint::black_box(u64::MAX); let members=std::hint::black_box([(m,0),(m,0)]); println!("centroid={}",bth_cluster_tax::ring_elapsed_centroid(&members,m)); }\n')
    for mode,checks in [('checked','true'),('unchecked-diagnostic','false')]:
        target=DRT/'target'/('centroid-probe-'+mode); env={'CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS':checks,'CARGO_TARGET_DIR':str(target),'RUST_BACKTRACE':'1'}
        run(mode+'-build',['cargo','build','--release','-vv'],cwd=probe,extra=env)
        exe=target/'release/centroid-probe'; receipt[mode]=dict(source_sha256=sha(SOURCE),executable_sha256=sha(exe),profile='release',overflow_checks=checks,manifest_sha256=sha(probe/'Cargo.toml'),resolved_lock_sha256=sha(probe/'Cargo.lock'))
        old=os.environ['CENTROID_MODE']=='old'
        output=run(mode+'-execute',[str(exe)],101 if old and checks=='true' else 0,60,probe,env)
        if old and checks=='true': assert 'attempt to add with overflow' in output and 'demurrage.rs:402' in output
        else: assert 'centroid='+str(9223372036854775806 if old else 18446744073709551615) in output
        receipt[mode]['output']=output; save()
    shutil.copytree(probe,OUT/'probe',ignore=shutil.ignore_patterns('target'))
    if os.environ['CENTROID_MODE']!='old':
        run('lean-toolchain',['bash','-c','lean --version && lake --version'],cwd=DRT/'lean')
        shutil.copy(DRT/'Cargo.lock',OUT/'resolved-drt-Cargo.lock'); shutil.copy(ROOT/'Cargo.lock',OUT/'copied-root-Cargo.lock')
        receipt['normal_runner_sha256']=sha(RUNNER); receipt['model_sha256']=sha(MODEL)
        run('live-vectors',['cargo','test','--release','centroid_live_rust_and_lean_vectors','--','--nocapture'],cap=420,cwd=DRT)
        run('helper-unit-tests',['cargo','test','--release','-p','bth-cluster-tax','--lib','test_ring_elapsed_centroid','--','--nocapture'],cap=420,extra={'CARGO_TARGET_DIR':str(DRT/'target')})
        receipt['discovered_legs']=run('discovery',[str(RUNNER),'--list'],cap=60,cwd=DRT).splitlines(); assert len(receipt['discovered_legs'])==28
        if os.environ['CENTROID_MODE']=='controls':
            vectors=[json.loads(l) for l in (DRT/'regressions/ring_elapsed_centroid.jsonl').read_text().splitlines()]
            start=ORIGINAL.decode().index('pub fn ring_elapsed_centroid('); end=ORIGINAL.decode().index('\n}\n',start)+2
            oldfn=subprocess.check_output(['git','show','9f62edc722e33988965d2d063eee2a65624cddf6:cluster-tax/src/demurrage.rs'],text=True)
            a=oldfn.index('pub fn ring_elapsed_centroid('); b=oldfn.index('\n}\n',a)+2; oldfn=oldfn[a:b]
            for kind,body,note,actual in [
                ('saturation',oldfn.replace('weighted += value as u128 * elapsed as u128;','weighted = weighted.saturating_add(value as u128 * elapsed as u128);'),'two maximal products','9223372036854775808'),
                ('unweighted','pub fn ring_elapsed_centroid(members: &[(u64,u64)], current_height: u64) -> u64 {\n    if members.is_empty() { return 0; }\n    (members.iter().map(|m| current_height.saturating_sub(m.1) as u128).sum::<u128>() / members.len() as u128) as u64\n}','unequal weights','6')]:
                text=ORIGINAL.decode(); mutated=text[:start]+body+text[end:]; SOURCE.write_text(mutated)
                patch=OUT/(kind+'.patch'); patch.write_text(''.join(difflib.unified_diff(text.splitlines(True),mutated.splitlines(True),fromfile='a/cluster-tax/src/demurrage.rs',tofile='b/cluster-tax/src/demurrage.rs')))
                control=dict(source_sha256=sha(SOURCE),patch_sha256=sha(patch)); receipt['controls'][kind]=control
                run(kind+'-build',['cargo','build','--release'],cwd=DRT); control['runner_sha256']=sha(RUNNER)
                target=OUT/(kind+'-target-fixtures'); target.mkdir(); v=next(v for v in vectors if v['note']==note)
                (target/'ring_elapsed_centroid.jsonl').write_text(json.dumps(v)+'\n')
                leg=report(kind+'-target',target,0,1); semantic(leg); assert leg['replayed']==1 and leg['mismatches'][0]['rust']==actual and leg['mismatches'][0]['lean']==v['expected']; control['target']=leg
                if kind=='saturation':
                    directory=OUT/'generated-fixtures'; directory.mkdir(); assert not list(directory.iterdir())
                    leg=report('generated',directory,64,1); semantic(leg); assert leg['replayed']==0 and leg['compared']>0 and leg['shrink_attempts']>0
                    fixture=directory/'ring_elapsed_centroid.jsonl'; before=sha(fixture); control['generated']=leg; control['saved_fixture_sha256']=before
                    replay=report('generated-failing-replay',directory,0,1); semantic(replay); assert replay['replayed']==len(fixture.read_text().splitlines()); assert sha(fixture)==before; control['replay']=replay
                SOURCE.write_bytes(ORIGINAL); assert sha(SOURCE)==hashes['cluster-tax/src/demurrage.rs']; save()
            run('restored-build',['cargo','build','--release'],cwd=DRT); receipt['restored_runner_sha256']=sha(RUNNER)
            for kind in ['saturation-target','unweighted-target','generated']:
                d=OUT/(kind+'-fixtures'); before=sha(d/'ring_elapsed_centroid.jsonl'); clean=report(kind+'-restored',d,0,0)
                assert not clean['mismatches'] and sha(d/'ring_elapsed_centroid.jsonl')==before; receipt[kind+'-restored']=clean
            run('restored-live-vectors',['cargo','test','--release','centroid_live_rust_and_lean_vectors','--','--nocapture'],cap=420,cwd=DRT)
        assert sha(MODEL)==receipt['model_sha256']
    assert {p:sha(ROOT/p) for p in hashes}==hashes
    run('final-source-diff',['git','diff','--exit-code'],cap=60)
    receipt['status']='success'
except BaseException:
    receipt['status']='failure'; receipt['error']=traceback.format_exc(); raise
finally:
    SOURCE.write_bytes(ORIGINAL); receipt['restored_source_sha256']=sha(SOURCE); receipt['all_commands_settled']=True; save()
