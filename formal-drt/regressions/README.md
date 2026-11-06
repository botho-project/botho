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
No mutation mechanism remains. Reports, coverage, source/binary hashes and
commands are retained in `/tmp/botho-1942-evidence.json` and the linked
`/tmp/botho-1942-*.json`/`.log` files; the Builder receipt links the final PR.

Reproduce after the documented Lean/runner build, from `formal-drt/` (the
retained run used `CARGO_TARGET_DIR=/Volumes/Stripe/cargo-target`,
`CARGO_BUILD_JOBS=2`, and Rust `nightly-2026-06-09`):

```sh
runner=/Volumes/Stripe/cargo-target/release/botho-formal-drt
"$runner" --legs timestamps --seed 1915 --cases 10000 --no-write-regressions --json /tmp/botho-1942-timestamps-1915.json
"$runner" --seed 1707 --cases 10000 --no-write-regressions --json /tmp/botho-1942-all-1707.json
```

Default model and regression locations are compiled from the runner's crate
path; `--model` and `--regressions` can select explicit equivalent locations.
Nightly discovers this same single leg through `--list`; no separate list is
maintained. Finite agreement is evidence for the helpers, not a proof of Rust
equivalence or random coverage of complete production acceptance.
