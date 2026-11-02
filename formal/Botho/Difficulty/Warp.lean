/-
Difficulty over many blocks, and what a producer who controls timestamps can
do to it (#1841, epic #1704; formalises the #1842 finding).

What is mirrored:
- `run` / `recordBlock`: the difficulty part of
  `EmissionController::record_block` (`botho/src/block.rs`): one `step` per
  block when `observed_secs` is `Some`, no change when it is `None`.
- `observedSecs`: how `node::commit_block_locked` (`botho/src/node/mod.rs`)
  derives `observed_secs` from the parent tip timestamp:
  `Some(ts - parent)` when `parent > 0 && ts >= parent`, else `None`.
- `tsValid` / `applyChain`: the two header timestamp rules, and nothing else,
  because there is nothing else (no median-time-past rule):
  `ts >= tip_timestamp` (`ledger/store/validation.rs`, and
  `consensus/validation.rs` `validate_minting_tx`) and
  `ts <= now + MAX_FUTURE_TIMESTAMP_SECS` with a 2 h bound against the
  VALIDATING node's wall clock (`ledger/store/validation.rs`,
  `consensus/validation.rs` `validate_minting_tx_intrinsic`). `now` is an
  input of the model, one per block: **trusted** (the model assumes nothing
  about clocks except where a theorem states it).
- An honest minter stamps its minting tx, and so its block, with its own
  wall clock at the moment it finds the PoW (`node/minter.rs`, `timestamp =
  SystemTime::now()`; `consensus/block_builder.rs` copies it into the
  header). It never uses `max(now, parent)`.

Not mirrored: the SCP choice of which minting tx wins a slot (highest
`pow_priority`), the PoW search itself, and the network.

Labels: every theorem here is **proven (model)**. `run` is tested against
`EmissionController::record_block` by the trajectory vectors in
`Botho/Difficulty/Vectors.lean`; `observedSecs` and `applyChain` are read
from the code and not differentially tested.

Numeric convention as in `Step.lean`: a higher number is EASIER PoW.
-/
import Botho.Difficulty.Step

namespace Botho.Difficulty

/-- `MAX_FUTURE_TIMESTAMP_SECS` (2 h), in `consensus/validation.rs` and
`ledger/store.rs`. -/
def MAX_FUTURE_TIMESTAMP_SECS : Nat := 7200

/-- `observed_secs` in `node::commit_block_locked`. -/
def observedSecs (parent ts : Nat) : Option Nat :=
  if 0 < parent ∧ parent ≤ ts then some (ts - parent) else none

/-- The difficulty update of `EmissionController::record_block`. -/
def recordBlock (d : Nat) : Option Nat → Nat
  | none => d
  | some o => step d o TARGET_BLOCK_TIME_SECS

/-- Difficulty after a run of blocks with the given observed gaps. -/
def run (d : Nat) : List Nat → Nat
  | [] => d
  | o :: os => run (step d o TARGET_BLOCK_TIME_SECS) os

/-- The header timestamp rules against tip timestamp `tip`, for a validator
whose clock reads `now`. -/
def tsValid (tip ts now : Nat) : Bool :=
  decide (tip ≤ ts) && decide (ts ≤ now + MAX_FUTURE_TIMESTAMP_SECS)

/-- Apply a chain of blocks, each given as `(timestamp, validator clock)`, to
state `(difficulty, tip timestamp)`. `none` if any block fails a timestamp
rule. -/
def applyChain (d tip : Nat) : List (Nat × Nat) → Option (Nat × Nat)
  | [] => some (d, tip)
  | (ts, now) :: rest =>
    if tsValid tip ts now then applyChain (recordBlock d (observedSecs tip ts)) ts rest
    else none

/-- The chain whose block timestamps are `p + g₁`, `p + g₁ + g₂`, ..., all
validated by a node whose clock reads `now`. -/
def gapsChain (p now : Nat) : List Nat → List (Nat × Nat)
  | [] => []
  | g :: gs => (p + g, now) :: gapsChain (p + g) now gs

