# formal-aeneas/ - Rust to Lean extraction (Charon + Aeneas) and proofs about it

Generated Lean for the cluster-tax kernels that Charon and Aeneas accept
unchanged, with no axioms in the output (#1712, epic #1704), and proofs about
that generated Lean (#1821). The spike memo with per-kernel results and the
go/no-go recommendation is
[`formal/aeneas/SPIKE.md`](../formal/aeneas/SPIKE.md).

- `toolchain.env`: pinned Aeneas release (with asset sha256 hashes) and the
  Rust nightly that charon-driver needs.
- `regen.sh`: regenerates `generated/`. Use `regen.sh --check` to compare
  instead of overwrite. Its header lists the toolchain prerequisites.
- `generated/`: output. Do not edit it by hand. Built as the lean_lib
  `MonetaryKernels`.
- `BothoAeneas/`: proofs about `generated/` (lean_lib `BothoAeneas`).
- `Theorems.lean`: the exported theorems, one `#print axioms NAME` line each.
- `axioms-allowlist.txt`: the axioms any declaration of the package may use.
- `lakefile.toml`, `lake-manifest.json`, `lean-toolchain`: the lake package.
  It requires the Aeneas Lean library (`backends/lean`) at `AENEAS_COMMIT`,
  which brings in Mathlib.
- `scripts/`: the gates (`check-gates.sh`, `gate_scan.py`,
  `AxiomReport.lean`) and their self-test (`selftest-gates.sh`).

This directory is deliberately outside `formal/`. The Aeneas library depends
on Mathlib, and `formal/`'s gates allow neither. Nothing here is part of the
`formal-gate` verification, and `formal/` neither imports nor builds anything
from here.

## What is proven

Label: **proven (extraction)** (see the labeling rule in
[`formal/README.md`](../formal/README.md)). In the extraction an overflowing
`+` / `*`, a division by zero and an over-wide shift are `fail`, so "returns
`ok`" means "does not panic".

`MonetaryPolicy::halving_reward` (`BothoAeneas/HalvingReward.lean`), assuming
`halving_interval * halving_count ≤ u64::MAX` and `halving_count ≤ 64`:

- `halving_reward_no_fail`: it returns `ok` for every height.
- `halving_reward_phase1`: for `height < halving_interval * halving_count`
  it returns `some (initial_reward >>> (height / halving_interval))`.
- `halving_reward_phase2`: for later heights it returns `none`.
- `halving_reward_antitone`, `halving_reward_none_stays`: monotone
  non-increasing in height (a later phase-1 reward is at most an earlier
  one, and `none` never turns back into `some`).

`MonetaryPolicy::calculate_tail_reward_u128` (`BothoAeneas/TailReward.lean`):

- `calculate_tail_reward_u128_spec` / `_no_fail`: for
  `target_block_time_secs ∈ [1, 31_536_000]` and
  `supply * (tail_inflation_bps + expected_fee_burn_rate_bps) ≤ u128::MAX`
  (implied by `supply < 2^95`, `overflow_bound_of_lt_two_pow_95`) it returns
  `ok (max(reward, 1) mod 2^64)`.
- `calculate_tail_reward_u128_ge_one`: the result is `max(reward, 1) ≥ 1`
  when `reward < 2^64` (no truncation by `as u64`).
- `calculate_tail_reward_u128_truncates_to_zero`: without that condition
  "result ≥ 1" is false. Example: one block per year, 100% tail inflation,
  supply `2^64` gives `0`.
- `calculate_tail_reward_u128_mainnet`: with the mainnet tail parameters
  (200 bps, 50 bps burn, 5 s blocks) and any supply below `2^91` picocredits
  (over a million times the design supply), it returns `ok`, does not
  truncate, and the result is at least 1. `mainnet_reward_exceeds_u64_at_two_pow_92`
  shows the bound cannot be raised to `2^92`.

## Building and gates

```bash
cd formal-aeneas
lake exe cache get              # Mathlib oleans (~7.2 GB unpacked, ~2 min)
lake build                      # the extraction and the proofs
scripts/check-gates.sh          # scan, build, axiom walk, #print axioms
scripts/selftest-gates.sh       # each gate layer rejects injected defects
```

The checkout of Mathlib under `.lake/` is large. To keep it off a small disk,
make `.lake` a symlink to another volume before the first `lake` call (it is
gitignored either way), and set `MATHLIB_CACHE_DIR` for the `.ltar` cache.

`check-gates.sh` fails unless:

1. the source scan passes (`scripts/gate_scan.py`): the lakefile declares
   only the two libraries and the `aeneas` dependency at `AENEAS_COMMIT`;
   imports stay within `Aeneas`, `MonetaryKernels` and `BothoAeneas`; and no
   source contains `sorry`, `admit`, `axiom`, `opaque`, `unsafe`,
   `partial`, `native_decide`, kernel-bypass options, code-running commands
   or attributes. Keywords match as tokens anywhere outside comments, so an
   indented or `@[...]`-prefixed `axiom` fails as well;
2. `lake build` succeeds;
3. the axiom walk over the compiled package (`scripts/AxiomReport.lean`)
   finds no declaration that reaches `sorryAx` or an axiom outside
   `axioms-allowlist.txt`, and no `axiom`, `opaque`, `unsafe` or `partial`
   declaration in the package; and
4. `#print axioms` for each line of `Theorems.lean` agrees with the walk.

The allow-list is `propext`, `Classical.choice` and `Quot.sound`. Running
`#print axioms` over every exported theorem and both extracted functions
shows nothing else: the Aeneas library declares axioms for some Rust std
items, but the extracted kernels reach none of them.

`regen.sh` runs the same scan (`gate_scan.py --generated`) over every fresh
extraction, so an unmodelled item cannot enter `generated/` in any spelling.

## CI

`.github/workflows/aeneas-extract.yml` has two jobs, both outside
`formal.yml` and its trust chain, and neither a required check:

- `drift` checks that `generated/` matches a fresh extraction.
- `typecheck` checks runner disk space (freeing some if below 15 GB), installs elan and the
  pinned Lean, restores the Mathlib `.ltar` cache (keyed on
  `toolchain.env`, `lean-toolchain` and `lake-manifest.json`), runs
  `lake exe cache get`, then `check-gates.sh` and `selftest-gates.sh`.

If you edit a mirrored Rust kernel (see `start_from` in `regen.sh`), run
`formal-aeneas/regen.sh` and commit the regenerated file in the same PR. If the
proofs then break, repair them in the same PR.

## Bumping the toolchain

Change `toolchain.env`, set the same commit as `rev` in `lakefile.toml`, run
`lake update` (this rewrites `lake-manifest.json` and fetches the matching
Mathlib), make `lean-toolchain` equal the Aeneas library's, then
`regen.sh` and `scripts/check-gates.sh`.
