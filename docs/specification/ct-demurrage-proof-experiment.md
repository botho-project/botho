# Inactive exact-demurrage R1CS proof experiment

This #1285 experiment generates and verifies actual randomized proofs for the
[exact integer construction](ct-demurrage-exact-constraints.md). It is entirely
inside the unpublished research crate's `cfg(test)` surface. It is **not** a
production proof implementation, transaction format, cryptographic audit,
accepted ADR, or protocol activation. The dependency is the existing vendored
Bulletproofs fork with its experimental R1CS feature; no dependency or randomness
API is patched by this experiment.

## Public statement and private witness separation

The [source](../../scripts/research/ct-demurrage/src/proof_experiment.rs) defines:

- `PublicStatement`: version, network identifier, context digest, token ID, raw
  input/output factors, elapsed blocks, rate, year, horizon, and compressed
  original amount/charge commitments;
- `CircuitLayout`: wire names/widths, public coefficients, and linear/product
  equations, built **only** from the public statement;
- `Witness`: private integer assignments, amount, charge, and two commitment
  openings. Only the prover receives it.

Prover and verifier call the same layout allocator, with `Some(witness)` and
`None` respectively. Verification takes the public statement and serialized
proof, not an amount, opening, quotient, remainder, cap selector, reference
`Candidate`, or synthetic zero-amount witness. Every proof fixture compares
layout wire widths, equations, products and external variable indexes exactly
with the independent integer reference. The original differential tests remain.

