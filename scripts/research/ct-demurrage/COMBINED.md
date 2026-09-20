# Inactive combined arithmetic pilot

Part of #1307. This unpublished, entirely `cfg(test)` experiment composes the
existing exact-demurrage constraints with shared committed variables, output
ranges, aggregate fee bucket membership and integer conservation. It does not
implement CT transactions, CLSAG ownership integration, ledger authentication,
wallet codecs, memo encryption, activation, or an accepted fee policy. It must
not close #1307 or the CT implementation epic.

The implementation is a private child module of `proof_experiment`:
[`src/proof_experiment/combined.rs`](src/proof_experiment/combined.rs). It reuses
`CircuitLayout::allocate`, project token-0 commitment generators, canonical
cross-dalek conversions, and the actual integer-kernel fixtures. No dependency,
production export, RNG API or consensus behavior changes.

## Relation and public parameter family

For each input, the exact same externally committed amount variable is bound to
the reused per-input circuit. Its exact charge variable is bound to `C_di` and
feeds the aggregate. Every output commitment is bound to its 64-bit amount. The
same original input amounts participate in every annual/accrued/reset branch;
there are no independently supplied transfer-value witnesses.

The public research parameters include significant bits `s` in `{2,3,4}` and a
u64 base unit. Base is `base_unit * max(input_count, output_count)` in u128.
The verifier checks counts in `1..=16`, computes `q = fee - base` without
underflow, and rejects a noncanonical bucket endpoint. It derives the exact
preimage `[L(q), H(q)]` of the deterministic rounded aggregate charge. Circuit
constraints require

```
D = sum(d_i)
D = L(q) + lower_gap
H(q) = D + upper_gap
sum(v_i) = sum(v_out) + fee
```

Both gaps are nonnegative 68-bit integers. Charge ranges come from the existing
64-bit exact-charge gadgets, so no independent free `D` variable is allocated.
This proves both bucket endpoints; it does not merely establish an upper bound.
Rounding occurs after aggregation. The public fee remains u64 and no fee overflow
is clamped. A zero charge has the singleton zero bucket. These parameters are a
research family, not D1/base ratification; public factor/age fixtures likewise do
not ratify D3. Ratified D2 is not changed.

Verification also checks the group conservation relation
`sum(P_i) - sum(C_out) - fee*H = 0`. Fixture generation balances input/output
blinding sums modulo the scalar order. The arithmetic proof binds these same
commitments, but it does **not** prove that a `P_i` belongs to an owned, unspent
ring member. Connecting the actual CLSAG selected member to that very commitment
is still mandatory future work.

## Bounds, binding and negative controls

The verifier constructs all layouts from public data only. Each reused input
layout is audited with the maximum of **every allowed bit range** on both sides
of every linear equation and every product, requiring each to be strictly below
the scalar order. The aggregate audit includes both bucket gaps and both
conservation sides. Sixteen u64 amounts/charges sum below `2^68`, but sixteen
outputs **plus a u64 fee can approach `17*(2^64-1)`**, requiring a 69-bit bound
before conservation is established. It is invalid to assume honest equality to
bound a potentially invalid assignment. A deliberately widened input range and
a scalar-order-offset witness are rejected by their respective bound/range
checks.

A separate inactive transcript domain binds version, fixture network/genesis,
parent, policy, body digest, exact height/expiry, s/base/fee/counts, and ordered
input parameters and fixture contexts. The commitment API additionally appends
the ordered input amount/charge and output commitments. The body digest and
context fields are opaque fixtures: this is **not** a canonical CT1 transaction
codec, authenticated ring record, proof of lineage or proof of parent-state
provenance. No claim is made that arbitrary externally supplied coefficients are
safe until the proposed authenticated derivation exists.

Four new tests exercise:

- Independent small-domain bucket partitions for all three s choices, octave
  transitions and neighbors through all 68-bit octaves, noncanonical endpoints,
  maximum aggregate rounding, rejected
  unaffordable/unrepresentable fixtures, count limits and unsafe bounds.
- Actual 1/1 and 16/16 proofs for each s, three randomized repetitions apiece;
  zero, maximum aggregate amount, independently saturated cancellation,
  one-unit cap difference, and asymmetric 1/16 and 16/1 boundary cases.
- Amount/charge/output-range equality omission controls, both bucket-bound
  omissions, and conservation omission. Each deliberately disconnected control
  verifies; restoring the constraint rejects an actual proof for the same
  in-range inconsistent witness. Only the conservation isolation test disables
  the independent group check, to ensure the circuit equation itself rejects.
  Proving never calls an integer-equation or group-balance oracle first.
- Same-arithmetic context replay with a missing-transcript control; changed fee,
  base, s, rate, amount/charge/output commitments, token, counts and ordering;
  incorrect openings and empty/truncated/appended proof encodings.