/-! ## Bounds over k blocks -/

theorem run_in_bounds {d : Nat} (os : List Nat) (h1 : 1 ≤ d) (h2 : d ≤ MAX_DIFFICULTY) :
    1 ≤ run d os ∧ run d os ≤ MAX_DIFFICULTY := by
  induction os generalizing d with
  | nil => exact ⟨h1, h2⟩
  | cons o os ih =>
    have := step_in_bounds d o TARGET_BLOCK_TIME_SECS
    exact ih this.1 this.2

theorem max_le_u64 : MAX_DIFFICULTY ≤ U64_MAX := Nat.le_of_lt max_lt_u64

/-- **The k-block bound, upper half.** Whatever the timestamps, `k` blocks
ease difficulty by at most `2^k`: `run d os ≤ 2^k * d`. Proven (model). -/
theorem run_le_pow {d : Nat} (os : List Nat) (h1 : 1 ≤ d) (h2 : d ≤ MAX_DIFFICULTY) :
    run d os ≤ 2 ^ os.length * d := by
  induction os generalizing d with
  | nil => simp [run]
  | cons o os ih =>
    have hb := step_in_bounds d o TARGET_BLOCK_TIME_SECS
    have hr := (step_ratio (o := o) (t := TARGET_BLOCK_TIME_SECS) h1
      (Nat.le_trans h2 max_le_u64)).2
    have := ih hb.1 hb.2
    simp only [run, List.length_cons, Nat.pow_succ]
    calc run (step d o TARGET_BLOCK_TIME_SECS) os
        ≤ 2 ^ os.length * step d o TARGET_BLOCK_TIME_SECS := this
      _ ≤ 2 ^ os.length * (2 * d) := Nat.mul_le_mul_left _ hr
      _ = 2 ^ os.length * 2 * d := by rw [Nat.mul_assoc]

/-- **The k-block bound, lower half.** Whatever the timestamps, `k` blocks
harden difficulty by at most `2^k`: `d / 2^k ≤ run d os`. Proven (model). -/
theorem run_ge_div_pow {d : Nat} (os : List Nat) (h1 : 1 ≤ d) (h2 : d ≤ MAX_DIFFICULTY) :
    d / 2 ^ os.length ≤ run d os := by
  induction os generalizing d with
  | nil => simp [run]
  | cons o os ih =>
    have hb := step_in_bounds d o TARGET_BLOCK_TIME_SECS
    have hr := (step_ratio (o := o) (t := TARGET_BLOCK_TIME_SECS) h1
      (Nat.le_trans h2 max_le_u64)).1
    have := ih hb.1 hb.2
    simp only [run, List.length_cons, Nat.pow_succ]
    calc d / (2 ^ os.length * 2)
        = d / 2 / 2 ^ os.length := by rw [Nat.div_div_eq_div_mul, Nat.mul_comm]
      _ ≤ step d o TARGET_BLOCK_TIME_SECS / 2 ^ os.length := Nat.div_le_div_right hr
      _ ≤ run (step d o TARGET_BLOCK_TIME_SECS) os := this

/-- Exact trajectory under sustained slow blocks: every claimed gap at least
10 s doubles, up to the ceiling. Proven (model). -/
theorem run_slow_exact {d : Nat} (os : List Nat) (hs : ∀ o ∈ os, 10 ≤ o)
    (h1 : 1 ≤ d) (h2 : d ≤ MAX_DIFFICULTY) :
    run d os = min MAX_DIFFICULTY (2 ^ os.length * d) := by
  induction os generalizing d with
  | nil => simp [run]; omega
  | cons o os ih =>
    have ho := hs o (List.mem_cons_self ..)
    have hrest : ∀ o' ∈ os, 10 ≤ o' := fun o' h => hs o' (List.mem_cons_of_mem _ h)
    have hb := step_in_bounds d o TARGET_BLOCK_TIME_SECS
    simp only [run, List.length_cons]
    rw [ih hrest hb.1 hb.2, step_slow ho h1, Nat.pow_succ]
    have hp : 1 ≤ 2 ^ os.length := Nat.one_le_two_pow
    by_cases hc : 2 * d ≤ MAX_DIFFICULTY
    · rw [Nat.min_eq_right hc, Nat.mul_assoc]
    · have hlt : MAX_DIFFICULTY < 2 * d := by omega
      rw [Nat.min_eq_left (Nat.le_of_lt hlt)]
      have e1 : MAX_DIFFICULTY ≤ 2 ^ os.length * MAX_DIFFICULTY :=
        Nat.le_mul_of_pos_left _ hp
      have e2 : 2 * d ≤ 2 ^ os.length * 2 * d := by
        rw [Nat.mul_assoc]; exact Nat.le_mul_of_pos_left _ hp
      rw [Nat.min_eq_left e1, Nat.min_eq_left (by omega)]

