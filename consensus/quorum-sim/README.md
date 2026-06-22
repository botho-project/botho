# bth-quorum-sim

Quorum-health analyzer **and** dynamic SCP simulator for Botho's curated FBAS
federation (Path A).

Two layers:

1. **Static analyzer** (#511/#512) — a small, Botho-grounded re-implementation
   of the metrics computed by tools such as
   [`fbas_analyzer`](https://github.com/wiberlin/fbas_analyzer) and
   [`python-fbas`](https://github.com/nano-o/python-fbas), tied directly to
   Botho's threshold rule.
2. **Dynamic message-level SCP simulator** (#514) — a round-based agreement
   engine that *runs* a nomination + accept-lock + commit protocol over the same
   FBAS model under faults and a configurable network, and empirically detects
   **forks** (safety violations) and **stalls** (liveness violations). It is the
   validate-before-landing tool for the #427 proposer-model decision (drop
   competing-coinbase → explicit leader election).

The curated federation is small (`N ≤ ~20`), so every static analysis
brute-forces over the `2^N` node subsets. This is exponential but exact, and
trivially fast at the targeted sizes.

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

## Dynamic simulator (#514)

The `simulate` subcommand runs the message-level engine over many seeds and
reports per-config **fork** (safety) / **stall** (liveness) counts,
rounds-to-decide, and leadership fairness.

- **Proposer models**: `competing-coinbase`, `hash-priority-leader`,
  `round-robin-leader`, `vrf-leader` (omit `--proposer` to sweep all four).
- **Fault kinds**: `crash` (silent) and `equivocate` (Byzantine: sends different
  values to different peers — the fork-inducing adversary).
- **Network**: synchronous (default) or partially-synchronous via `--max-delay`
  / `--drop-prob` (both seeded ⇒ reproducible).
- **Quorum oracle**: commit decisions delegate to the static `Fbas::is_quorum`,
  so dynamic outcomes cross-check the static splitting/blocking-set predictions.

Key empirical guarantees (encoded as tests in `tests/dynamic.rs`): with
equivocators **below** the static minimal splitting set, no run ever forks; at
the splitting set, a leader-equivocation fork **is** observed; unanimity below 4
nodes stalls on a single crash; and a given `(config, seed)` is bit-for-bit
reproducible.

### SCP simplifications

This is simulation/test tooling, not production consensus. It collapses SCP's
accept→confirm into a single **accept-lock**, models **one slot + one leader per
run** (fairness measured across seeds), and has **no leader-timeout recovery**
(a crashed leader is survived via a deterministic fallback, but a *Byzantine
leader stalls* the slot). See the module docs in `src/sim.rs` for the full list.

## CLI

```
# Threshold-rule comparison table (N = 2..=12)
cargo run -p bth-quorum-sim --bin quorum-sim -- compare --min 2 --max 12

# Full static-health report for one symmetric federation (JSON for CI)
cargo run -p bth-quorum-sim --bin quorum-sim -- analyze --n 4 --threshold 2 --json

# Growth/churn timeline: start at 3, admit 2, then shun node 0
cargo run -p bth-quorum-sim --bin quorum-sim -- churn --initial 3 --admit 2 --shun 0

# Dynamic simulation: n=4, one Byzantine equivocator, partial synchrony,
# sweeping all proposer models over 300 seeds
cargo run -p bth-quorum-sim --bin quorum-sim -- simulate \
    --n 4 --faulty 0 --fault equivocate --max-delay 3 --drop-prob 0.2 --seeds 300

# Two equivocators (= splitting set) → leader-equivocation forks appear
cargo run -p bth-quorum-sim --bin quorum-sim -- simulate \
    --n 4 --faulty 0 --faulty 2 --fault equivocate --seeds 300
```

All subcommands accept `--json` for machine-readable output.

## Grounding

- `botho/src/config.rs` — `QuorumConfig::effective_threshold` (the formula under
  test) and `test_quorum_effective_threshold_*`.
- #510 research (Threads A+B) — metric definitions and expected values.
