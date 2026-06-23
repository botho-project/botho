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
- Threshold-rule comparison: Botho's `n − floor((n−1)/3)` vs `ceil(2n/3)` vs
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
- **View-change / leader-timeout** (#519): `--view-change` enables SCP-style
  leader rotation; `--view-budget N` sets the per-view round budget (default 4).
  Omit `--view-change` to run the v1 behavior (no rotation) for comparison.
- **Quorum oracle**: commit decisions delegate to the static `Fbas::is_quorum`,
  so dynamic outcomes cross-check the static splitting/blocking-set predictions.

Key empirical guarantees (encoded as tests in `tests/dynamic.rs`): with
equivocators **below** the static minimal splitting set, no run ever forks; at
the splitting set, a leader-equivocation fork **is** observed (even with
view-change on — it is not masked); unanimity below 4 nodes stalls on a single
crash; and a given `(config, seed)` is bit-for-bit reproducible (with and
without view-change).

### View-change / leader-timeout (#519)

The ratified production proposer design (#427) is **round-robin leader election
WITH mandatory leader-timeout / view-change**. The v1 engine had no view-change,
so a *Byzantine (equivocating) leader* could stall its own slot — leader models
stalled ~15% (29/200) under an equivocating leader. View-change closes exactly
this gap:

> If the current view's leader has not driven the slot to a decision within
> `--view-budget` rounds, the **view** advances and the leader is **rotated
> round-robin** to `(base_leader + view) % n`; the slot is retried under the new
> leader. Undecided, not-yet-locked nodes follow the new leader's value, so a
> stalled (Byzantine or crashed) leader is rotated out and liveness is restored.

**Safety is preserved exactly.** View-change only changes which leader an
as-yet-undecided node follows; it never unwinds an `accept`-lock or a commit. A
correct node's vote is pinned to its accepted value forever, so once a node
accepts (let alone commits) it keeps that value across every view. Two correct
nodes therefore still cannot commit different values unless the Byzantine set
reaches the splitting threshold — the same quorum-intersection argument as
without view-change. The fork at/above the splitting set is **still observed**
with view-change on (it is not papered over). View-change is bounded by
`--max-rounds` (each view consumes ≥1 round), so the simulation still terminates.

**Empirical result** (n ∈ {4,7,10}, equivocating leader, 200 seeds):

| config                          | forks | stalls         |
|---------------------------------|-------|----------------|
| round-robin, **no** view-change | 0     | ~29/200 (~15%) |
| round-robin, **with** view-change | 0   | **0/200 (0%)** |

Run the comparison yourself:

```
# WITHOUT view-change — Byzantine leader stalls ~15% of slots
cargo run -p bth-quorum-sim --bin quorum-sim -- simulate \
    --n 7 --proposer round-robin-leader --faulty 0 --fault equivocate --seeds 200

# WITH view-change — stalls collapse to ~0, still 0 forks
cargo run -p bth-quorum-sim --bin quorum-sim -- simulate \
    --n 7 --proposer round-robin-leader --faulty 0 --fault equivocate --seeds 200 \
    --view-change --view-budget 4
```

The `vc` column in the table shows `off` or `v<budget>`.

### Coinbase churn: pinned vs unpinned (#535, simulation arm of #532)

Botho's `competing-coinbase` proposer is also its **reward mechanism**: each
validator's RandomX miner produces a fresh, higher-PoW-priority coinbase several
times a second, and SCP's `combine_fn` keeps the single highest-priority coinbase
per slot (`botho/src/consensus/service.rs`). If every node kept swapping its
nominated coinbase mid-slot, the candidate set would never stabilize into a
shared confirmed-nominate and the slot would **jam** (#419/#417). Production
fixed this by **pinning** the first-proposed coinbase per slot
(`propose_pending_values`).

The simulator models this directly:

- `--churn-rate R` — per-round probability `R ∈ [0,1]` that a node's miner
  produces a fresh, strictly higher-priority coinbase mid-slot. `0` (default) is
  the original churn-free behavior. Only affects `competing-coinbase`.
- `--pin-coinbase` (default `true`, the #419 production fix) — keep the FIRST
  coinbase per slot. `--pin-coinbase=false` is the pre-#419 bug: re-nominate each
  newly-mined higher coinbase, so the candidate set never settles.

With churn active, the competing-coinbase combiner is faithful to service.rs:
priority == value id, so the champion picks the **highest** value heard. Churn is
value-selection only — it never touches the accept/commit machinery — so it
cannot affect safety (zero forks in both modes with no faults).

**Empirical result** (n=4, 3-of-4, 300 seeds; the headline #535 deliverable):

| network        | churn | **unpinned** stall % | **pinned** stall % | forks (both) |
|----------------|-------|----------------------|--------------------|--------------|
| sync           | 0.60  | 0.0%                 | 0.0%               | 0            |
| psync(delay 3) | 0.10  | 3.3%                 | 0.0%               | 0            |
| psync(delay 3) | 0.30  | 23.3%                | 0.0%               | 0            |
| psync(delay 3) | 0.60  | 82.0%                | 0.0%               | 0            |
| psync(delay 3) | 0.90  | 99.7%                | 0.0%               | 0            |

Stalls also grow with message **delay** (unpinned, churn 0.5): delay 1 → 22.0%,
delay 3 → 62.3%, delay 6 → 85.0%. With **one crash** below the blocking set
(n=4, churn 0.3, delay 3): unpinned 55.0% vs pinned **0.0%**. The jam needs
**asynchrony** — under pure synchrony even unpinned churn converges (every node
hears the global max within one round), which is why the real #419 stall shows up
on the live, delayed testnet rather than in lock-step tests.

**Conclusion for #532**: pinned competing-coinbase shows **~0 stalls across the
entire churn × delay × 1-crash stress range** while unpinned reproduces the
#419 jam (up to 99.7%). The production pinning fix is sufficient for
competing-coinbase liveness here, so a view-change escape-hatch is not required
for this failure mode (it remains a rare backstop at most).

Run the comparison yourself:

```
# UNPINNED (pre-#419 bug) — slot jams under churn + delay
cargo run -p bth-quorum-sim --bin quorum-sim -- simulate \
    --n 4 --proposer competing-coinbase \
    --churn-rate 0.6 --pin-coinbase=false --max-delay 3 --seeds 300

# PINNED (production #419 fix) — stalls collapse to 0, still 0 forks
cargo run -p bth-quorum-sim --bin quorum-sim -- simulate \
    --n 4 --proposer competing-coinbase \
    --churn-rate 0.6 --pin-coinbase=true --max-delay 3 --seeds 300
```

The table's `churn` / `pin` columns show the active settings (`-` for the leader
models, which are unaffected).

### SCP simplifications

This is simulation/test tooling, not production consensus. It collapses SCP's
accept→confirm into a two-phase **accept-lock + confirming-quorum commit**,
models **one slot per run** (fairness measured across seeds), models
**leader-timeout / view-change** as an optional round-robin leader rotation
(`--view-change`); with view-change off, a crashed leader is still survived via a
deterministic fallback but a *Byzantine leader stalls* the slot; and models
**coinbase churn** (`--churn-rate` / `--pin-coinbase`) on the competing-coinbase
proposer (priority == value id, highest wins, mirroring service.rs). See the
module docs in `src/sim.rs` for the full list.

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

# Round-robin + view-change vs the equivocating leader (the #427 validation):
# stalls collapse to ~0 while forks stay 0
cargo run -p bth-quorum-sim --bin quorum-sim -- simulate \
    --n 7 --proposer round-robin-leader --faulty 0 --fault equivocate \
    --seeds 200 --view-change --view-budget 4

# Coinbase churn (#535): unpinned reproduces the #419 jam, pinned (production) fixes it
cargo run -p bth-quorum-sim --bin quorum-sim -- simulate \
    --n 4 --proposer competing-coinbase --churn-rate 0.6 \
    --pin-coinbase=false --max-delay 3 --seeds 300   # unpinned → ~82% stalls
cargo run -p bth-quorum-sim --bin quorum-sim -- simulate \
    --n 4 --proposer competing-coinbase --churn-rate 0.6 \
    --pin-coinbase=true --max-delay 3 --seeds 300    # pinned   → 0% stalls
```

All subcommands accept `--json` for machine-readable output.

## Grounding

- `botho/src/config.rs` — `QuorumConfig::effective_threshold` (the formula under
  test) and `test_quorum_effective_threshold_*`.
- #510 research (Threads A+B) — metric definitions and expected values.