/-- Exact trajectory under sustained fast blocks: every claimed gap of at most
2 s (including a timestamp equal to the parent's) halves, down to the floor.
Proven (model). -/
theorem run_fast_exact {d : Nat} (os : List Nat) (hs : ∀ o ∈ os, o ≤ 2)
    (h1 : 1 ≤ d) (h2 : d ≤ MAX_DIFFICULTY) :
    run d os = max MIN_DIFFICULTY (d / 2 ^ os.length) := by
  induction os generalizing d with
  | nil => simp [run, MIN_DIFFICULTY]; omega
  | cons o os ih =>
    have ho := hs o (List.mem_cons_self ..)
    have hrest : ∀ o' ∈ os, o' ≤ 2 := fun o' h => hs o' (List.mem_cons_of_mem _ h)
    have hb := step_in_bounds d o TARGET_BLOCK_TIME_SECS
    simp only [run, List.length_cons]
    rw [ih hrest hb.1 hb.2, step_fast ho (Nat.le_trans h2 max_le_u64), Nat.pow_succ,
      Nat.mul_comm (2 ^ os.length) 2, ← Nat.div_div_eq_div_mul]
    unfold MIN_DIFFICULTY
    by_cases hd : 1 ≤ d / 2
    · rw [Nat.max_eq_right hd]
    · have h0 : d / 2 = 0 := by omega
      rw [h0, Nat.zero_div, show max 1 0 = 1 from rfl]
      have : 1 / 2 ^ os.length ≤ 1 := Nat.div_le_self _ _
      omega

/-! ## Property 3: convergence under slow blocks (#1842) -/

theorem two_pow_eight_initial : 2 ^ 8 * INITIAL_DIFFICULTY = MAX_DIFFICULTY := by
  rw [max_difficulty_value]; unfold INITIAL_DIFFICULTY; decide

theorem initial_in_range : 1 ≤ INITIAL_DIFFICULTY ∧ INITIAL_DIFFICULTY ≤ MAX_DIFFICULTY := by
  rw [max_difficulty_value]; unfold INITIAL_DIFFICULTY; decide

/-- **Property 3 (sustained gaps of 10 s or more).** From `INITIAL_DIFFICULTY`,
ANY run of 8 blocks whose claimed gaps are all at least 10 s ends exactly at
`MAX_DIFFICULTY` (the easiest PoW), and every longer such run stays there.
This is the #1842 finding: at the 10, 20 and 40 s pacing tiers (load below
5 tx/s) every honest gap is at least 10 s, so difficulty is pinned at the
ceiling from the 8th block on. Proven (model). -/
theorem easing_to_max_in_8 (os : List Nat) (hs : ∀ o ∈ os, 10 ≤ o) (hl : 8 ≤ os.length) :
    run INITIAL_DIFFICULTY os = MAX_DIFFICULTY := by
  rw [run_slow_exact os hs initial_in_range.1 initial_in_range.2]
  apply Nat.min_eq_left
  rw [← two_pow_eight_initial]
  exact Nat.mul_le_mul_right _ (Nat.pow_le_pow_right (by decide) hl)

