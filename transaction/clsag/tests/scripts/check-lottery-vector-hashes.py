"""Independent hashlib/integer cross-check of the committed Rust/WASM vectors."""
import hashlib
import json
from pathlib import Path
import struct

v = json.loads((Path(__file__).parent.parent / 'fixtures/lottery-v2.json').read_text())
H = lambda b: hashlib.sha256(b).digest()
le = lambda n, width: n.to_bytes(width, 'little')
order = 2**252 + 27742317777372353535851937790883648493
manifest = H(b'BOTHO_LOTTERY_MANIFEST_V2\0' + le(2, 4) + bytes([3])*32 + le(3, 4) + le(900, 8) + bytes([4])*32 + le(5, 4) + le(901, 8))
assert manifest.hex() == v['manifest']
expected = b'BOTHO_LOTTERY_SPEND_TWEAK_V2\0' + bytes([1])*32 + bytes([2])*32 + le(17, 8) + bytes([5])*32 + manifest + le(0, 4) + bytes([3])*32 + le(3, 4) + bytes.fromhex(v['source_target']) + le(3, 4) + bytes(32) + le(900, 8)
assert expected.hex() == v['preimage']
for item in [v, v['repeated']] + v['lineage']:
    digest = hashlib.sha512(bytes.fromhex(item['preimage']) + le(item['counter'], 1)).digest()
    delta = int.from_bytes(digest, 'little') % order
    assert le(delta, 32).hex() == item['delta']
    # Final source tweak field precedes the 8-byte amount in the preimage.
    previous = int.from_bytes(bytes.fromhex(item['preimage'])[-40:-8], 'little')
    actual = item.get('context_tweak', item.get('tweak'))
    assert le((previous + delta) % order, 32).hex() == actual
record = bytes.fromhex(v['record'])
assert H(b'BOTHO_LOTTERY_ROOT_V2\0' + le(1, 4) + record).hex() == v['payout_root']
summary = H(b'BOTHO_LOTTERY_SUMMARY_V2\0' + struct.pack('<QQQ', 1125, 900, 225) + bytes([9])*32)
assert summary.hex() == v['summary_root']
assert H(b'BOTHO_BODY_ROOT_V2\0' + bytes([6])*32 + bytes([5])*32 + bytes.fromhex(v['payout_root']) + summary).hex() == v['body_root']
print('Independent SHA-256/SHA-512, LE encoding and modular-scalar vectors match')

assert H(b'BOTHO_LOTTERY_ROOT_V2\0' + le(1, 4) + bytes.fromhex(v['classical_record'])).hex() == v['classical_payout_root']