The layout preserves all three independent caps and the subtraction/max
complementarity gates. Every private integer is bit-ranged; the original
[full-domain bounds](ct-demurrage-exact-constraints.md#bounds-and-absence-of-scalar-field-wrap)
still apply. The prover checks witness vector length and bit widths before
allocation to avoid silent truncation of malformed host integers. It does not
call the integer equation checker during proof generation: negative tests
produce actual proofs for deliberately inconsistent in-range assignments and
require cryptographic verification to fail.

## Commitment generators and transcript

Amount generators come from
[`bth_crypto_ring_signature::generators(token_id)`](../../crypto/ring-signature/src/ring_signature/mod.rs).
The project amount generator is not Bulletproofs' default generator. Both base
points cross dalek 5 to dalek 4 through canonical compressed Ristretto encoding,
matching the conversion strategy in
[`transaction/core` range proofs](../../transaction/core/src/range_proofs/mod.rs).
Scalar openings cross through canonical scalar bytes. Tests compare both base
point encodings and project `CompressedCommitment::new` bytes with the converted
Bulletproofs generators, for token IDs 0, 1 and u64::MAX and amounts 0, 1 and
u64::MAX. Each real proof checks that R1CS `Prover::commit` produces the supplied
project amount and charge commitment bytes. The amount bit sum is constrained
to that original amount commitment; it is not replaced by a post-charge amount.

The complete path uses fixed transcript domain
`botho/inactive/ct-demurrage-r1cs`, with separately labeled fields:

| Label | Encoding |
| --- | --- |
| `experiment-version` | u64 little endian (fixture version 1) |
| `network`, `context` | 32 bytes each |
| `token` | u64 little endian |
| `input-factor`, `output-factor` | raw u64 little endian |
| `elapsed`, `year`, `horizon` | u64 little endian |
| `rate` | u32 API value encoded as u64 little endian |
| `amount-commitment`, `charge-commitment` | 32 compressed bytes each |

The same append function runs on both sides. R1CS's own commitment API also
appends the commitment points; the explicit commitment append is redundant
binding, whereas the equalities connecting those points to circuit bit sums are
necessary. Token selects the generator and is also transcript-bound. Network
and context values are test fixtures, not a specification of real transaction
context derivation. The verifier experiments with bound version numbers; it
does not implement a production version-support policy.

`prove` and `verify` always use complete bindings. Explicitly named test control
helpers can omit statement transcript fields or external equalities solely to
demonstrate the resulting failure. There is no production entry point.

## Actual rejection and omission-control evidence

Eight valid cases use actual Rust demurrage outputs: ordinary, zero rate, zero
time, zero year, nested-floor boundary, full-domain u128 multiplication overflow,
independently capped cancellation, and one-cap difference equal to one.

The negative tests cover:

- Nine individually altered fields: version, network, context, input/output
  factors, elapsed, rate, year and horizon. The selected mutation pairs have
  **identical circuit coefficients**, so rejection cannot be credited to an
  incidental arithmetic change. With the complete transcript, all replays fail.
  With deliberately omitted statement binding, each replay succeeds. These
  controls retain R1CS's intrinsic commitment binding.
- Changed token, wrong input/charge commitments, an invalid commitment encoding,
  empty/truncated proofs, one corrupted proof byte, and appended proof data.
- Individually omitted amount and charge equalities. The prover commits to a
  different known value while retaining an otherwise valid original witness.
  The detached commitment control verifies; restoring the equality produces a
  proof that fails verification.
- Coordinated invalid cap, difference and max witnesses, plus invalid annual
  quotient and time remainder. Each stays within bit bounds; no integer oracle
  precheck stops the experiment. Each generates proof bytes and then fails
  cryptographic verification.

These are concrete regressions for the implementation, not an adversarial
security proof. In particular, the malformed-proof sample is not fuzzing or a
complete parser audit.

## Measurements and reproduction

```
CARGO_TARGET_DIR=/tmp/botho-1281-target RUSTC_WRAPPER= \
  cargo test --locked -p bth-ct-demurrage-reference --release \
  -- --nocapture --test-threads=1
```

One recorded local run used Apple M3 Ultra, aarch64-apple-darwin, macOS 27.0,
`rustc 1.93.0-nightly (646a3f8c1 2025-12-02)` (repository nightly-2025-12-03),
workspace release profile, and serial Rust test execution. The host was not
isolated for benchmarking. Generator setup is outside each timed proof/verify
call; statement/layout construction inside those calls is included.

| Case | Proof bytes | Prove ms | Verify ms |
| --- | ---: | ---: | ---: |
| ordinary | 1121 | 204.832 | 16.287 |
| zero rate | 1121 | 206.037 | 16.223 |
| zero time | 1121 | 205.931 | 16.232 |
| zero year | 1121 | 673.061 | 251.753 |
| floor boundary | 1121 | 464.403 | 16.183 |
| maximum domain | 1121 | 204.951 | 16.175 |
| both caps cancel | 1121 | 205.108 | 16.233 |
| one-cap difference one | 1121 | 204.397 | 15.908 |

All eight allocate **1,412 multipliers and 2,844 linear constraints**, matching
the reference census on both prover and verifier. `BulletproofGens` has capacity
2,048 and one party. The measured 1,121 bytes are only serialized `R1CSProof`
bytes; they exclude public statement, commitments, and any transaction framing.
These are single observations with visible timing variation, not median or
worst-case estimates, throughput claims, or deployment sizing. The ten-test
release run, including negative controls and original arithmetic tests, passed
in 6.79 seconds. The same ten tests passed in the default CI test profile in
20.49 seconds; strict scoped clippy and formatting checks also passed.

Fixture openings come from a fixed seeded ChaCha RNG **only in tests**. The
vendored prover internally obtains `thread_rng` randomness, as does its
verifier; neither API accepts an injected RNG. Proof bytes are therefore
randomized, not reproducible byte fixtures. No dependency modification attempts
to make them deterministic. Reproducing this run means verifying equivalent
relations and observing proof lengths, not reproducing identical proof bytes.

The existing Workspace Build gate executes this research package's tests. All
new dependencies are dev-only and reuse versions already in the workspace lock.

## Recommendation and unresolved work

The experiment supports taking the exact quotient/remainder plus
complementarity circuit to independent cryptographic and implementation review.
It does not resolve D1 quantization, D2 factor policy, D3 tag-derived bounds,
private factor/age proofs, multi-input conservation, actual transaction-context
construction, version/activation policy, wire format, side channels, proof
aggregation, or wallet witness generation. BigUint witness work and fixed test
openings are not production code. Successful round trips and rejected mutations
are internal evidence only; no zero-knowledge, soundness, or mainnet-readiness
claim follows from them alone.