/-- 8 is the minimum: NO run of 7 blocks reaches `MAX_DIFFICULTY` from
`INITIAL_DIFFICULTY`, whatever its timestamps. Proven (model). -/
theorem no_max_in_7 (os : List Nat) (hl : os.length ≤ 7) :
    run INITIAL_DIFFICULTY os < MAX_DIFFICULTY := by
  have h := run_le_pow os initial_in_range.1 initial_in_range.2
  have : 2 ^ os.length * INITIAL_DIFFICULTY ≤ 2 ^ 7 * INITIAL_DIFFICULTY :=
    Nat.mul_le_mul_right _ (Nat.pow_le_pow_right (by decide) hl)
  have h7 : 2 ^ 7 * INITIAL_DIFFICULTY < MAX_DIFFICULTY := by
    rw [max_difficulty_value]; unfold INITIAL_DIFFICULTY; decide
  omega

set_option maxRecDepth 100000 in
/-- **Property 3 (sustained gap of 9 s).** Exactly 10 blocks. Proven (model). -/
theorem sustained_9s_reaches_max_in_10 :
    run INITIAL_DIFFICULTY (List.replicate 9 9) < MAX_DIFFICULTY ∧
    run INITIAL_DIFFICULTY (List.replicate 10 9) = MAX_DIFFICULTY := by
  rw [max_difficulty_value]; decide

set_option maxRecDepth 100000 in
/-- **Property 3 (sustained gap of 8 s).** Exactly 12 blocks. Proven (model). -/
theorem sustained_8s_reaches_max_in_12 :
    run INITIAL_DIFFICULTY (List.replicate 11 8) < MAX_DIFFICULTY ∧
    run INITIAL_DIFFICULTY (List.replicate 12 8) = MAX_DIFFICULTY := by
  rw [max_difficulty_value]; decide

set_option maxRecDepth 100000 in
/-- **Property 3 (sustained gap of 7 s).** Exactly 17 blocks. Proven (model). -/
theorem sustained_7s_reaches_max_in_17 :
    run INITIAL_DIFFICULTY (List.replicate 16 7) < MAX_DIFFICULTY ∧
    run INITIAL_DIFFICULTY (List.replicate 17 7) = MAX_DIFFICULTY := by
  rw [max_difficulty_value]; decide

set_option maxRecDepth 100000 in
/-- **Property 3 (sustained gap of 6 s).** Exactly 31 blocks. Proven (model). -/
theorem sustained_6s_reaches_max_in_31 :
    run INITIAL_DIFFICULTY (List.replicate 30 6) < MAX_DIFFICULTY ∧
    run INITIAL_DIFFICULTY (List.replicate 31 6) = MAX_DIFFICULTY := by
  rw [max_difficulty_value]; decide

/-- Blocks at or faster than the 5 s target never ease: from any in-range
difficulty, a run of gaps all at most 5 s never rises. So a sustained gap of
5 s or less never reaches `MAX_DIFFICULTY` from `INITIAL_DIFFICULTY`.
Proven (model). -/
theorem run_le_of_le_target {d : Nat} (os : List Nat) (hs : ∀ o ∈ os, o ≤ 5)
    (h1 : 1 ≤ d) (h2 : d ≤ MAX_DIFFICULTY) : run d os ≤ d := by
  induction os generalizing d with
  | nil => exact Nat.le_refl _
  | cons o os ih =>
    have ho := hs o (List.mem_cons_self ..)
    have hrest : ∀ o' ∈ os, o' ≤ 5 := fun o' h => hs o' (List.mem_cons_of_mem _ h)
    have hle : step d o TARGET_BLOCK_TIME_SECS ≤ d := by
      have := step_mono_observed (c := d) (t := TARGET_BLOCK_TIME_SECS) ho
      rw [step_on_target h1 h2] at this
      exact this
    have hb := step_in_bounds d o TARGET_BLOCK_TIME_SECS
    exact Nat.le_trans (ih hrest hb.1 hb.2) hle

