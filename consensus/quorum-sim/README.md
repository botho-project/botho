# bth-quorum-sim

Static quorum-health analyzer for Botho's curated FBAS federation (Path A).

This is **Deliverable 2 of #510**, scoped to **Path A** (a curated validator
federation + thin clients; see #427). It is a small, Botho-grounded
re-implementation of the static metrics computed by tools such as
[`fbas_analyzer`](https://github.com/wiberlin/fbas_analyzer) and
[`python-fbas`](https://github.com/nano-o/python-fbas), tied directly to
Botho's threshold rule.

## Scope (v1)

Static analysis + threshold/growth comparison. **No** dynamic message-level SCP
round simulator, Byzantine equivocation injection, or partial-synchrony
modelling — that is an explicit follow-up.

The curated federation is small (`N ≤ ~20`), so every analysis brute-forces
over the `2^N` node subsets. This is exponential but exact, and trivially fast
at the targeted sizes.

## What it computes

- `is_quorum` — the FBAS quorum predicate.
- `has_quorum_intersection` — do all quorums pairwise intersect? `false` ⇒ a
  fork is possible.
- `minimal_quorums` — smallest quorums by set inclusion.
- `minimal_blocking_sets` — smallest crash-fault sets that halt the network
  (the **liveness** buffer).
- `minimal_splitting_sets` — smallest Byzantine sets that can fork the network
  (the **safety** buffer).
- Threshold-rule comparison: Botho's `n − floor((n−1)/3)` vs `ceil(0.67·n)` vs
  unanimity.
- Growth/churn: curated admission and reactive-shun, flagging any action that
  breaks quorum intersection.

## CLI

```
# Threshold-rule comparison table (N = 2..=12)
cargo run -p bth-quorum-sim --bin quorum-sim -- compare --min 2 --max 12

# Full static-health report for one symmetric federation (JSON for CI)
cargo run -p bth-quorum-sim --bin quorum-sim -- analyze --n 4 --threshold 2 --json

# Growth/churn timeline: start at 3, admit 2, then shun node 0
cargo run -p bth-quorum-sim --bin quorum-sim -- churn --initial 3 --admit 2 --shun 0
```

All subcommands accept `--json` for machine-readable output.

## Grounding

- `botho/src/config.rs` — `QuorumConfig::effective_threshold` (the formula under
  test) and `test_quorum_effective_threshold_*`.
- #510 research (Threads A+B) — metric definitions and expected values.
