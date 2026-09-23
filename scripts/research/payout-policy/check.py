"""Independent proposed policy recurrence; no draw implementation or production use."""
THRESHOLD = 250_001_000_000
POLICIES = ('baseline', 'threshold_reserve', 'adaptive_count')


def uint(value, bits):
    assert type(value) is int and 0 <= value < 2**bits
    return value


def check_block(row, previous_reserve):
    """Check each raw block, including zero draws; amount fields are integer pico."""
    policy = row['policy']; assert policy in POLICIES
    previous_reserve = uint(previous_reserve, 128)
    fee = uint(row['fees'], 64)
    assert uint(row['emission'], 64) == 0
    pool = fee * 800 // 1000
    assert uint(row['burn'], 64) == fee - pool
    available = min(2**128 - 1, previous_reserve + pool)
    assert uint(row['available'], 128) == available
    cap = uint(row['cap'], 64)
    rho = uint(row['eligible'], 64)
    assert cap == min(rho * 250_000_000_000, uint(row['block_reward'], 64))
    budget = min(available, cap)
    slots = min(4, rho)
    count = slots if budget else 0
    if policy == 'threshold_reserve' and slots and budget < slots * THRESHOLD:
        count = 0
    if policy == 'adaptive_count':
        count = min(slots, budget // THRESHOLD)
    expected = budget if count else 0
    assert uint(row['distribution'], 64) == expected
    assert uint(row['reserve'], 128) == available - expected
    awards = row['awards']; assert len(awards) == count
    assert len({a['id'] for a in awards}) == count
    assert all(uint(a['value'], 64) == expected // count + (expected % count if i == count-1 else 0)
               for i, a in enumerate(awards))
    assert sum(a['value'] for a in awards) == expected
    if policy != 'baseline':
        assert all(a['value'] >= THRESHOLD for a in awards)
    return available - expected


def derive_fifo(blocks):
    """FIFO is reserve accounting attribution, not ownership or entitlement."""
    from collections import deque
    lots = deque(); releases = []; reserve = 0
    reasons = {}; owner_capture = [0]*101
    for offset, block in enumerate(blocks):
        height = uint(block['height'],64); assert height == 20000+offset
        reserve = check_block(block,reserve)
        reason=block['reason']; reasons[reason]=reasons.get(reason,0)+1
        expected_reason = ('no_eligible' if block['eligible']==0 else 'zero_budget' if min(block['available'],block['cap'])==0 else
            'draw' if block['distribution'] else 'cap_stall' if block['cap'] < THRESHOLD*(min(4,block['eligible']) if block['policy']=='threshold_reserve' else 1) else 'accumulating')
        assert reason == expected_reason
        cohort = block['cohort_expiry']
        assert type(cohort) is dict
        expiry = {}
        for k,v in cohort.items():
            assert str(int(k)) == k
            expiry[uint(int(k),64)] = uint(v,64)
            assert height < int(k) <= height+9281
        inflow=block['fees']*800//1000
        assert sum(expiry.values()) == (block['eligible'] if block['fees'] else 0)
        if inflow: lots.append([height,inflow,expiry])
        remaining=block['distribution']
        for a in block['awards']:
            owner=uint(a['owner'],64);assert owner<101
            owner_capture[owner]+=a['value']
        while remaining:
            created, amount, cohort = lots[0]
            take=min(remaining,amount);remaining-=take;lots[0][1]-=take
            releases.append(dict(inflow_height=created,release_height=height,value=take,wait_blocks=height-created,
                                 cohort_size=sum(cohort.values()),expired_cohort=sum(n for h,n in cohort.items() if h<=height)))
            if lots[0][1]==0:lots.popleft()
        assert sum(lot[1] for lot in lots)==reserve
    return dict(releases=releases,unreleased=[dict(height=h,value=v,cohort_expiry=c) for h,v,c in lots],
                reasons=reasons,owner_capture=owner_capture,
                concentration_numerator=sum(v*v for v in owner_capture),concentration_denominator=sum(owner_capture)**2)


def validate_data(data, baseline):
    import importlib.util, json
    from pathlib import Path
    here=Path(__file__).resolve().parent
    assert data['schema']==3 and data['config']==json.loads((here/'config.json').read_text())
    spec=importlib.util.spec_from_file_location('reinvestment_check',here.parent/'ct-reinvestment/check.py')
    old=importlib.util.module_from_spec(spec);spec.loader.exec_module(old)
    assert len(data['rows'])==6
    seen=set(); summaries=[]
    for item in data['rows']:
        row,blocks=item['row'],item['blocks'];assert len(blocks)==11232
        name=blocks[0]['policy'];assert name in POLICIES
        key=(row['seed'],name);assert row['seed'] in [1306,902] and key not in seen;seen.add(key)
        assert all(b['policy']==name for b in blocks)
        old.validate_row(row)
        assert sum(b['fees'] for b in blocks)==int(row['fees'])
        assert sum(b['burn'] for b in blocks)==int(row['burn'])
        assert sum(b['distribution'] for b in blocks)==int(row['capture'])
        assert blocks[-1]['reserve']==int(row['reserve'])
        fifo=derive_fifo(blocks)
        assert fifo['owner_capture']==[int(o['capture']) for o in row['owners']]
        if name=='baseline':assert row==next(r for r in baseline['rows'] if r['seed']==row['seed'] and r['mode']=='candidate_age720')
        summaries.append(dict(seed=row['seed'],policy=name,fifo=fifo,
            receipts=[dict(fee=r['fee'],consumed=r['consumed'],remaining_principal=r['value']) for r in row['receipts']]))
    return summaries


def load(directory):
    import json
    from pathlib import Path
    from collect import SOURCES, ROOT, COMMAND, digest, artifact
    p=Path(directory);m=json.loads((p/'manifest.json').read_text())
    assert m['schema']==3 and m['status']=='complete'
    assert m['compile_exit']==m['matrix_exit']==0 and m['compile_command']==COMMAND
    assert m['compile_bound_seconds']==900 and m['matrix_bound_seconds']==180
    assert m['histories']==6 and m['replays']==1
    assert m['sources']=={s:digest(ROOT/s) for s in SOURCES}
    expected={'baseline.json','cargo.jsonl','compile.log','matrix.log','matrix-stderr.log','raw.json','fifo-summary.json'}
    assert set(m['raw_hashes'])==expected
    for f,h in m['raw_hashes'].items():assert digest(p/f)==h
    a=artifact(p/'cargo.jsonl');assert a==m['artifact']
    assert m['matrix_command']==[a['executable'],'fixed_six_histories_source_review_draft','--exact','--nocapture']
    assert '1 passed; 0 failed; 0 ignored' in (p/'matrix.log').read_text()
    baseline=json.loads((p/'baseline.json').read_text())
    import gzip
    assert (p/'baseline.json').read_bytes()==gzip.decompress((ROOT/'scripts/research/ct-reinvestment/evidence/raw.json.gz').read_bytes())
    summaries=validate_data(json.loads((p/'raw.json').read_text()),baseline)
    # JSON object keys in unreleased expiry histograms canonicalize to strings.
    assert json.loads(json.dumps(summaries))==json.loads((p/'fifo-summary.json').read_text())
    return summaries

if __name__=='__main__':
    import argparse
    p=argparse.ArgumentParser();p.add_argument('directory');args=p.parse_args()
    print(f'PASS {len(load(args.directory))} histories, owner accounts and FIFO policy recurrence')
