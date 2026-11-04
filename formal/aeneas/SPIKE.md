# Aeneas spike: Rust to Lean extraction of the cluster-tax kernels

Issue #1712, part of epic #1704. Run on 2026-11-01/02 against main `acd2dc4a`.

**Recommendation: go, with limits.** Charon and Aeneas ran here and are
fast. They accept 2 of the 8 kernels unchanged, with axiom-free output that
type-checks. A third kernel extracts with one Charon flag. Small,
behavior-preserving rewrites bring in the other 5 (#1820). The two clean kernels
are checked in under `formal-aeneas/generated/`, with a regeneration script and
a drift CI job. `draw_eligible` stays out of scope for Aeneas.

Nothing in this PR changes Rust code or consensus behavior.

**Update (#1820):** the five kernels marked "needs a refactor" below now
extract axiom-free after behaviour-preserving rewrites, each covered by a
differential test against the old code, and are checked in under
`formal-aeneas/generated/`. One more blocker turned up that the table does
not list: `demurrage_charge`'s `u64::try_from(u128).unwrap_or(u64::MAX)`
extracts to an axiom for the `TryFrom` impl, so it is now spelled
`min(u64::MAX as u128) as u64`. See `formal-aeneas/README.md`.

## What was run

Everything ran in isolated locations. The workspace `rust-toolchain`
(`nightly-2026-06-09`) and the global rustup default (`stable`) were not
changed.

| Piece | Pin | Where it lived |
|---|---|---|
| Aeneas | release `nightly-2026.11.02-ab34bd2` (commit `ab34bd2`), prebuilt `aeneas-macos-aarch64.tar.gz` | `/Volumes/Stripe/aeneas-spike/mac` |
| Charon | 0.1.304 (`7b462d9`), bundled in the Aeneas tarball | same |
| Rust for `charon-driver` | `nightly-2026-09-17` + `rustc-dev,llvm-tools,rust-src,miri` | separate `RUSTUP_HOME=/Volumes/Stripe/aeneas-spike/rustup` |
| Lean backend library | `backends/lean` from the tarball: Lean `v4.35.0` (the same as `formal/lean-toolchain`) + Mathlib `v4.35.0` | Mathlib via `lake exe cache get` (9031 files, 7.2 GB, about 1m45s) |

All pins live in `formal-aeneas/toolchain.env`, with the sha256 of each
release asset. The release ships prebuilt binaries for linux-x86_64,
linux-aarch64 and macos-aarch64, so neither OCaml nor a Charon build is
needed. Releases are cut nightly and old ones are kept: 292 back to
2026-03-13 at the time of writing.

Command, per kernel:

```bash
charon cargo --preset aeneas --start-from <rust path> --dest-file K.llbc -- -p <pkg> --lib
aeneas -backend lean -dest out/ K.llbc
lake env lean out/K.lean     # inside the Aeneas backends/lean package
```

## Per-kernel results

File and line references were re-checked on main `acd2dc4a`. Extraction time
is wall time with a warm build of the crate's dependencies. The first cold
Charon run built the cluster-tax dependencies in about 10 s, and the whole
`botho` crate graph in 16.7 s (Charon only needs metadata and MIR, not
codegen). Aeneas takes about 0.3 s per kernel. Type-checking takes about 3 s
per file.

| Kernel | Location | Charon | Aeneas | Output type-checks | Axioms in output | Verdict |
|---|---|---|---|---|---|---|
| `MonetaryPolicy::halving_reward` | `cluster-tax/src/monetary.rs:306` | accepted, 0.8 s | accepted | yes | none | **clean, checked in** |
| `MonetaryPolicy::calculate_tail_reward_u128` | `cluster-tax/src/monetary.rs:335` | accepted, 0.8 s | accepted | yes | none | **clean, checked in** |
| `compute_pool_accounting` | `botho/src/consensus/lottery.rs:326` | accepted, 16.7 s cold / 5 s warm | accepted | **no**: `F64` undefined | none | clean with `--opaque bth_cluster_tax::lottery::LotteryDrawConfig` (one opaque-type axiom), or refactor (#1820) |
| `sybil_reward_cap` | `cluster-tax/src/lottery.rs:272` | accepted, 1.6 s | accepted | yes | `core.num.U128.saturating_mul` | needs a one-line refactor or an upstream model (#1820) |
| `MonetaryPolicy::lottery_emission_share` | `cluster-tax/src/monetary.rs:271` (via `lottery_emission_bps`, :254) | accepted, 0.8 s | accepted | yes | `core.num.U32.saturating_mul` | needs a one-line refactor or an upstream model (#1820) |
| `demurrage_charge` | `cluster-tax/src/demurrage.rs:109` | accepted, 0.8 s | accepted | yes | `core.num.U128.saturating_mul` | needs a one-line refactor or an upstream model (#1820) |
| `capitalized_reset_charge` | `cluster-tax/src/demurrage.rs:198` | accepted, 0.8 s | accepted | yes | inherits `U128.saturating_mul` from `demurrage_charge` | clean once `demurrage_charge` is |
| `count_eligible` | `cluster-tax/src/lottery.rs:248` | accepted, 0.9 s | accepted, with warnings | **no**: `F64` undefined | `Iterator::filter`, `Filter::count`, slice `Iter::count`, type `Filter` | needs a refactor: integer-only view and explicit loop (#1820) |

"Accepted" means the tool ran with no error. An axiom in the output is a
Rust standard-library item that Aeneas has no Lean model for. It type-checks,
but nothing can be proven about its result, so a kernel that calls one is not
usable for proofs as extracted.

### What is outside the supported subset

Three things, and nothing else in these kernels:

1. **`saturating_mul`.** The Aeneas standard library models `saturating_add`
   and `saturating_sub` but not `saturating_mul` (see
   `backends/lean/Aeneas/Std/Scalar/SaturatingOps.lean`). It does model
   `checked_mul` and `Option::unwrap_or`. The smallest rewrite is
   `a.saturating_mul(b)` to `a.checked_mul(b).unwrap_or(T::MAX)`, which is
   identical for unsigned integers. A prototype of exactly this rewrite
   extracts axiom-free and type-checks. In `sybil_reward_cap` the product
   cannot overflow (`usize` is at most 2^64 and the base fee is below 2^38),
   so a plain `*` is also identical. The alternative with no Rust change is
   to add a `saturating_mul` model upstream in Aeneas, which is about 5 lines
   next to `saturating_add`, and bump the pin.
2. **`f64`.** Aeneas emits `F64` but has no Lean definition for it, so any
   file that mentions a struct with an `f64` field fails to type-check.
   `LotteryCandidate.tag_entropy`, `LotteryDrawConfig.pool_fraction` and
   `SelectionMode::{Hybrid, EntropyWeighted}` carry `f64`. These fields feed
   only the research-only `EntropyWeighted` mode and the `Hybrid` weight, not
   eligibility or pool accounting, but they are still on the struct. With a
   stub `abbrev F64 := Float` added by hand, both blocked files type-check, so
   `F64` is the only blocker for them. The smallest rewrite is to have the
   kernels take integer-only views:
   - `count_eligible` over a `(creation_block, value)` view with the three
     window bounds;
   - `split_fees` / `compute_pool_accounting` taking
     `pool_fraction_permille: u32`.
   For `compute_pool_accounting` alone there is a no-refactor path: Charon
   `--opaque bth_cluster_tax::lottery::LotteryDrawConfig` replaces the struct
   with `axiom LotteryDrawConfig : Type`. The output then type-checks, and the
   function never reads that field. It is an opaque type, not an opaque
   function, so no property depends on it. It still fails the "no axiom" bar
   used for `generated/`.
3. **Iterator adapters and closures.** Charon translates the `filter` closure
   in `count_eligible` fine, into a closure struct with `FnMut`/`FnOnce`
   instances. Aeneas has no model of `Iterator::filter` or `Iterator::count`,
   so both become axioms. Rewriting as `while i < cs.len() { if
   is_eligible(&cs[i], ..) { n += 1 } i += 1 }` extracts to an Aeneas `loop`
   with no axioms (prototyped). Proofs about it need a loop invariant and a
   termination measure (`cs.len() - i`).

u128 was not a problem: `U128` arithmetic, casts, `min`/`max`/`clamp` and
`u64::try_from(u128)` with `unwrap_or` are all modelled.

`draw_eligible` (`cluster-tax/src/lottery.rs:361`, not in this list) is the
kernel Aeneas is least suited to: `std::collections::HashSet`,
`iter().enumerate().filter().map().sum()` chains, `Vec<(&T, u128)>`, and
`verifiable_random_u128`, which reaches sha2. Keep it on the hand-written
model plus DRT path in `formal/`.

### What extraction already surfaces

Aeneas models every Rust panic as `Result.fail`, so the extracted functions
state their panic conditions exactly. These are side conditions any proof has
to discharge. They are listed here for the record and are not bugs under the
ratified parameters:

- `halving_reward` fails if `halving_interval * halving_count` overflows u64
  (in `tail_emission_start_height`), or if the shift amount is 64 or more.
  The second cannot happen in phase 1 while `halving_count < 64`. It returns
  `none` (no division) when `halving_interval == 0`, because
  `height < 0` is false.
- `calculate_tail_reward_u128` fails on `target_block_time_secs == 0`
  (division by zero), on `target_block_time_secs > 31_536_000` (then
  `blocks_per_year == 0`), and on `supply * bps` overflowing u128. The final
  `as u64` is a truncating cast, so a proof has to bound `reward` to show
  nothing is cut off.
- `lottery_emission_share` and `split_fees` both end in a truncating
  `as u64` that is lossless only because the bps or permille factor is at
  most 10000 or 1000 respectively. The model makes that obligation explicit.

## Estimated proof-maintenance cost per kernel

"Upkeep" is the expected work when the Rust kernel changes. The drift job
forces a regenerate on any edit to a mirrored file, even one that only moves
lines, because the output embeds source spans. Upkeep is then reviewing the
regenerated diff and re-running the proofs.

| Kernel | Initial proof effort (est.) | Upkeep per Rust change | Notes |
|---|---|---|---|
| `halving_reward` | 0.5 day | low: straight-line code, `scalar_tac` / `step*` close most goals | bounds, `none` after the tail start, monotone in height |
| `calculate_tail_reward_u128` | 1 day | low | no-overflow and no-truncation bounds under mainnet parameters, result ≥ 1 |
| `lottery_emission_share` | 0.5 day after #1820 | low | share ≤ reward, 0 in epoch 0, capped at 50% |
| `sybil_reward_cap` | 0.25 day after #1820 | very low | equals `ρ · base_fee` exactly (no saturation reachable) |
| `demurrage_charge` | 1–2 days after #1820 | medium: nonlinear u128 arithmetic with two divisions | zero cases; monotone in value, factor and elapsed; charge ≤ `u64::MAX` |
| `capitalized_reset_charge` | 0.5 day on top of `demurrage_charge` | low | 0 unless downgrading, ≤ the charge at the floor factor |
| `compute_pool_accounting` | 0.5 day after #1820 (or now with `--opaque`) | low | `fee_pool + fee_burn = total_fees`, `payout ≤ min(available, cap)`, `available` never decreases except through saturation |
| `count_eligible` | 1–2 days after #1820 | medium: loop invariant | result ≤ `len`, equals the size of the filter over the list |

Fixed costs on top of that:

- **Toolchain bumps.** Aeneas and Charon release nightly, and each Aeneas pin
  fixes a Rust nightly and a Lean + Mathlib version. Expect about 0.5 day per
  bump, mostly regenerating output and repairing proofs where the shape of the
  generated code changed. Bump on purpose, about once a quarter or when a
  needed model lands, not on every release.
- **Mathlib.** The Aeneas Lean library depends on Mathlib, about 7 GB of cache.
  That is why the generated Lean cannot live in `formal/`. `formal/` is
  dependency-free, and its gates (`gate_scan.py`) reject Mathlib imports,
  `module` / `public import`, and any `.lean` file outside `Botho/`. It is
  also why the type-check is a follow-up rather than part of the drift job.

## Go / no-go

**Go, scoped to integer kernels in `cluster-tax` (plus `compute_pool_accounting`).**

- For: Installation is a tarball and one `rustup toolchain install`, with no
  OCaml, Nix or Docker needed. A full regeneration takes about 2 s warm and
  under a minute cold. The output reads as a faithful transcription of the
  Rust, panics included. The output is byte-identical across platforms:
  `regen.sh --check` produced the same file on macOS arm64 and in a Linux
  container (the `rust:1-bookworm` image) on arm64 and amd64. It also proves
  facts about the code that runs, rather than about a hand-written model that
  DRT has to tie back to it.
- Against: 6 of 8 kernels need either an upstream Aeneas change or a Rust
  rewrite first. The tool is a moving nightly with a heavy Mathlib dependency.
  Floats, `HashSet`, iterator chains and hashing are out of reach, so the
  `formal/` hand-model plus DRT path is still needed for the draw and
  everything around it.
- Suggested split: use Aeneas for the pure integer kernels above, where
  "proven about the extraction" is strictly better than "proven about a model
  + tested". Keep `formal/` for protocol-level models (the draw, state
  transitions) that Aeneas cannot reach.

## What this PR lands

- `formal/aeneas/SPIKE.md`: this memo. It is not a `.lean` file, so the
  formal gates ignore it. `gate_scan.py` passes with it present.
- `formal-aeneas/toolchain.env`: all pins and sha256 hashes.
- `formal-aeneas/regen.sh`: downloads and verifies the pinned release (or
  uses `AENEAS_DIR`), runs Charon and Aeneas, and refuses any output that
  contains `axiom` or `sorry`. Without arguments it rewrites `generated/`;
  with `--check` it fails on any difference.
- `formal-aeneas/generated/MonetaryKernels.lean`: the axiom-free extraction of
  `halving_reward` and `calculate_tail_reward_u128`, with their callees
  `is_halving_phase` and `tail_emission_start_height`. It type-checks against
  the pinned Aeneas library. It lives outside `formal/`, so it is not part of
  the lake package that `formal-gate` verifies.
- `.github/workflows/aeneas-extract.yml`: a separate workflow on
  `ubuntu-latest`. It installs the pinned nightly, caches the release tarball,
  and runs `regen.sh --check`. It triggers on `cluster-tax/**`,
  `formal-aeneas/**` and the workspace manifests, and does not touch
  `formal.yml` or its build → verify trust chain. It is not a required check.
  Checked locally: it passes on a clean tree and fails, with a readable Lean
  diff, after a one-token edit to `halving_reward`.

## Follow-ups

- #1820: behavior-preserving rewrites (`saturating_mul`, integer-only config
  views, an explicit loop in `count_eligible`), each with DRT showing the old
  and new functions agree, plus adding each kernel to `regen.sh`.
- #1821: type-check `formal-aeneas/generated/` in CI (Mathlib cache) and add
  the first proofs over the extraction in a separate lake package. Done:
  see `formal-aeneas/README.md`.
- Possible upstream contributions to AeneasVerif/aeneas, not filed from here:
  a `saturating_mul` model and an opaque `F64`. Either would remove the need
  for the corresponding rewrite in #1820.