theorem sustained_le_5s_never_max (os : List Nat) (hs : ∀ o ∈ os, o ≤ 5) :
    run INITIAL_DIFFICULTY os < MAX_DIFFICULTY := by
  have := run_le_of_le_target os hs initial_in_range.1 initial_in_range.2
  have h : INITIAL_DIFFICULTY < MAX_DIFFICULTY := by
    rw [max_difficulty_value]; unfold INITIAL_DIFFICULTY; decide
  omega

/-- Once at `MAX_DIFFICULTY`, difficulty stays there for as long as every
claimed gap is at least the 5 s target. Proven (model). -/
theorem max_absorbing_run (os : List Nat) (hs : ∀ o ∈ os, 5 ≤ o) :
    run MAX_DIFFICULTY os = MAX_DIFFICULTY := by
  induction os with
  | nil => rfl
  | cons o os ih =>
    simp only [run]
    rw [step_max_absorbing (hs o (List.mem_cons_self ..))]
    exact ih (fun o' h => hs o' (List.mem_cons_of_mem _ h))

/-! ## Timestamp rules: what a producer may claim -/

theorem applyChain_gaps {d p now : Nat} (gs : List Nat) (hp : 0 < p)
    (hf : p + gs.sum ≤ now + MAX_FUTURE_TIMESTAMP_SECS) :
    applyChain d p (gapsChain p now gs) = some (run d gs, p + gs.sum) := by
  induction gs generalizing d p with
  | nil => simp [gapsChain, applyChain, run]
  | cons g gs ih =>
    simp only [List.sum_cons] at hf
    have hv : tsValid p (p + g) now = true := by
      simp only [tsValid, Bool.and_eq_true, decide_eq_true_eq]; omega
    have ho : observedSecs p (p + g) = some g := by
      have hc : 0 < p ∧ p ≤ p + g := ⟨hp, by omega⟩
      simp only [observedSecs, hc, and_self, ↓reduceIte]; congr 1; omega
    simp only [gapsChain, applyChain, hv, ↓reduceIte, ho, recordBlock, run, List.sum_cons]
    rw [ih (by omega) (by omega)]
    congr 2; omega

/-- **Property 4(b), tightness.** The upper `k`-block bound `2^k` is attained
under the real header rules: from any tip `p > 0` that the validator's clock
admits (`p + 80 ≤ now + 2 h`, true whenever `p ≤ now`), the chain stamped
`p + 10, p + 20, ..., p + 80` is valid and takes `INITIAL_DIFFICULTY` to
`MAX_DIFFICULTY`. Forward-dating needs only 80 s of claimed time, far inside
the 2 h slack, and buys nothing a run of honest 10 s blocks does not.
Proven (model). -/
theorem forward_dating_reaches_max_in_8 {p now : Nat} (hp : 0 < p)
    (hf : p + 80 ≤ now + MAX_FUTURE_TIMESTAMP_SECS) :
    (applyChain INITIAL_DIFFICULTY p (gapsChain p now (List.replicate 8 10))).map
      Prod.fst = some MAX_DIFFICULTY := by
  rw [applyChain_gaps _ hp (by simp; omega)]
  simp only [Option.map_some]
  rw [easing_to_max_in_8 _ (by simp) (by simp)]

