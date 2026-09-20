# Bounded complete-composition resource matrix

Issue #1348 adds seven legitimate, test-only cases to the complete public-library
[ownership harness](OWNERSHIP.md), using s=2 and ring20. It does not change
arithmetic relations, CLSAG internals, public binding, production code, policy,
or activation. The historical ownership/combined measurements retain their
original source hashes and are not relabelled as this run.

Each case runs in a fresh compiled test executable process. Three ordinary
cases have three actual prove/verify samples apiece; four boundary cases have
one each: **13 composed proofs total**. A separate generator-only process is an
additional baseline observation. Compilation is outside measurement. Setup and
fixture/branch-premise checking are outside prove/verify timings. Generation
includes the arithmetic proof, public digest work, OS-seeded signing RNG and
all ordinary CLSAG signatures; verification includes arithmetic, public shape
checks, group conservation and every ownership signature. No signatures,
constraints or balance checks are bypassed.

The approved boundary cases are zero, both independently capped capital terms
cancelling, one capped capital term exceeding the other by exactly one, and
sixteen maximum u64 amounts with zero charge. Tests assert these branch premises
before proving. All cases are representable and affordable under their fixture
parameters. The maximum-sum case does not claim arbitrary maximum-domain
parameters are affordable. Ordinary fixtures retain their earlier values;
maximum-value decoys use a checked fallback to avoid overflowing u64.

## Local observations

Recorded on Apple M3 Ultra, macOS/arm64, repository nightly 2025-12-03, workspace
release profile. The machine was not isolated. Prove/verify columns show
min–max over three samples for ordinary cases and one observation for boundaries.

| Case | Inputs / outputs | Prove ms | Verify ms | Whole-case peak RSS MiB |
|---|---|---|---|---|
| ordinary-1 | 1 / 1 | 218.08–219.10 | 20.92–21.18 | 26.17 |
| ordinary-4 | 4 / 4 | 821.23–826.23 | 77.75–80.13 | 75.98 |
| ordinary-16 | 16 / 16 | 3171.97–3202.23 | 301.63–308.00 | 286.00 |
| zero | 1 / 1 | 211.79 | 20.07 | 21.70 |
| both-caps-cancel | 1 / 1 | 211.08 | 19.75 | 21.38 |
| one-cap-difference | 1 / 1 | 211.27 | 20.54 | 23.52 |
| maximum-sum | 16 / 16 | 3175.28 | 302.02 | 182.81 |

Generator-only whole-process RSS was 12.84 MiB. Full-case RSS includes generators,
fixtures, libtest and all samples, including any allocator retention. It is not
per-proof memory. The lower one-sample maximum-sum RSS versus the three-sample
ordinary-16 process does not demonstrate a cheaper worst-case proof. Do not
subtract RSS peaks to infer an exact prover allocation.

| Inputs / outputs | Multipliers | Constraints | Arithmetic proof bytes | CLSAG field bytes |
|---|---:|---:|---:|---:|
| 1 / 1 | 1612 | 3248 | 1121 | 736 |
| 4 / 4 | 6040 | 12167 | 1249 | 2944 |
| 16 / 16 | 23752 | 47843 | 1377 | 11776 |

Prover/verifier metrics agree in every case. Arithmetic allocations are below
the proposed 65,536-multiplier / 262,144-constraint ceilings. These are actual
circuit counts, not a claimed operation count for CLSAG or complete verification.
Proof and signature-field bytes remain separate. They do not measure an encoded
32KiB proof section or a complete 100KiB transaction: public rings, commitments,
contexts, encrypted fields and framing are not included. No external parser or
byte/block-work admission implementation is added. No safe platform bound,
mobile/WASM feasibility, exact-height five-second liveness, consensus guarantee,
cryptographic security proof or policy ratification follows from this run.

## Runner and provenance

The checked-in [local summary](evidence/ownership-resources-macos/summary.json)
contains every sample, setup/fixture times, source and executable SHA256s,
checkout identity, dirty status, tracked diff digest, environment, compile recipe,
commands, exit statuses and raw log/RSS hashes.
Raw outputs are retained beside it. Source hashes identify the working source;
`checkout_commit` identifies its parent checkout during collection, not a claim
that uncommitted new files were in that commit. This is reproducible measurement
provenance, not a hermetic or independently attested build.

The runner selects the test executable from the preceding Cargo JSON artifact,
not a directory scan that might choose a stale executable. The captured Cargo
stdout and executable hashes tie this observation to the separately executed
compile invocation; the workflow compiles and measures the same checkout in
sequence. The recorded recipe is documentary, not proof that arbitrary
caller-supplied Cargo output came from those exact flags. Its exact test filter
uses `--ignored` only for the single explicitly named resource test; it never
runs a blanket ignored suite. Existing ordinary ownership regressions continue
to run in normal tests. Output directories must be new, preventing stale result
merging. macOS `/usr/bin/time -l` reports RSS bytes; Linux `-v` reports KiB, which
are converted explicitly to bytes. Missing RSS, failed execution, incomplete
samples and timeout remain failures; no successful sample is fabricated.

Each case has a 120-second deadline, and the matrix has a 300-second deadline.
Timeout kills and waits for the whole process group, including the timing
wrapper's child. Compilation is separately bounded in CI. The Linux workflow
runs only this matrix and the fast collector checks, preserves outcomes even on
failure, and labels artifacts with run ID/attempt. **Linux measurements remain
pending until the new workflow runs successfully on the published head**; this
local report makes no Linux performance claim. CI artifacts expire after 14 days
and should be downloaded for a reviewed cross-platform report.

Reproduce from the repository root, using a new output directory:

```sh
RUSTC_WRAPPER= CARGO_TARGET_DIR=/tmp/botho-1346-target \
  cargo test --locked --release -p bth-ct-demurrage-reference --no-run \
  --message-format=json > /tmp/ownership-build.jsonl
python3 scripts/research/ct-demurrage/measure_ownership.py \
  --cargo-messages /tmp/ownership-build.jsonl --output-dir /tmp/ownership-results
```

The fixed case list and sample counts are not CLI knobs. New cases or budgets
require a reviewed source change. The remaining parent #1307 review, policy and
integration gates stay open.
