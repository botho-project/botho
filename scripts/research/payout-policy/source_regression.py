"""Source-only extraction check, not a claim of regenerated observation equality."""
import gzip
import hashlib
import json
import re
import subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parents[3]
BASE = 'df09a41d6944d955b979b3b0cb4688297c6c8a3a'
def old(path): return subprocess.check_output(['git','show',f'{BASE}:{path}'],cwd=ROOT)
def normalize(s): return re.sub(r',(?=\))', '', re.sub(r'\s+', '', s))
path='botho/tests/ct_economics_reinvestment.rs'
a=old(path).decode();b=(ROOT/path).read_text()
# Original three test bodies exactly retained, including archived output serialization.
assert a[a.index('    #[test]'):]==b[b.index('    #[test]'):]
body=a[a.index('    const OWNERS:'):a.index('    #[test]')]
shared=(ROOT/'botho/tests/common/reinvestment_model.rs').read_text()
shared=shared[shared.index('const OWNERS:'):]
shared=shared.replace('pub(crate) fn','fn')
shared=shared.replace('fn history(seed: u64, mode: &str) -> Value {\n    history_with(seed, mode, awards)\n}\n','')
start=shared.index('fn history_with(');end=shared.index('    let mut model',start)
shared=shared[:start]+'fn history(seed: u64, mode: &str) -> Value {\n'+shared[end:]
shared=shared.replace('draw(&mut model, height, seed, block_fees, reserve, mode)', 'awards(&mut model, height, seed, block_fees, reserve, mode)')
assert normalize(body)==normalize(shared), 'non-test helper changed beyond explicit callback/visibility seam'
archive='scripts/research/ct-reinvestment/evidence/raw.json.gz'
raw=gzip.decompress((ROOT/archive).read_bytes());assert raw==gzip.decompress(old(archive))
rows=[r for r in json.loads(raw)['rows'] if r['mode']=='candidate_age720']
assert len(rows)==2
print('PASS original test bodies; normalized exact helper extraction; archived baseline bytes unchanged')
for r in rows:
 print(r['seed'],hashlib.sha256(json.dumps(r,sort_keys=True,separators=(',',':')).encode()).hexdigest())
print('Runtime regenerated row equality remains UNEXECUTED; new target asserts serialized Value equality.')