The original ten exact-kernel, malformed-proof, cap/max, generator and binding
tests remain. This finite suite is internal evidence, not a soundness proof or
adversarial audit. The first upper-bucket negative fixture accidentally used its
valid endpoint (512 for charge 500); it was corrected to 256 and now asserts the
intended out-of-bucket premise before proof generation. No constraints were
relaxed to make that test pass.

## Measurements

Recorded at base `31de92f619e5863c499957c08d0c2ef8f69ba1b4` on Apple M3 Ultra,
aarch64 macOS 27.0 (26A428), repository nightly
`rustc 1.93.0-nightly (646a3f8c1 2025-12-02)`, workspace release profile.
The host was not isolated. The standalone test process was measured with
`/usr/bin/time -l` after a separate release compilation, excluding rustc/Cargo
memory. Generator setup is timed separately; proof/verification timings include
statement/bound construction, with verifier group conservation included.
Fixture openings are seeded for reproducibility, but upstream proof and verifier
randomness remain untouched. Proof bytes are not deterministic golden vectors.

The [measurement artifact](combined-measurements.json) records all 18 observations,
source hashes and scope. Times below are min/median/max milliseconds across three
samples per cell, not population confidence intervals or worst-case bounds.

| Inputs/outputs | s | Prove ms | Verify ms |
| --- | --- | --- | --- |
| 1/1 | 2 | 196.14 / 197.50 / 197.74 | 15.11 / 15.14 / 15.19 |
| 1/1 | 3 | 195.70 / 198.16 / 198.38 | 14.91 / 15.04 / 15.92 |
| 1/1 | 4 | 196.46 / 197.21 / 198.26 | 14.96 / 14.99 / 14.99 |
| 16/16 | 2 | 2947.33 / 2955.66 / 2968.14 | 223.43 / 223.92 / 225.51 |
| 16/16 | 3 | 2946.08 / 2947.23 / 2950.63 | 223.23 / 223.45 / 228.06 |
| 16/16 | 4 | 2954.47 / 2969.97 / 2971.09 | 222.63 / 223.23 / 224.99 |

Generator construction (capacity 32,768) took 328.44 ms, outside proof timings.
The entire serial 14-test process passed in 46.66 seconds with zero ignored
or failed tests. Its maximum resident set was 545,275,904 bytes (520.02 MiB);
macOS also reported a 516,703,216-byte peak memory footprint. These are whole
process measurements including generators and all tests, **not** per-proof memory.
The final added large-octave assertions passed separately in 0.02 seconds; no
prover/verifier/measurement code changed after the recorded full run. Scoped
all-target clippy with `-D warnings`, formatting, and nine existing CT reference
vector tests also passed.

| Inputs/outputs | Multipliers | Linear constraints | R1CS bytes | Arithmetic subtotal bytes |
| --- | ---: | ---: | ---: | ---: |
| 1/1 | 1,612 | 3,248 | 1,121 | 1,158 |
| 16/16 | 23,752 | 47,843 | 1,377 | 1,894 |

The arithmetic subtotal is only `4-byte length + 1-byte count + 32*n charge
commitments + R1CS proof`. It excludes input pseudo commitments, output
commitments, **all public statement/ring records**, ciphertexts/contexts, CLSAG
signatures and remaining transaction framing. It is not an encoded or measured
complete proof section/transaction. Test assertions compare measured honest
proofs and allocation counts with proposed ceilings; this experiment does not
implement an external parser or enforce a 32KiB untrusted-byte admission cap.


Reproduce all tests:

```
RUSTC_WRAPPER= CARGO_TARGET_DIR=/tmp/botho-1307-target \
  cargo test --locked --release -p bth-ct-demurrage-reference \
  -- --nocapture --test-threads=1
```

The existing Workspace Build workflow already compiles and executes this
research crate in release with separate compilation/execution deadlines.

## Remaining acceptance

The candidate 65,536-multiplier/262,144-constraint/32KiB envelope is only a ceiling
for this experiment. Full public statement, commitments, rings, encryption,
ownership proofs, parser overhead, block admission and worst-case platform costs
remain unmeasured here. The exact-height transaction candidate also needs native,
WASM/mobile and Linux liveness evidence; one workstation timing cannot establish
that proving plus propagation fits a roughly five-second block interval.

Independent internal cryptographic review, actual CLSAG linkage, authenticated
factor/age/context derivation, D1/D3/base decisions, canonical transaction codecs,
side-channel-safe witness generation and migration remain open. BigUint and the
seeded fixture openings are test scaffolding, not wallet code. Any integration
requires its own reviewed checkpoint; this pilot activates nothing.
