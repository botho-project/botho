# Fixed candidate reinvestment observations

This current-source capture ran eight prescribed 11,232-block histories, one exact
replay, and two accounting/boundary controls at checkout
`8a02b08f6f04d6730dce9ea7d51658e9f2c0d5cd`. All three selected tests passed (two
unrelated reference tests were filtered out). Compile took 38.42 seconds and the
selected process took 7.42 seconds on the host recorded in
`evidence/manifest.json`; the executable SHA256 is
`c3b5c70e3debefe7e19a202a8deb141e79a2398a7ba1074af85f7235159dc30c` and the raw
observation SHA256 is
`15becae3468cffbca2f2faecdd749416e98e310ae4dabfadfd98187dc85f7ab6`.
The independent checker passes against the committed evidence after decompression.
Hosted Linux evidence is pending. These are finite synthetic model observations,
not accepted wallet transactions, V2 deployment, CT ratification or steady state.

Every history completed all 113 ordinary payments: 56.5 BTH gross fees, 45.2 BTH
cumulative lottery capture, 11.3 BTH burned, zero reserve and 779 public outputs.
Private ordinary principal ended at 3,175.5 BTH and unspent awards at 45.2 BTH;
together with burn this reconciles the 3,232-BTH initial stock. All 808 individual
owner records pass the independent transfer/fee/capture accounting checker.

| Seed | Enabled strategy | Attempts | No aged private input | Unaffordable | Success |
|---|---|---:|---:|---:|---:|
| 1306 | age 720 | 11,413 | 4,577 | 6,836 | 0 |
| 1306 | age 10001 | 11,413 | 11,141 | 272 | 0 |
| 902 | age 720 | 11,413 | 4,748 | 6,665 | 0 |
| 902 | age 10001 | 11,413 | 11,143 | 270 | 0 |

The four locked histories scheduled no consumption attempts. No history had
payment or selection failures. There were no consumed awards, new recycled
outputs or successful main-grid reinvestment receipts. The oldest-first policy
cannot borrow ordinary principal. This is not an optimal-strategy search.

The result is consistent with the actual reference fee: background charges are
zero, but consolidation costs 0.25 BTH per input. Each main-grid draw is funded
by a 0.5-BTH two-output payment, distributes 0.4 BTH among four winners, and hence
creates 0.1-BTH awards. Combining these awards does not amortize the per-input
base. The original planning premise was corrected before the first capture;
no matrix fee, selection rule or cadence was changed to obtain success. Both
fee and award are denominated in BTH; floating purchasing power alone does not
change this fee/value ratio under the fixed policy.

The distinct positive control uses an ordinary sixteen-output self-transfer
from existing principal, paying its helper-derived 4-BTH fee. The production
accounting/draw functions produce larger awards that can pay the helper-derived
consolidation fee. It checks private consumption once, public ticket retention,
new-output age and stock/fee conservation. Duplicate IDs fail without mutation.
It is an accounting control, not an extra observed main-grid strategy or signed
ledger-spend claim. Zero main-grid successes do not prove universal inability to
spend awards or establish a fee target. Traffic calibration, richer funded owner
strategies, CT policy decisions and actual V2 integration remain separate work.

## Provenance and review

`evidence/manifest.json` binds 21 captured sources, the dirty checkout baseline,
compiler flags/profile, executable, raw logs and observations. The compile-only
source-review checkpoint remains separately preserved in the local compile
evidence directory; collector metadata was extended before this capture. The
source hashes are capture identity, not a claim that the working tree was clean.

Large raw files are losslessly gzip-compressed with deterministic headers. To
review with the independent checker, copy `evidence/` to a fresh temporary folder,
decompress `cargo.jsonl.gz`, `raw.json.gz` and `matrix.log.gz` there, and run `check.py` on that
folder from this source checkout. The manifest hashes the decompressed bytes.
`summary.json` is a compact projection of those checked raw observations.

The original sixteen-history test bodies, configuration and committed report
remain unchanged. Its shared model helper is now included in the fresh collector
source inventory; historical captured source lists remain untouched. The proposed
hosted workflow additionally compiles and runs the original workload to check the
extraction rather than asserting behavioral equivalence from the new matrix alone.