/-- **Property 4(b), bound.** For ANY chain that passes the header timestamp
rules, `k` blocks move difficulty by at most a factor `2^k` either way. The
timestamp rules do not tighten the per-step clamp: the bound is exactly the
clamp's (see `forward_dating_reaches_max_in_8` and
`time_warp_to_floor_in_55` for attainment). Proven (model). -/
theorem applyChain_bounds {d tip d' tip' : Nat} (bs : List (Nat × Nat))
    (h1 : 1 ≤ d) (h2 : d ≤ MAX_DIFFICULTY)
    (h : applyChain d tip bs = some (d', tip')) :
    d / 2 ^ bs.length ≤ d' ∧ d' ≤ 2 ^ bs.length * d ∧ 1 ≤ d' ∧ d' ≤ MAX_DIFFICULTY := by
  induction bs generalizing d tip with
  | nil =>
    simp only [applyChain, Option.some.injEq, Prod.mk.injEq] at h
    obtain ⟨rfl, -⟩ := h
    simp; omega
  | cons b bs ih =>
    obtain ⟨ts, now⟩ := b
    simp only [applyChain] at h
    split at h
    · have hr : recordBlock d (observedSecs tip ts) / 2 ≤ d ∧
          d / 2 ≤ recordBlock d (observedSecs tip ts) ∧
          recordBlock d (observedSecs tip ts) ≤ 2 * d ∧
          1 ≤ recordBlock d (observedSecs tip ts) ∧
          recordBlock d (observedSecs tip ts) ≤ MAX_DIFFICULTY := by
        generalize observedSecs tip ts = ob
        cases ob with
        | none => simp only [recordBlock]; omega
        | some o =>
          simp only [recordBlock]
          have := step_ratio (o := o) (t := TARGET_BLOCK_TIME_SECS) h1
            (Nat.le_trans h2 max_le_u64)
          have := step_in_bounds d o TARGET_BLOCK_TIME_SECS
          unfold MIN_DIFFICULTY at *
          omega
      obtain ⟨-, hlo, hhi, hb1, hb2⟩ := hr
      obtain ⟨i1, i2, i3, i4⟩ := ih hb1 hb2 h
      simp only [List.length_cons, Nat.pow_succ]
      refine ⟨?_, ?_, i3, i4⟩
      · calc d / (2 ^ bs.length * 2)
            = d / 2 / 2 ^ bs.length := by rw [Nat.div_div_eq_div_mul, Nat.mul_comm]
          _ ≤ recordBlock d (observedSecs tip ts) / 2 ^ bs.length := Nat.div_le_div_right hlo
          _ ≤ d' := i1
      · calc d' ≤ 2 ^ bs.length * recordBlock d (observedSecs tip ts) := i2
          _ ≤ 2 ^ bs.length * (2 * d) := Nat.mul_le_mul_left _ hhi
          _ = 2 ^ bs.length * 2 * d := by rw [Nat.mul_assoc]
    · contradiction

/-! ## Property 4(a): hardening to the floor -/

theorem initial_div_pow_55 : INITIAL_DIFFICULTY / 2 ^ 55 = 1 := by
  unfold INITIAL_DIFFICULTY; decide

theorem initial_div_pow_54 : INITIAL_DIFFICULTY / 2 ^ 54 = 2 := by
  unfold INITIAL_DIFFICULTY; decide

