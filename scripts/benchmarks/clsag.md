# Actual valid CLSAG library timings

This benchmark calls `bth_crypto_ring_signature::Clsag::sign` and
`Clsag::verify`, separately from the existing `RingMLSAG` benchmark. It changes
no cryptographic primitive, production policy, wallet codec or transaction
integration. Only ordinary valid synthetic library fixtures are constructed.

Ring 20 is the current transaction default/minimum in `transaction/clsag`.
Ring 2 is a library-only small-ring case, not an admitted transaction ring;
ring 32 is scaling sensitivity, not a claimed protocol maximum. The six
Criterion identities are `CLSAG sign/ring_size/{2,20,32}` and the corresponding
`CLSAG verify` identities.

## What the timer includes

Fixture construction is outside timing: token-0 project generators, synthetic
ring points, matching real private/public spend key at the middle position,
original amount commitment and same-value pseudo-output commitment with a
separate blinding. A deterministic seed identifies this workload; it is not
production key generation. Each case performs a full valid sign/verify preflight
and checks response count before timing.

The signing CSPRNG is separately seeded once through the existing
`bth_util_from_random::OsRng`, then advances across warmup and all measured
iterations. No nonce or per-iteration seed reset occurs. Timing includes public
signing, its RNG consumption, allocation, and dropping the returned signature.
OS seed acquisition, fixture creation and preflight are excluded. The public
sign API retains its amount-preservation check.

Verification holds one valid signature/fixture constant and repeatedly invokes
the actual public verifier, including decompression, hashing, arithmetic and
allocations. Every timed verification must succeed. This is warm repeated
verification, not cold-cache or diverse-network-transaction performance.
Black-box inputs prevent removing the work; verification failure is not silently
recorded as fast successful operation.

## Reproduce this target only

```
RUSTC_WRAPPER= CARGO_TARGET_DIR=/tmp/botho-clsag-bench \
  cargo bench --locked -p bth-crypto-ring-signature \
  --bench clsag_benchmarks -- --test

RUSTC_WRAPPER= CARGO_TARGET_DIR=/tmp/botho-clsag-bench \
  cargo bench --locked -p bth-crypto-ring-signature \
  --bench clsag_benchmarks -- --sample-size 20 --warm-up-time 1 \
  --measurement-time 3 --noplot
```

The benchmark workflow explicitly selects both MLSAG and CLSAG targets;
its retained evidence identifies MLSAG and CLSAG separately. Compilation success
or a tolerated failed quick step is not a timing observation. Local recording
uses the pinned nightly; workflow uses stable and must identify its actual
compiler independently.

## Recorded observation

Final source measurements and raw Criterion `new` JSON are recorded under
[`clsag-evidence/`](clsag-evidence/). The manifest binds source, lockfile, command,
compiler, platform and each raw file. Results are wall-time estimates on one
Apple M3 Ultra / aarch64 macOS 27.0 host, not an isolated performance lab.
Criterion confidence intervals describe this sample, not universal bounds.
An initial run overlapped local compilation and was discarded; the retained run
followed completion of compilation/clippy, with no concurrent work initiated by
this agent. Other system activity was not controlled.

No total transaction bytes, per-operation CPU/RSS, electrical power, energy,
network fanout, block admission or fee recommendation is measured here. CLSAG is
one component; these times do not complete confidential-amount integration or
establish that the sum of component medians is a transaction latency
quantile. Actual full-transaction/resource calibration remains outstanding.

| Operation | Ring | Median ms | 95% median interval ms |
| --- | ---: | ---: | ---: |
| sign | 2 | 0.4187 | 0.4164–0.4222 |
| sign | 20 | 4.1400 | 4.1329–4.1515 |
| sign | 32 | 6.6202 | 6.6117–6.6408 |
| verify | 2 | 0.4239 | 0.4232–0.4252 |
| verify | 20 | 4.1929 | 4.1759–4.2074 |
| verify | 32 | 6.6819 | 6.6728–6.7377 |
