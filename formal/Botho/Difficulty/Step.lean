/-
Per-block difficulty step (#1841, epic #1704).

Mirrors `EmissionController::compute_time_adjusted_difficulty`
(`botho/src/block.rs`, module `difficulty`, the M5 / #554 time-based
controller). It is the only difficulty kernel on the live block path: the node
calls it once per committed block, through `EmissionController::record_block`
(`block.rs`), from `node::commit_block_locked` (`botho/src/node/mod.rs`). The
legacy kernels (`compute_adjusted_difficulty`, `calculate_new_difficulty`) and
the simulator's `cluster-tax` `DifficultyController` are NOT mirrored.

Numeric convention (`BlockHeader::is_valid_pow`, `block.rs`): PoW is valid iff
`pow_value(hash) < difficulty`, so a HIGHER difficulty number is EASIER PoW.
At difficulty `d` one hash succeeds with probability `d / 2^64`.
`MIN_DIFFICULTY = 1` is the hardest PoW (a block needs `pow_value = 0`),
`MAX_DIFFICULTY` the easiest (about 1.3 expected hashes per block).

The model works over `Nat`. Rust's `u64` and `u128` are modeled as `Nat`
values, and every Rust operation that could wrap or panic is written out:
`saturating_mul` as `min _ U64_MAX`, `u64::try_from(..).unwrap_or(u64::MAX)`
as `truncU64`, and `Ord::clamp` as `clampN`. `step_no_overflow` shows the two
`u128` products the Rust code forms stay below `2^128` for every `u64` input,
so the `Nat` model and the Rust arithmetic agree (no wrap, no panic).

Labels (formal/README.md):
- every theorem in this file is **proven (model)**;
- the model agrees with the Rust kernel on the vectors in
  `Botho/Difficulty/Vectors.lean` (**tested**: the Lean kernel proves the
  model's outputs there, and the Rust test
  `formal_drt_difficulty::step_vectors_match_rust` checks the Rust kernel
  against the same file).
-/
namespace Botho.Difficulty

/-- `u64::MAX`. -/
def U64_MAX : Nat := 18446744073709551615

/-- `u128::MAX`. -/
def U128_MAX : Nat := 340282366920938463463374607431768211455

/-- `node::minter::INITIAL_DIFFICULTY` (`botho/src/node/minter.rs`). -/
def INITIAL_DIFFICULTY : Nat := 54000000000000000

/-- `difficulty::MAX_EASE_MULTIPLE`. -/
def MAX_EASE_MULTIPLE : Nat := 256

/-- `difficulty::MIN_DIFFICULTY`: the hardest PoW. -/
def MIN_DIFFICULTY : Nat := 1

/-- `difficulty::MAX_DIFFICULTY = INITIAL_DIFFICULTY.saturating_mul(256)`:
the easiest PoW. Equals `13_824_000_000_000_000_000` (`max_difficulty_value`). -/
def MAX_DIFFICULTY : Nat := min (INITIAL_DIFFICULTY * MAX_EASE_MULTIPLE) U64_MAX

/-- `difficulty::TARGET_BLOCK_TIME_SECS`. -/
def TARGET_BLOCK_TIME_SECS : Nat := 5

/-- `difficulty::MAX_OBSERVED_BLOCK_TIME_MULTIPLE`. -/
def MAX_OBSERVED_BLOCK_TIME_MULTIPLE : Nat := 12

/-- `difficulty::RATIO_SCALE` (basis points, `10_000` = 1.0x). -/
def RATIO_SCALE : Nat := 10000

/-- `difficulty::MAX_ADJUSTMENT_FACTOR_BPS` (2.0x). -/
def MAX_ADJUSTMENT_FACTOR_BPS : Nat := 2 * RATIO_SCALE

/-- Rust `Ord::clamp(x, lo, hi)` for `lo ≤ hi` (`core::cmp::Ord::clamp`):
`lo` if `x < lo`, `hi` if `x > hi`, else `x`. The Rust call sites all have
constant bounds with `lo ≤ hi`, so its `assert!(min <= max)` never fires. -/
def clampN (lo hi x : Nat) : Nat :=
  if x < lo then lo else if hi < x then hi else x

/-- `u64::try_from(x).unwrap_or(u64::MAX)` for a `u128` value `x`. -/
def truncU64 (x : Nat) : Nat :=
  if x ≤ U64_MAX then x else U64_MAX

/-- The observed time after the per-step cap:
`observed_secs.min(target_secs.saturating_mul(MAX_OBSERVED_BLOCK_TIME_MULTIPLE))`. -/
def cappedObserved (observed target : Nat) : Nat :=
  min observed (min (target * MAX_OBSERVED_BLOCK_TIME_MULTIPLE) U64_MAX)

/-- The multiplier in basis points, before the clamp:
`observed as u128 * RATIO_SCALE / target_secs as u128`. -/
def rawBps (observed target : Nat) : Nat :=
  cappedObserved observed target * RATIO_SCALE / target

/-- The multiplier after the `[0.5x, 2.0x]` clamp. -/
def stepBps (observed target : Nat) : Nat :=
  clampN (RATIO_SCALE / 2) MAX_ADJUSTMENT_FACTOR_BPS (rawBps observed target)

/-- The full kernel, `compute_time_adjusted_difficulty(current, observed_secs,
target_secs)`. Higher result = easier PoW. -/
def step (current observed target : Nat) : Nat :=
  if target = 0 then clampN MIN_DIFFICULTY MAX_DIFFICULTY current
  else
    clampN MIN_DIFFICULTY MAX_DIFFICULTY
      (truncU64 (current * stepBps observed target / RATIO_SCALE))

/-! ## Constants -/

theorem max_difficulty_value : MAX_DIFFICULTY = 13824000000000000000 := by
  unfold MAX_DIFFICULTY INITIAL_DIFFICULTY MAX_EASE_MULTIPLE U64_MAX
  decide

theorem max_difficulty_eq : MAX_DIFFICULTY = INITIAL_DIFFICULTY * 256 := by
  rw [max_difficulty_value]; unfold INITIAL_DIFFICULTY; decide

/-- `2 * MAX_DIFFICULTY` does not fit in a `u64`, so the
`unwrap_or(u64::MAX)` branch of the kernel is reachable (`step_truncates_at_max`). -/
theorem two_max_gt_u64 : U64_MAX < 2 * MAX_DIFFICULTY := by
  rw [max_difficulty_value]; unfold U64_MAX; decide

theorem max_lt_u64 : MAX_DIFFICULTY < U64_MAX := by
  rw [max_difficulty_value]; unfold U64_MAX; decide

/-! ## Clamp and truncation lemmas -/

theorem min_le_max_difficulty : MIN_DIFFICULTY ≤ MAX_DIFFICULTY := by
  rw [max_difficulty_value]; unfold MIN_DIFFICULTY; decide


theorem clampN_ge {lo hi x : Nat} (h : lo ≤ hi) : lo ≤ clampN lo hi x := by
  unfold clampN; split <;> (try split) <;> omega

theorem clampN_le {lo hi x : Nat} (h : lo ≤ hi) : clampN lo hi x ≤ hi := by
  unfold clampN; split <;> (try split) <;> omega

theorem clampN_of_mem {lo hi x : Nat} (h1 : lo ≤ x) (h2 : x ≤ hi) :
    clampN lo hi x = x := by
  unfold clampN; split <;> (try split) <;> omega

theorem clampN_mono {lo hi x y : Nat} (hlh : lo ≤ hi) (h : x ≤ y) :
    clampN lo hi x ≤ clampN lo hi y := by
  unfold clampN; split <;> split <;> (try split) <;> (try split) <;> omega

theorem truncU64_le (x : Nat) : truncU64 x ≤ x := by
  unfold truncU64; split <;> omega

theorem truncU64_mono {x y : Nat} (h : x ≤ y) : truncU64 x ≤ truncU64 y := by
  unfold truncU64; split <;> split <;> omega

theorem clampN_eq {lo hi x : Nat} (h : lo ≤ hi) : clampN lo hi x = max lo (min x hi) := by
  unfold clampN; split <;> (try split) <;> omega

theorem truncU64_eq (x : Nat) : truncU64 x = min x U64_MAX := by
  unfold truncU64; split <;> omega

/-- `step` with the clamp and truncation written as `max`/`min`, for `omega`. -/
theorem step_eq_minmax {c o t : Nat} (ht : t ≠ 0) :
    step c o t = max 1 (min (min (c * stepBps o t / 10000) U64_MAX) 13824000000000000000) := by
  unfold step
  simp only [ht, ↓reduceIte]
  rw [clampN_eq min_le_max_difficulty, truncU64_eq, max_difficulty_value]
  rfl

theorem cappedObserved_five (o : Nat) : cappedObserved o 5 = min o 60 := by
  unfold cappedObserved MAX_OBSERVED_BLOCK_TIME_MULTIPLE U64_MAX; omega

theorem stepBps_five (o : Nat) : stepBps o 5 = max 5000 (min (min o 60 * 10000 / 5) 20000) := by
  unfold stepBps rawBps
  rw [cappedObserved_five, clampN_eq (by unfold RATIO_SCALE MAX_ADJUSTMENT_FACTOR_BPS; decide)]
  rfl

theorem stepBps_ge (o t : Nat) : 5000 ≤ stepBps o t := by
  unfold stepBps RATIO_SCALE MAX_ADJUSTMENT_FACTOR_BPS
  exact clampN_ge (by decide)

theorem stepBps_le (o t : Nat) : stepBps o t ≤ 20000 := by
  unfold stepBps RATIO_SCALE MAX_ADJUSTMENT_FACTOR_BPS
  exact clampN_le (by decide)

theorem stepBps_mono {o o' t : Nat} (h : o ≤ o') : stepBps o t ≤ stepBps o' t := by
  unfold stepBps
  apply clampN_mono (by unfold RATIO_SCALE MAX_ADJUSTMENT_FACTOR_BPS RATIO_SCALE; decide)
  unfold rawBps cappedObserved
  apply Nat.div_le_div_right
  apply Nat.mul_le_mul_right
  omega

/-! ## Property 1: bounds and no overflow -/

/-- **Property 1 (bounds).** For every input, the result is in
`[MIN_DIFFICULTY, MAX_DIFFICULTY]` (so never 0). Proven (model). -/
theorem step_in_bounds (c o t : Nat) :
    MIN_DIFFICULTY ≤ step c o t ∧ step c o t ≤ MAX_DIFFICULTY := by
  have hle : MIN_DIFFICULTY ≤ MAX_DIFFICULTY := by
    rw [max_difficulty_value]; unfold MIN_DIFFICULTY; decide
  unfold step
  split
  · exact ⟨clampN_ge hle, clampN_le hle⟩
  · exact ⟨clampN_ge hle, clampN_le hle⟩

/-- **Property 1 (no overflow).** For a `u64` current difficulty and ANY
observed gap (so for every timestamp delta header validation admits), the two `u128` products
the Rust kernel forms (`observed as u128 * RATIO_SCALE` and
`current as u128 * adjustment_bps`) are at most `u128::MAX`, and the
`saturating_mul` input fits a `u64`. So the Rust arithmetic neither wraps nor
panics (release builds keep `overflow-checks = true`, #663), and the `Nat`
model computes exactly what Rust computes. The kernel has no other operation
that can fail: the division is by `target_secs ≠ 0` (checked first) and by the
constant `RATIO_SCALE`. Proven (model). -/
theorem step_no_overflow {c o t : Nat} (hc : c ≤ U64_MAX) :
    cappedObserved o t * RATIO_SCALE ≤ U128_MAX ∧
    c * stepBps o t ≤ U128_MAX ∧
    cappedObserved o t ≤ U64_MAX := by
  have hcap : cappedObserved o t ≤ U64_MAX := by
    unfold cappedObserved; omega
  refine ⟨?_, ?_, hcap⟩
  · have := Nat.mul_le_mul_right RATIO_SCALE hcap
    unfold U64_MAX RATIO_SCALE U128_MAX at *; omega
  · have := Nat.mul_le_mul hc (stepBps_le o t)
    unfold U64_MAX U128_MAX at *; omega

/-- The `unwrap_or(u64::MAX)` branch is reachable, and is clamped back to
`MAX_DIFFICULTY`: at `current = MAX_DIFFICULTY` and a slow block, the `u128`
product is `2 * MAX_DIFFICULTY > u64::MAX`. Proven (model). -/
theorem step_truncates_at_max :
    U64_MAX < MAX_DIFFICULTY * stepBps 60 5 / RATIO_SCALE ∧
    step MAX_DIFFICULTY 60 5 = MAX_DIFFICULTY := by
  rw [max_difficulty_value]
  decide

/-! ## Property 2: the per-step clamp -/

/-- **Property 2 (per-step clamp).** For a `u64` difficulty `c ≥ 1`, one step
moves it at most by the `[0.5x, 2x]` clamp: `⌊c/2⌋ ≤ step ≤ 2c`. This holds
after the absolute clamp too (`⌊c/2⌋ ≤ 2^63 < MAX_DIFFICULTY`). Proven (model). -/
theorem step_ratio {c o t : Nat} (h1 : 1 ≤ c) (hc : c ≤ U64_MAX) :
    c / 2 ≤ step c o t ∧ step c o t ≤ 2 * c := by
  by_cases ht : t = 0
  · subst ht
    unfold step
    simp only [↓reduceIte]
    rw [clampN_eq min_le_max_difficulty, max_difficulty_value]
    unfold MIN_DIFFICULTY; unfold U64_MAX at hc; omega
  · have hlo := stepBps_ge o t
    have hhi := stepBps_le o t
    have p1 : c * 5000 ≤ c * stepBps o t := Nat.mul_le_mul_left c hlo
    have p2 : c * stepBps o t ≤ c * 20000 := Nat.mul_le_mul_left c hhi
    have q1 : c * 5000 / 10000 ≤ c * stepBps o t / 10000 := Nat.div_le_div_right p1
    have q2 : c * stepBps o t / 10000 ≤ c * 20000 / 10000 := Nat.div_le_div_right p2
    rw [step_eq_minmax ht]
    generalize c * stepBps o t / 10000 = n at q1 q2
    unfold U64_MAX at *; omega

/-- Difficulty is non-decreasing in the current difficulty. Proven (model). -/
theorem step_mono_current {c c' o t : Nat} (h : c ≤ c') : step c o t ≤ step c' o t := by
  unfold step
  split
  · exact clampN_mono min_le_max_difficulty h
  · apply clampN_mono min_le_max_difficulty
    apply truncU64_mono
    apply Nat.div_le_div_right
    exact Nat.mul_le_mul_right _ h

/-- Difficulty is non-decreasing in the observed block time (a slower claimed
block never makes PoW harder). Proven (model). -/
theorem step_mono_observed {c o o' t : Nat} (h : o ≤ o') : step c o t ≤ step c o' t := by
  unfold step
  split
  · exact Nat.le_refl _
  · apply clampN_mono min_le_max_difficulty
    apply truncU64_mono
    apply Nat.div_le_div_right
    exact Nat.mul_le_mul_left _ (stepBps_mono h)

/-! ## Exact step at the live target (`target = 5`) -/

theorem stepBps_slow {o : Nat} (h : 10 ≤ o) : stepBps o 5 = 20000 := by
  rw [stepBps_five]; omega

theorem stepBps_fast {o : Nat} (h : o ≤ 2) : stepBps o 5 = 5000 := by
  rw [stepBps_five]; omega

/-- A block claiming a gap of at least 10 s (2x target) doubles the number
(eases PoW 2x), up to `MAX_DIFFICULTY`. This includes every gap the 10, 20
and 40 s pacing tiers produce, and every forward-dated gap. Proven (model). -/
theorem step_slow {c o : Nat} (h : 10 ≤ o) (h1 : 1 ≤ c) :
    step c o TARGET_BLOCK_TIME_SECS = min MAX_DIFFICULTY (2 * c) := by
  unfold TARGET_BLOCK_TIME_SECS
  rw [step_eq_minmax (by decide), stepBps_slow h, max_difficulty_value]
  unfold U64_MAX; omega

/-- A block claiming a gap of at most 2 s, including a timestamp EQUAL to its
parent's (which header validation admits), halves the number (hardens PoW 2x),
down to `MIN_DIFFICULTY`. Proven (model). -/
theorem step_fast {c o : Nat} (h : o ≤ 2) (hc : c ≤ U64_MAX) :
    step c o TARGET_BLOCK_TIME_SECS = max MIN_DIFFICULTY (c / 2) := by
  unfold TARGET_BLOCK_TIME_SECS MIN_DIFFICULTY
  rw [step_eq_minmax (by decide), stepBps_fast h]
  unfold U64_MAX at *; omega

/-- A block exactly on target (5 s) leaves an in-range difficulty unchanged.
Proven (model). -/
theorem step_on_target {c : Nat} (h1 : 1 ≤ c) (h2 : c ≤ MAX_DIFFICULTY) :
    step c 5 TARGET_BLOCK_TIME_SECS = c := by
  unfold TARGET_BLOCK_TIME_SECS
  have hb : stepBps 5 5 = 10000 := by rw [stepBps_five]; decide
  rw [step_eq_minmax (by decide), hb]
  rw [max_difficulty_value] at h2
  unfold U64_MAX; omega

/-- At `MAX_DIFFICULTY`, every block with a claimed gap of at least the 5 s
target keeps difficulty at `MAX_DIFFICULTY`: the ceiling is absorbing for
non-fast blocks. Proven (model). -/
theorem step_max_absorbing {o : Nat} (h : 5 ≤ o) :
    step MAX_DIFFICULTY o TARGET_BLOCK_TIME_SECS = MAX_DIFFICULTY := by
  have hle := (step_in_bounds MAX_DIFFICULTY o TARGET_BLOCK_TIME_SECS).2
  have hge : step MAX_DIFFICULTY 5 TARGET_BLOCK_TIME_SECS ≤
      step MAX_DIFFICULTY o TARGET_BLOCK_TIME_SECS := step_mono_observed h
  rw [step_on_target (by rw [max_difficulty_value]; decide) (Nat.le_refl _)] at hge
  omega

/-- If one step lands on `MIN_DIFFICULTY`, the block that made it was itself
mined at difficulty at most 3, whatever its timestamp: success probability at
most `3 / 2^64` per hash. Proven (model). -/
theorem floor_entry_requires_hard_block {c o t : Nat} (h1 : 1 ≤ c) (hc : c ≤ U64_MAX)
    (h : step c o t = MIN_DIFFICULTY) : c ≤ 3 := by
  have := (step_ratio (o := o) (t := t) h1 hc).1
  unfold MIN_DIFFICULTY at h
  omega

/-- The hardest difficulty a block can leave behind is at least half the
difficulty it was mined at (rounded down): `c ≤ 2 * step c o t + 1`. So a
miner can make the next block at most about 2x harder (in expected hashes)
than the hardest block it mined itself. Proven (model). -/
theorem step_hardening_bounded {c o t : Nat} (h1 : 1 ≤ c) (hc : c ≤ U64_MAX) :
    c ≤ 2 * step c o t + 1 := by
  have := (step_ratio (o := o) (t := t) h1 hc).1
  omega

end Botho.Difficulty
