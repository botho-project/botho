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

`timestamps.jsonl` preserves all 30 named helper fixtures from #1942 and adds
`clock_u_minus_61`, for 31 expected outputs. It calls five pure production
helpers and three scalar predicates shared with production validators (#1943).
Inputs require `window` (oldest first), `parent`, `stamp`, `minter_now`
and `validator_now`; every scalar and window element is a decimal u64 string.
Unknown/missing fields, JSON numbers, malformed decimal strings and overflow
are rejected. Replay accepts arbitrary finite windows without pre-truncation;
generation biases lengths 0/1/5/6/10/11/12/32 within 0..32 and retains an
independent arbitrary-u64 branch. Codec tests also pass 64-entry windows.

Each `expected` object retains `mtp`, `next_min`, `honest_stamp`, complete
`rolled`, and nullable `observed_gap`; output integers are strings. For both
`candidate` and `honest`, `lower_*`, `ledger_future_*` and `ledger_*` expose the
shared lower predicate, saturated ledger upper predicate and conjunction
independently. The nested `minting` object contains `contextual_candidate`,
`contextual_honest`, `intrinsic_candidate`, `intrinsic_honest` at
`validator_now ≤ U-60` (`U = 18446744073709551615`). Contextual uses lower and
ordinary-add upper; intrinsic uses only upper. Above that domain it is exactly
`{"error":"validator_now_out_of_domain"}`; no ordinary-add call occurs and
helper/ledger outputs remain. This is a DRT domain error, not a production
rejection rule. No input is clamped, filtered or resampled. The module tests
assert the complete pinned outputs; the runner independently compares Rust
with the landed Lean model after strict decoding of every input.

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
scalar timestamp-rule history; earlier stamps are not asserted honestly
produced. Complete-validator equivalence, clock acquisition/failure, SCP
scheduling and physical-clock realizability remain outside this finite DRT
claim. The ledger retains full-u64 saturation; production minting retains
ordinary addition. Both primitive outcomes remain observable when a conjunction
fails. `length_six_mtp_equality` pins contextual rejection with intrinsic
acceptance of the timestamp stage. The maximum-MTP fixture pins lower failure
with upper success, and `zero_skew_step_6` pins the headroom counterexample.

Production entrypoint tests separately cover header/mint mismatch before MTP,
MTP/future rejection, backward-parent admission in the ledger, and contextual
future-before-MTP rejection. Minting tests reaching `InvalidPoW` establish only
that the timestamp stage passed. Source review preserves ledger lower-before-
clock and clock-error precedence; no clock-failure injection was executed.

Historical #1942 validation (2026-11-06, before #1943): 10,000 generated
timestamp cases at seed 1915 plus all 30 replays; all-leg PR tier, 10,000 cases per discovered leg at seed 1707.
Every generated timestamp case is classified by window size, gap, successor
and future saturation, and both adapter outcomes. The isolated negative
control changed only the post-helper `Some(3)` result to `Some(4)` and failed
on `length_one_gap_two`; byte-identical restored source passed the same replay.
No mutation mechanism remains. Historical reports, coverage, source/binary hashes
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
cp "$WORKTREE_ABS/Cargo.lock" "$WORKTREE_ABS/formal-drt/Cargo.lock"
cargo +nightly-2026-06-09 build --release --manifest-path "$WORKTREE_ABS/formal-drt/Cargo.toml"
runner="$CARGO_TARGET_DIR/release/botho-formal-drt"
model="$WORKTREE_ABS/formal-drt/lean/.lake/build/bin/botho-model"
regressions="$WORKTREE_ABS/formal-drt/regressions"
"$runner" --model "$model" --regressions "$regressions" --legs timestamps --seed 1915 --cases 10000 --no-write-regressions --json /tmp/botho-1943-reproduced-timestamps-1915.json
"$runner" --model "$model" --regressions "$regressions" --seed 1707 --cases 10000 --no-write-regressions --json /tmp/botho-1943-reproduced-all-1707.json
```

The excluded `formal-drt` workspace needs the root lockfile copied in, as in
CI. Its first build adapts that copy to the DRT crate graph; `--locked` rejects
that necessary update immediately after the copy. The tracked root lockfile
is unchanged.

The output filenames above keep the retained reports intact. Runner hashes
identify a particular build: ordinary builds embed build time, so record the
new hash rather than expecting a rebuild to match the retained binary.
Nightly discovers this same single leg through `--list`; no separate list is
maintained. Finite agreement is evidence for these helpers and scalar
predicates on the declared domains, not a proof of Rust equivalence or full-validator admission.

Current #1943 validation (2026-11-06) used only the managed issue-1943
worktree's fresh `target/`, with two Cargo jobs and the pinned toolchains above.
Timestamp seed 1915 passed 10,000 generated cases and all 31 replays: 9,005
common-domain cases and 995 explicit minting domain errors, with both outcomes
for every applicable primitive and conjunction. Seed 1707 passed 10,000 cases
per each of 24 discovered legs (240,000 total) and all 141 replays. The 12 DRT
unit tests passed, including codecs, domain tags, seeded coverage, shrinking,
fixture expectations and all seven rolling-state links. The checked CI-profile
Botho library run passed all 1,746 non-ignored tests (13 existing ignored,
zero filtered), with 16 MTP-module tests and seven scoped timestamp selectors
also passing. DRT release clippy with warnings denied, both formatting checks
and stale-doc lint passed. The repository CI clean-crate clippy gate passed.
`cargo build --workspace` passed locally on macOS;
the complete Linux workspace CI job remains required on the PR head.

The negative control changed the actual production
`ledger_timestamp_within_future_bound` comparison from `<=` to `<`. A private
rebuild made the pinned `future_cap_equality` replay fail (exit 1, one mismatch
in both ledger upper observations and conjunctions). Byte-for-byte source
restoration, another private rebuild, and the identical replay passed (exit 0).
No control switch remains. Actual SHA-256 identities:

- Production helper source before/after restoration: `5ab80ae9a6d23cd8d35d00a69e223bbb1eb94b006c482eb5ee1ffd540889d7a3`.
- Mutated runner: `b26045974b93b364789faeb12bc31ad4ddc38d8d05b8041f758197758ad0562c`.
- Restored runner used for both generated runs: `83c2c480fd9e275140ccb45fe757769b68b2b9539536d5f76e24ece73b4fb070`.
- Lean model: `3c9f32ead79520132b06e1ab61ff92ce9de4dc40b6d29f62d4a9f6cf4ecd38ba`.

The run source is base `3a46b2e4113c43ef5a4d9ebf9e250dfc8b43b8a1` plus
`/tmp/botho-1943-run-source.diff`, whose exact hash and per-file identities are
in `/tmp/botho-1943-run-identity.json`. Subsequent documentation-only changes
record these results. Commands, exits, reports, classified counts, mutation
diff and binary identities are retained in `/tmp/botho-1943-evidence.json`
and the final `/tmp/botho-1943-builder-receipt.json`; individual commands are
also in `/tmp/botho-1943-commands.jsonl`.

The pinned Lean DRT build succeeded before the coverage-comment update in
`Timestamps.lean`; all model declarations remain byte-identical. Its binary
is not claimed to have been built from the final comment bytes.
The separate local Formal build gate was
blocked by this host's intentional `lake` memory-safety wrapper; an attempted
base verifier over the unstaged build directory failed artifact intake on
auxiliary `.ilean`/`.trace` files. Neither attempt is a passing Formal verdict.
The complete Formal CI build/verify gate on the PR head remains required.