/-- **Property 4(a), reachability in the model (counterexample to "the
timestamp rules prevent it").** From any tip `p > 0` the validator's clock
admits, the chain of 55 blocks all stamped `p` (equal to the parent, which
`ts >= parent` allows) is valid and takes `INITIAL_DIFFICULTY` to
`MIN_DIFFICULTY = 1`. Proven (model). -/
theorem time_warp_to_floor_in_55 {p now : Nat} (hp : 0 < p)
    (hf : p ≤ now + MAX_FUTURE_TIMESTAMP_SECS) :
    applyChain INITIAL_DIFFICULTY p (gapsChain p now (List.replicate 55 0)) =
      some (MIN_DIFFICULTY, p) := by
  rw [applyChain_gaps _ hp (by simp; omega)]
  rw [run_fast_exact _ (by simp) initial_in_range.1 initial_in_range.2]
  simp only [List.length_replicate]
  rw [initial_div_pow_55]
  rfl

/-- 55 is the minimum: no chain of 54 blocks, with any timestamps, reaches the
floor from `INITIAL_DIFFICULTY` (it ends at 2 or more). Proven (model). -/
theorem no_floor_in_54 (os : List Nat) (hl : os.length ≤ 54) :
    2 ≤ run INITIAL_DIFFICULTY os := by
  have h := run_ge_div_pow os initial_in_range.1 initial_in_range.2
  have : INITIAL_DIFFICULTY / 2 ^ 54 ≤ INITIAL_DIFFICULTY / 2 ^ os.length :=
    Nat.div_le_div_left (Nat.pow_le_pow_right (by decide) hl) (Nat.two_pow_pos _)
  rw [initial_div_pow_54] at this
  omega

/-- From the live state (the x1 fleet sits at `MAX_DIFFICULTY`, #1842), the
floor takes exactly 63 equal-timestamp blocks: 62 leave 2, 63 leave 1.
Proven (model). -/
theorem hardening_from_max_in_63 :
    run MAX_DIFFICULTY (List.replicate 62 0) = 2 ∧
    run MAX_DIFFICULTY (List.replicate 63 0) = 1 := by
  have hm : 1 ≤ MAX_DIFFICULTY := by rw [max_difficulty_value]; decide
  rw [run_fast_exact _ (by simp) hm (Nat.le_refl _),
    run_fast_exact _ (by simp) hm (Nat.le_refl _)]
  simp only [List.length_replicate, MIN_DIFFICULTY]
  rw [max_difficulty_value]
  decide

/-- **Property 4(a), why it is NOT a cheap halt.** Every block of a hardening
run is mined at the difficulty BEFORE it, and `step_hardening_bounded` caps
each step at a halving. So the block that lands on `MIN_DIFFICULTY` was mined
at difficulty at most 3, a success probability of at most `3 / 2^64` per
hash: about 6e18 expected hashes for that one block. More generally, to leave
the chain at difficulty `d'` the producer must itself have mined a block at
difficulty at most `2 * d' + 1`, so the next block can need at most about 2x
the expected hashes of the hardest block the attacker paid for. Proven (model). -/
theorem run_append_one (d : Nat) (os : List Nat) (o : Nat) :
    run d (os ++ [o]) = step (run d os) o TARGET_BLOCK_TIME_SECS := by
  induction os generalizing d with
  | nil => rfl
  | cons o' os ih => exact ih _

theorem floor_needs_attacker_block_at_3 {d : Nat} (os : List Nat) (o : Nat)
    (h1 : 1 ≤ d) (h2 : d ≤ MAX_DIFFICULTY)
    (h : run d (os ++ [o]) = MIN_DIFFICULTY) : run d os ≤ 3 := by
  rw [run_append_one] at h
  have hb := run_in_bounds os h1 h2
  exact floor_entry_requires_hard_block hb.1 (Nat.le_trans hb.2 max_le_u64) h

/-! ## Share of blocks: one interleaved timestamp at idle pacing -/

/-- **Property 4(a), precondition.** If an attacker controls only every other
timestamp, and the honest block after it is stamped with the honest wall
clock, the two gaps add up to the real time between honest blocks. At the
10, 20 and 40 s pacing tiers that is at least 20 s, and then the pair cannot
push difficulty below `min(c, MAX/2)` (less 1 for rounding). Hardening to the
floor therefore needs runs of consecutive attacker blocks, not one block in
two. Proven (model). -/
theorem alternating_pair_bound {c x y : Nat} (hxy : 20 ≤ x + y) (h1 : 1 ≤ c)
    (h2 : c ≤ MAX_DIFFICULTY) :
    min (MAX_DIFFICULTY / 2) c ≤
      step (step c x TARGET_BLOCK_TIME_SECS) y TARGET_BLOCK_TIME_SECS + 1 := by
  have hm := max_difficulty_value
  have hu := max_lt_u64
  by_cases hx : 10 ≤ x
  · rw [step_slow hx h1]
    have hb := step_in_bounds c x TARGET_BLOCK_TIME_SECS
    rw [step_slow hx h1] at hb
    have := (step_ratio (o := y) (t := TARGET_BLOCK_TIME_SECS) hb.1 (by omega)).1
    omega
  · have hy : 10 ≤ y := by omega
    have hb := step_in_bounds c x TARGET_BLOCK_TIME_SECS
    rw [step_slow hy hb.1]
    have := (step_ratio (o := x) (t := TARGET_BLOCK_TIME_SECS) h1 (by omega)).1
    omega

/-! ## Forward-dating locks honest minters out (counterexample) -/

theorem applyChain_append {d tip d' tip' : Nat} (bs cs : List (Nat × Nat))
    (h : applyChain d tip bs = some (d', tip')) :
    applyChain d tip (bs ++ cs) = applyChain d' tip' cs := by
  induction bs generalizing d tip with
  | nil =>
    simp only [applyChain, Option.some.injEq, Prod.mk.injEq] at h
    obtain ⟨rfl, rfl⟩ := h
    rfl
  | cons b bs ih =>
    obtain ⟨ts, now⟩ := b
    simp only [applyChain] at h
    simp only [List.cons_append, applyChain]
    split at h
    · rename_i hv; simp only [hv, ↓reduceIte]; exact ih h
    · contradiction

/-- **Counterexample (liveness of honest minting).** A block stamped with the
maximum future offset, `ts = now_a + 2 h`, is valid for a validator whose clock
reads `now_a`. After it, a block from an honest minter, stamped with the
honest wall clock `now_h`, is REJECTED for as long as `now_h < now_a + 2 h`:
honest minting is locked out for up to 2 h by one block. Proven (model);
the honest-stamp rule is read from `node/minter.rs`. -/
theorem forward_date_locks_out_honest {d tip d' tip' now_a now_h : Nat}
    (bs : List (Nat × Nat)) (h : applyChain d tip bs = some (d', tip'))
    (hv : tip' ≤ now_a + MAX_FUTURE_TIMESTAMP_SECS)
    (hh : now_h < now_a + MAX_FUTURE_TIMESTAMP_SECS) :
    (applyChain d tip (bs ++ [(now_a + MAX_FUTURE_TIMESTAMP_SECS, now_a)])).isSome ∧
    applyChain d tip (bs ++ [(now_a + MAX_FUTURE_TIMESTAMP_SECS, now_a),
      (now_h, now_h)]) = none := by
  rw [applyChain_append _ _ h, applyChain_append _ _ h]
  have hv' : tsValid tip' (now_a + MAX_FUTURE_TIMESTAMP_SECS) now_a = true := by
    simp only [tsValid, Bool.and_eq_true, decide_eq_true_eq]; omega
  have hn : tsValid (now_a + MAX_FUTURE_TIMESTAMP_SECS) now_h now_h = false := by
    simp only [tsValid, Bool.and_eq_false_iff, decide_eq_false_iff_not]; omega
  simp [applyChain, hv', hn]

/-- An attacker keeps the lock indefinitely: stamping EVERY block it produces
with its validators' clock plus 2 h is valid for any non-decreasing sequence
of clock readings, so the tip always sits 2 h ahead of the wall clock and no
honest block (`forward_date_locks_out_honest`) is valid while the attacker
keeps producing. The attacker's own gaps are then the real gaps, so the
controller sees nothing unusual. Proven (model). -/
theorem forward_dated_chain_valid {d tip : Nat} (nows : List Nat) (n0 : Nat)
    (hsorted : ∀ i j, i ≤ j → j < nows.length → nows[i]! ≤ nows[j]!)
    (h0 : tip ≤ n0 + MAX_FUTURE_TIMESTAMP_SECS) (hfirst : ∀ n ∈ nows, n0 ≤ n) :
    (applyChain d tip (nows.map fun n => (n + MAX_FUTURE_TIMESTAMP_SECS, n))).isSome := by
  induction nows generalizing d tip n0 with
  | nil => rfl
  | cons n ns ih =>
    have hn : n0 ≤ n := hfirst n (List.mem_cons_self ..)
    have hv : tsValid tip (n + MAX_FUTURE_TIMESTAMP_SECS) n = true := by
      simp only [tsValid, Bool.and_eq_true, decide_eq_true_eq]; omega
    simp only [List.map_cons, applyChain, hv, ↓reduceIte]
    apply ih n
    · intro i j hij hj
      have := hsorted (i + 1) (j + 1) (by omega) (by simp; omega)
      simpa using this
    · omega
    · intro m hm
      obtain ⟨k, hk, rfl⟩ := List.getElem_of_mem hm
      have := hsorted 0 (k + 1) (by omega) (by simp; omega)
      simpa [hk] using this

end Botho.Difficulty
