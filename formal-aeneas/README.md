# formal-aeneas/ - Rust to Lean extraction (Charon + Aeneas) and proofs about it

Generated Lean for the consensus kernels that Charon and Aeneas extract with
no axioms in the output (#1712, epic #1704), and proofs about that generated
Lean (#1821). Two kernels extract unchanged; five more extract after small
behaviour-preserving Rust rewrites (#1820, see below). The spike memo with per-kernel results and the
go/no-go recommendation is
[`formal/aeneas/SPIKE.md`](../formal/aeneas/SPIKE.md).

- `toolchain.env`: pinned Aeneas release (with asset sha256 hashes) and the
  Rust nightly that charon-driver needs.
- `regen.sh`: regenerates `generated/`. Use `regen.sh --check` to compare
  instead of overwrite. Its header lists the toolchain prerequisites.
- `generated/`: output, one file per extracted crate. Do not edit it by
  hand. `MonetaryKernels.lean` (crate `bth-cluster-tax`) and
  `PoolAccounting.lean` (crate `botho`) are built as the lean_libs of the
  same names.
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

Lottery and emission kernels (`BothoAeneas/LotteryKernels.lean`):

- `sybil_reward_cap_eq`: `sybil_reward_cap(ρ)` never fails and equals
  `ρ · 250_000_000_000` (the `checked_mul` cannot overflow).
- `is_eligible_at_eq`: `is_eligible_at` is the Path C window test, with
  `age = current_block - creation_block` truncated at 0.
- `lottery_emission_bps_spec`: `lottery_emission_bps` is
  `min(epoch · 1000, 5000)`, and 0 when `halving_interval = 0`.
- `lottery_emission_share_spec`, `lottery_emission_share_le_half`,
  `lottery_emission_share_epoch0`: the share is `reward · bps / 10000`
  with no overflow or truncation, at most half the reward, and 0 in epoch 0.
- `capitalized_reset_charge_no_downgrade`: no charge unless
  `output_factor < input_floor_factor`.

`count_eligible_in` (the loop behind `count_eligible`) and the
`demurrage_charge` arithmetic are extracted and pass the axiom gate, but
have no theorems yet.

Pool accounting (`BothoAeneas/PoolAccounting.lean`), for
`pool_fraction_permille ≤ 1000` (the ratified value is 800):

- `split_fees_permille_spec`: the pool gets `⌊total · permille / 1000⌋` and
  `pool + burn = total`.
- `compute_pool_accounting_permille_spec` / `_conserves` / `_available`:
  never fails; fees are conserved; `available` is
  `min(stored + emission + fee_pool, u128::MAX)` (exact below saturation);
  `payout = min(available, payout_cap)`.

### The #1820 rewrites

Each is behaviour-preserving and has a differential test in the Rust crate
(module `aeneas_refactor_drt` next to the kernel) that checks it against a
verbatim `#[cfg(test)]` copy of the old code, on boundary values
(`u128::MAX` / `u32::MAX` overflow, zero divisors, empty slices, window
edges) and a deterministic pseudo-random sweep:

- `a.saturating_mul(b)` became `a.checked_mul(b).unwrap_or(T::MAX)` in
  `sybil_reward_cap`, `MonetaryPolicy::lottery_emission_bps` and
  `demurrage_charge` (Aeneas models `checked_mul` and `unwrap_or`, not
  `saturating_mul`).
- `u64::try_from(charge).unwrap_or(u64::MAX)` became
  `charge.min(u64::MAX as u128) as u64` in `demurrage_charge` (Aeneas has no
  model of the `u128 -> u64` `TryFrom`).
- `count_eligible` delegates to the integer-only `is_eligible_at` and
  `count_eligible_in`, an explicit `while` loop over any
  `T: EligibilityView` (Aeneas models neither `f64`, which
  `LotteryCandidate` carries, nor `Iterator::filter` / `count`).
- `compute_pool_accounting` and `LotteryFeeConfig::split_fees` delegate to
  `compute_pool_accounting_permille` / `split_fees_permille`, which take
  `pool_fraction_permille: u32` instead of the config (whose `draw_config`
  carries `f64`).

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
   only the three libraries and the `aeneas` dependency at `AENEAS_COMMIT`;
   imports stay within `Aeneas`, `MonetaryKernels`, `PoolAccounting` and
   `BothoAeneas`; and no
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
`#print axioms` over every exported theorem and over the extracted kernels
listed in `Theorems.lean` shows nothing else: the Aeneas library declares
axioms for some Rust std items, but the extracted kernels reach none of them.

`regen.sh` runs the same scan (`gate_scan.py --generated`) over every fresh
extraction, so an unmodelled item cannot enter `generated/` in any spelling.

## CI

`.github/workflows/aeneas-extract.yml` has two jobs, both outside
`formal.yml` and its trust chain, and neither a required check:

- `drift` checks that `generated/` matches a fresh extraction. It runs on
  changes to `cluster-tax/**`, `botho/src/consensus/lottery.rs`,
  `formal-aeneas/**` and the workspace manifests. Extracting
  `PoolAccounting` runs Charon over the whole `botho` crate graph (metadata
  and MIR only, about 25 s cold on an M-series laptop).
- `typecheck` checks runner disk space (freeing some if below 15 GB), installs elan and the
  pinned Lean, restores the Mathlib `.ltar` cache (keyed on
  `toolchain.env`, `lean-toolchain` and `lake-manifest.json`), runs
  `lake exe cache get`, then `check-gates.sh` and `selftest-gates.sh`.

`typecheck` compiles the PR's Lean in the same job that runs the gates. There
is no isolated build -> verify split like `formal.yml`'s (#1799): code that
runs during the build (the package, or the Aeneas and Mathlib lakefiles, which
are Lean programs) could in principle affect the gate's result. So a green
`typecheck` gives less assurance than `formal/`'s isolated `verify` job. It is
a correctness check for honest contributors, it is informational, and it is
not a required check.

If you edit a mirrored Rust kernel (see `cluster_tax_start_from` and
`botho_start_from` in `regen.sh`, and the functions they call), run
`formal-aeneas/regen.sh` and commit the regenerated file in the same PR. If the
proofs then break, repair them in the same PR.

## Bumping the toolchain

Change `toolchain.env`, set the same commit as `rev` in `lakefile.toml`, run
`lake update` (this rewrites `lake-manifest.json` and fetches the matching
Mathlib), make `lean-toolchain` equal the Aeneas library's, then
`regen.sh` and `scripts/check-gates.sh`.
