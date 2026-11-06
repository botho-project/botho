# bth-ct-demurrage-reference

Inactive, test-only research crate: an exact-integer R1CS encoding of the CT
demurrage charge, plus randomized Bulletproofs prove/verify experiments. It is
not production proof code, consensus validation, or an accepted ADR. CI runs it
in `Workspace Build` (`cargo test --locked --release -p
bth-ct-demurrage-reference`).

## Current exact relation

For full-u64 amount, factor, elapsed and year, and a u32 rate:

```text
p      = clamp(factor, 1000, 6000) - 1000
charge = 0                                             if year = 0
         min(u64::MAX, floor(value * rate * p * elapsed
                            / (50_000_000 * year)))     otherwise
```

Capitalized reset is the positive difference of independently capped input and
output horizon charges. Monotonicity under factor clamping preserves the
production raw-factor early return. Settled spend is the maximum of accrued
and capitalized charge; the fixed spend horizon is checked against production
`SETTLEMENT_HORIZON_BLOCKS`. Differential tests and proof fixtures use the live
`bth_cluster_tax` functions. The frozen `src/legacy_kernel.rs`, including its
historical commentary, is retained only for an explicit old-versus-exact
short-hold discriminator: maximum amount, equal factors 6000, elapsed 1,
rate 200 and year 6,307,200 give exact charge **58,494,241,735**, legacy zero,
and capitalized difference zero. An actual arithmetic proof covers that case.

Each exact division uses `c64`, `excess147`, `remainder90` and `complement90`:
`N = D*(c+excess)+remainder`, `remainder+complement = D-1`, and
`c*excess = u64::MAX*excess`. Year zero uses public coefficient zero and
`D=1`. `Candidate` supplies canonical witnesses; the verifier's `CircuitLayout`
is constructed independently from public parameters. Tests bound both sides
of every equation for arbitrary ranged assignments, including invalid ones,
and generate in-range malformed proofs that the cryptographic verifier rejects.

The fixed research identifier `botho/inactive/demurrage-exact-9-v1` is bound in
the single-input and combined transcripts and in the ownership statement
digest, which feeds both arithmetic and indexed CLSAG messages. Mismatch
controls exercise all three paths. This introduces no selectable relation
mode or production protocol-version change.

The current allocation formulas are 1,557 range bits plus five products per
input: **1,562 multipliers and 3,140 constraints**, checked against actual
prover/verifier allocations. For `n` inputs and `m` outputs, the combined
circuit has `1562*n + 64*m + 136` multipliers and
`3140*n + 129*m + 275` constraints. Ownership uses the same arithmetic circuit.
Existing allocation and section-size ceilings remain unchanged.

## Historical evidence and pending work (#1914)

[RESOURCES.md](RESOURCES.md), [COMBINED.md](COMBINED.md),
[OWNERSHIP.md](OWNERSHIP.md), and their retained manifests, reviews and captures
remain historical evidence for the pre-9.0.0 quantized circuit. They have not
been regenerated or relabeled as exact-relation evidence.

The exact implementation is pending the independent relation review in #1953
and fresh retained resource measurements/publication in #1954. Local allocation
checks and finite production parity tests do not supply that review, a
cryptographic security proof, a Rust/R1CS equivalence theorem, or an activation
claim. No new Lean theorem or production policy change is included.
