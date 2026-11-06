# DRT regression fixtures

One file per DRT leg, `<leg>.jsonl`, one JSON object per line with at least an
`input` field (the leg's input type, as serialized by `formal-drt/src/legs.rs`).
Every run replays every line of every selected leg before generating new cases.

The runner appends a line here, shrunk, whenever the Lean model and the Rust
code disagree (unless `--no-write-regressions`). Commit such a line together
with the fix, or with the bug issue it reproduces, so the case is replayed
from then on.

The lines committed with #1707 were found by the mutation check (see the
`note` field): each one disagrees with a mutated Rust function and agrees with
the real one. They pin the boundaries those mutations broke.

## Timestamps

`timestamps.jsonl` (#1942) pins 30 expected outputs for the five pure production
helpers. Inputs require `window` (oldest first), `parent`, `stamp`, `minter_now`
and `validator_now`; every scalar and window element is a decimal u64 string.
Unknown/missing fields, JSON numbers, malformed decimal strings and overflow
are rejected. Replay accepts arbitrary finite windows without pre-truncation;
generation biases lengths 0/1/5/6/10/11/12/32 within 0..32 and retains an
independent arbitrary-u64 branch. Codec tests also pass 64-entry windows.

Each `expected` object has `mtp`, `next_min`, `honest_stamp`, complete `rolled`,
nullable `observed_gap`, and booleans `adapter_ledger_candidate` and
`adapter_ledger_honest`. Output integers are strings. The module test asserts
every expected object against actual helper/adapter outputs; the normal runner
reads only `input` and independently compares Rust with the landed Lean model.

Pinned cases are `empty_parent_zero`; `length_one_gap_two`,
`length_five_unsorted_backward`, `length_six_mtp_equality`,
`length_ten_ascending_gap_two`, `length_eleven_ascending_gap_three`,
`length_twelve_ignored_prefix_gap_four`, `overlong_unsorted_ignored_prefix_rolling`;
`duplicate_equal_gap`, `single_high_outlier`; `minter_{below,at,above}_minimum`;
`future_cap_equality`, `future_cap_plus_one`, `future_clock_plus_one`;
`above_2pow53_exact`; `clock_u_minus_60`, `clock_u_minus_59`,
`clock_u_minus_one`, `clock_u_max`, `u_minus_one_successor` and
`u_max_saturated_successor_rejects`; and `zero_skew_step_0` through `_6`.

The seven linked snapshots check six candidate-accepted stamps of 1060 at
validator clock 1000 from empty, then MTP 1060 and rejected honest stamp 1061
with both next clocks 1000. Tests assert every rolling-state link. This is a
timestamp-rule/adapter history; earlier stamps are not asserted honestly
produced, and full ledger, SCP pacing and physical-clock reachability are not
established. The two adapter fields use the ledger's saturating cap after the
actual MTP/helper calls. They do not execute ledger or minting admission checks;
#1943 owns that residual. Plain minting-validator `now+60` agreement is confined
to `now <= U-60`; intrinsic validation has only the upper bound.

Retained validation: 10,000 generated timestamp cases at seed 1915 plus all
30 replays; all-leg PR tier, 10,000 cases per discovered leg at seed 1707.
Every generated timestamp case is classified by window size, gap, successor
and future saturation, and both adapter outcomes. The isolated negative
control changed only the post-helper `Some(3)` result to `Some(4)` and failed
on `length_one_gap_two`; byte-identical restored source passed the same replay.
No mutation mechanism remains. Current reports, coverage, source/binary hashes
and exact commands are retained in `/tmp/botho-1942-isolated-evidence.json` and
`/tmp/botho-1942-isolated-builder-receipt.json`, with the
[validation correction](https://github.com/botho-project/botho/pull/1950#issuecomment-6678440600).
These measurements used source commit `ebcee54f869a0316ec514cce54207b56d1d64b81`
(tree `f74a68063cbac2d0f5572b1e82d6bcc7ea574878`); the subsequent correction
changes only this README. Earlier shared-target results are superseded history
and are not verdict-bearing evidence.

To reproduce, select a private Cargo target according to
[the isolation recipe](../../.loom/roles/cargo-target-isolation.md). Keep
`CARGO_TARGET_DIR` if Loom assigned it under the repository's `.loom/targets/`
or it matches this worktree's `.loom-cargo-target-dir` marker. Otherwise,
including an inherited shared cache, use the worktree-owned fallback below.
The retained isolated run used this fallback, with no marker or target symlink.
Only one agent may build into the selected target during the run.

```sh
# Run only when no Loom-owned target was assigned to this run:
export CARGO_TARGET_DIR="$(git rev-parse --show-toplevel)/target"
```

With that private target selected, run from this worktree (Lean 4.35.0,
Rust `nightly-2026-06-09`, two Cargo jobs, checked-overflow release profile):

```sh
WORKTREE_ABS="$(git rev-parse --show-toplevel)"
: "${CARGO_TARGET_DIR:?Select the private target described above}"
export CARGO_TARGET_DIR CARGO_BUILD_JOBS=2
(cd "$WORKTREE_ABS/formal-drt/lean" && elan run leanprover/lean4:v4.35.0 lake build)
cargo +nightly-2026-06-09 build --release --manifest-path "$WORKTREE_ABS/formal-drt/Cargo.toml"
runner="$CARGO_TARGET_DIR/release/botho-formal-drt"
model="$WORKTREE_ABS/formal-drt/lean/.lake/build/bin/botho-model"
regressions="$WORKTREE_ABS/formal-drt/regressions"
"$runner" --model "$model" --regressions "$regressions" --legs timestamps --seed 1915 --cases 10000 --no-write-regressions --json /tmp/botho-1942-reproduced-timestamps-1915.json
"$runner" --model "$model" --regressions "$regressions" --seed 1707 --cases 10000 --no-write-regressions --json /tmp/botho-1942-reproduced-all-1707.json
```

The output filenames above keep the retained reports intact. Runner hashes
identify a particular build: ordinary builds embed build time, so record the
new hash rather than expecting a rebuild to match the retained binary.
Nightly discovers this same single leg through `--list`; no separate list is
maintained. Finite agreement is evidence for the helpers, not a proof of Rust
equivalence or random coverage of complete production acceptance.
