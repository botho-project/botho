# Benchmark evidence

The Benchmarks workflow runs separate **RingMLSAG** (11/16/32 plus batches)
and **Clsag** valid-library signing/verification (2/20/32) targets. Ring 20 is
the current transaction default/minimum; 2 is library-only and 32 a scaling
case, not an asserted protocol maximum. See [CLSAG measurements](clsag.md).
Transaction-core benches exercise range proofs and legacy RctBulletproofs
fixtures; no complete CT1 cost or network energy estimate is implied.

Each execution job removes only restored `target/criterion` reports before its
unchanged benchmark commands. It always attempts to upload current Criterion
reports plus `benchmark-evidence/manifest.json`, with 14-day retention and a
job/run/attempt-specific name. Compile-only jobs supply no timing artifact.
Canceled or failed setup can prevent collection/upload entirely: absent artifacts
mean missing evidence, never successful measurements.

The manifest records the actual checkout SHA (which can be a PR merge ref), PR
head, run/attempt/job, mode, actual rustc/cargo output, platform, source/lock hashes,
raw step outcomes and SHA256/size/JSON validity of Criterion files. No environment
dump, credentials or secrets are collected. `step.outcome` preserves failures
hidden by `continue-on-error`; the existing quick-mode tolerance is unchanged.

Statuses distinguish missing reports, partial/unsuccessful execution, incomplete
Criterion files and successful steps with measurements. Even the last status is
**not a completeness or performance attestation**: inspect the per-benchmark
names, `new/estimates.json` and `new/sample.json`, expected benchmark selection and
raw outcomes. Cached `base` or `change` files alone are not fresh observations.
Different node suites share their job's fresh report directory, so inspect each
step rather than attributing all files to a successful neighboring suite.

Quick versus full Criterion measurements, stable compiler versions, runner
hardware and load can differ; preserve metadata when comparing. Wall-time
estimates are not CPU time, per-operation memory, calibrated electrical power,
network replication or real monetary cost. Performance targets printed by the
workflow are documentation, not automated threshold checks.

Collector tests use synthetic empty/partial/malformed/success artifacts:

```
python3 -m unittest discover -s scripts/benchmarks -v
```

No cryptographic benchmarks need to be repeated to test evidence handling.

Explicit `--bench` target selection is required when forwarding Criterion CLI
flags. Hosted run 35538870413 exposed `Unrecognized option: 'quick'` when the
package-wide command dispatched a libtest harness before Criterion. Crypto and
transaction steps now name their Criterion targets in both quick and full modes.
Local `--test` smoke validates target selection without repeating full benchmark
measurements; retained hosted measurements still require a successful fresh run.
