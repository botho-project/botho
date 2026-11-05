/-
Checked production monetary projection (#1921), source ae6655e5.
Core model/proofs, not extraction or whole-ledger/controller equivalence.
Rust ordinary u128 arithmetic panics on overflow in the release profile;
`Error` records those boundaries. A successful narrowing to zero stays zero.
The production constants below are not the generic or fast-test policy.
-/
import Botho.Supply.Block

namespace Botho.Monetary
open Botho.Lottery

/-- Picocredits: 50 BTH, with 10^12 pico per BTH. -/
def initialReward : Nat := 50000000000000
def halvingInterval : Nat := 6307200
def halvingCount : Nat := 5
def secondsPerYear : Nat := 31536000
def referenceBlockTime : Nat := 5
def blocksPerYear : Nat := secondsPerYear / referenceBlockTime
def tailEmissionStartHeight : Nat := halvingInterval * halvingCount
def isHalvingPhase (height : Nat) : Bool := height < tailEmissionStartHeight

def halvingReward (height : Nat) : Option Nat :=
  if isHalvingPhase height then some (initialReward >>> (height / halvingInterval)) else none

/-- Invalid-width errors belong to the Nat input boundary; the other errors
name Rust arithmetic panics, not a production Result-returning API. -/
inductive Error where
  | heightWidth | supplyWidth | netMultiply | burnMultiply | annualAdd
  | heightSuccessor | grossAdd
  deriving DecidableEq, Repr

def annualNet (supply : Nat) : Nat := supply * 200 / 10000
def annualBurn (supply : Nat) : Nat := supply * 50 / 10000
def annualGross (supply : Nat) : Nat := annualNet supply + annualBurn supply
def tailPreCast (supply : Nat) : Nat := max 1 (annualGross supply / blocksPerYear)

/-- Check exactly the two products and the sum, in source evaluation order.
`max(1)` precedes the final `as u64` modulo narrowing. -/
def tailRewardU128 (supply : Nat) : Except Error Nat :=
  if supply > U128_MAX then .error .supplyWidth
  else if supply * 200 > U128_MAX then .error .netMultiply
  else if supply * 50 > U128_MAX then .error .burnMultiply
  else if annualGross supply > U128_MAX then .error .annualAdd
  else .ok (truncU64 (tailPreCast supply))

/-- The u64-input wrapper widens its input before calling the same helper. -/
def tailRewardU64 (supply : Nat) : Except Error Nat :=
  if supply > U64_MAX then .error .supplyWidth else tailRewardU128 supply

def blockRewardAt (height supply : Nat) : Except Error Nat :=
  if height > U64_MAX then .error .heightWidth
  else if supply > U128_MAX then .error .supplyWidth
  else if isHalvingPhase height then .ok ((halvingReward height).getD 1)
  else tailRewardU128 supply

/-- Scalar projection only: checked next height, that height's reward from
current gross issuance, then checked u128 gross addition. No difficulty,
fee statistics, simulation u64 shadows or asserted closure of a safe budget. -/
structure Successor where
  height : Nat
  reward : Nat
  gross : Nat
  deriving DecidableEq, Repr

def issuanceSuccessor (height supply : Nat) : Except Error Successor := do
  if height > U64_MAX then .error .heightWidth
  else match checkedAdd U64_MAX height 1 with
  | none => .error .heightSuccessor
  | some next =>
    let reward ← blockRewardAt next supply
    match checkedAdd U128_MAX supply reward with
    | none => .error .grossAdd
    | some gross => .ok ⟨next, reward, gross⟩

def demurrageRateBps (height : Nat) : Nat :=
  if halvingInterval = 0 ∨ height < halvingInterval then 0 else 200

/-- A sufficient one-call supply budget, not an invariant or a reachable
horizon. Its exact value is derived, not assumed as an intermediate bound. -/
def safeSupply : Nat := U64_MAX * halvingInterval * 40

/-- All typed heights select a safe phase. In Phase 1, the shift is <5<64;
the threshold and divisors are fixed nonzero production constants. -/
theorem phase_safe (height : Nat) (_h : height ≤ U64_MAX) :
    0 < halvingInterval ∧ 0 < referenceBlockTime ∧ blocksPerYear = halvingInterval ∧
    tailEmissionStartHeight = 31536000 ∧ tailEmissionStartHeight ≤ U64_MAX ∧
    (isHalvingPhase height = true ↔ height < 31536000) ∧
    (isHalvingPhase height = true → height / halvingInterval < 5 ∧
      height / halvingInterval < 64 ∧
      halvingReward height = some (initialReward / 2 ^ (height / halvingInterval))) ∧
    (isHalvingPhase height = false → halvingReward height = none) := by
  refine ⟨by decide, by decide, rfl, rfl, by decide, ?_, ?_, ?_⟩
  · change decide (height < 31536000) = true ↔ height < 31536000
    exact decide_eq_true_iff
  · intro hp
    have hlt : height < 31536000 := of_decide_eq_true hp
    have hq : height / halvingInterval < 5 := by unfold halvingInterval; omega
    exact ⟨hq, by omega, by simp [halvingReward, hp, Nat.shiftRight_eq_div_pow]⟩
  · intro hp; simp [halvingReward, hp]

/-- Only the halving branch is monotone; no cross-phase monotonicity claim. -/
theorem halving_nonincreasing (a b : Nat) (hab : a ≤ b)
    (hb : b < tailEmissionStartHeight) :
    ∃ ra rb, halvingReward a = some ra ∧ halvingReward b = some rb ∧ rb ≤ ra := by
  have ha : a < tailEmissionStartHeight := by omega
  have hd := Nat.div_le_div_right hab (c := halvingInterval)
  have hp := Nat.pow_le_pow_right (by decide : 0 < 2) hd
  refine ⟨initialReward / 2 ^ (a / halvingInterval), initialReward / 2 ^ (b / halvingInterval),
    ?_, ?_, Nat.div_le_div_left hp (Nat.two_pow_pos _)⟩ <;>
    simp [halvingReward, isHalvingPhase, ha, hb, Nat.shiftRight_eq_div_pow]

/-- Every successful call, including arithmetic-safe wide supplies, returns
the actual modulo result. Positivity is intentionally absent here. -/
theorem tail_success_iff (supply reward : Nat) (hs : supply ≤ U128_MAX) :
    tailRewardU128 supply = .ok reward ↔
      supply * 200 ≤ U128_MAX ∧ supply * 50 ≤ U128_MAX ∧
      annualGross supply ≤ U128_MAX ∧ reward = truncU64 (tailPreCast supply) := by
  unfold tailRewardU128
  simp only [show ¬supply > U128_MAX by omega, ↓reduceIte]
  split <;> rename_i hn
  · simp; omega
  · split <;> rename_i hb
    · simp; omega
    · split <;> rename_i hg
      · simp; omega
      · simp only [Except.ok.injEq]
        constructor
        · intro h; exact ⟨by omega, by omega, by omega, h.symm⟩
        · intro h; exact h.2.2.2.symm

private theorem safe_bounds (supply : Nat) (hs : supply ≤ safeSupply) :
    supply ≤ U128_MAX ∧ supply * 200 ≤ U128_MAX ∧ supply * 50 ≤ U128_MAX ∧
    annualGross supply ≤ U128_MAX ∧ tailPreCast supply ≤ U64_MAX ∧ 1 ≤ tailPreCast supply := by
  have hn := Nat.div_le_div_right (Nat.mul_le_mul_right 200 hs) (c := 10000)
  have hb := Nat.div_le_div_right (Nat.mul_le_mul_right 50 hs) (c := 10000)
  have hg : annualGross supply ≤ annualGross safeSupply := by unfold annualGross annualNet annualBurn; omega
  have hr := Nat.div_le_div_right hg (c := blocksPerYear)
  have hmax := Nat.le_max_left 1 (annualGross supply / blocksPerYear)
  have hv : annualGross safeSupply / blocksPerYear = U64_MAX := by decide
  have ht : tailPreCast supply ≤ U64_MAX := by
    unfold tailPreCast
    exact Nat.max_le.mpr ⟨by decide, by omega⟩
  refine ⟨?_, ?_, ?_, ?_, ht, hmax⟩
  all_goals unfold safeSupply halvingInterval U64_MAX U128_MAX annualGross annualNet annualBurn at * <;> omega

/-- Safe products/sum, lossless narrowing and positive reward are derived
from the supply bound. No intermediate result bound is an input premise. -/
theorem tail_safe_domain (supply : Nat) (hs : supply ≤ safeSupply) :
    supply * 200 ≤ U128_MAX ∧ supply * 50 ≤ U128_MAX ∧ annualGross supply ≤ U128_MAX ∧
    tailRewardU128 supply = .ok (tailPreCast supply) ∧
    1 ≤ tailPreCast supply ∧ tailPreCast supply ≤ U64_MAX := by
  have hb := safe_bounds supply hs
  have hc : truncU64 (tailPreCast supply) = tailPreCast supply := by
    unfold truncU64
    apply Nat.mod_eq_of_lt
    have hm : U64_MAX + 1 = 2 ^ 64 := by decide
    omega
  refine ⟨hb.2.1, hb.2.2.1, hb.2.2.2.1, ?_, hb.2.2.2.2.2, hb.2.2.2.2.1⟩
  exact (tail_success_iff supply _ hb.1).2 ⟨hb.2.1, hb.2.2.1, hb.2.2.2.1, hc.symm⟩

/-- Every u64 input lies inside the sufficient domain; the wrapper's widening
is exact. Neither this nor the u128 theorem closes the domain under issuance. -/
theorem tail_u64_domain (supply : Nat) (hs : supply ≤ U64_MAX) :
    supply ≤ safeSupply ∧ tailRewardU64 supply = tailRewardU128 supply ∧
    tailRewardU64 supply = .ok (tailPreCast supply) ∧
    1 ≤ tailPreCast supply ∧ tailPreCast supply ≤ U64_MAX := by
  have hm : U64_MAX ≤ safeSupply := by decide
  have hb : supply ≤ safeSupply := by omega
  have ht := tail_safe_domain supply hb
  have hw : tailRewardU64 supply = tailRewardU128 supply := by simp [tailRewardU64, show ¬supply > U64_MAX by omega]
  exact ⟨hb, hw, hw.trans ht.2.2.2.1, ht.2.2.2.2⟩

/-- The current policy holiday and subsequent rate; no resolution of #1685. -/
theorem demurrage_current (height : Nat) :
    (height < halvingInterval → demurrageRateBps height = 0) ∧
    (halvingInterval ≤ height → demurrageRateBps height = 200) := by
  constructor <;> intro h <;> simp [demurrageRateBps, halvingInterval] at * <;> omega


private theorem halving_value_bounds (height : Nat) (hp : height < tailEmissionStartHeight) :
    1 ≤ (halvingReward height).getD 1 ∧ (halvingReward height).getD 1 ≤ U64_MAX := by
  have hq : height / halvingInterval ≤ 4 := by
    unfold tailEmissionStartHeight halvingInterval halvingCount at *; omega
  have hpow : 2 ^ (height / halvingInterval) ≤ 16 :=
    Nat.pow_le_pow_right (by decide) hq
  have hlo := Nat.div_le_div_left hpow (Nat.two_pow_pos _) (a := initialReward)
  have hhi := Nat.div_le_self initialReward (2 ^ (height / halvingInterval))
  have hr : initialReward ≤ U64_MAX := by decide
  have hfloor : initialReward / 16 ≥ 1 := by decide
  simpa [halvingReward, isHalvingPhase, hp, Nat.shiftRight_eq_div_pow] using
    (show 1 ≤ initialReward / 2 ^ (height / halvingInterval) ∧
      initialReward / 2 ^ (height / halvingInterval) ≤ U64_MAX from ⟨by omega, by omega⟩)

/-- A typed height and the sufficient supply domain make either production
branch safe; halving ignores the current supply after its input-width check. -/
theorem block_reward_safe (height supply : Nat) (hh : height ≤ U64_MAX)
    (hs : supply ≤ safeSupply) :
    ∃ reward, blockRewardAt height supply = .ok reward ∧ 1 ≤ reward ∧ reward ≤ U64_MAX := by
  have hb := safe_bounds supply hs
  have hn : ¬height > U64_MAX := by omega
  have hw : ¬supply > U128_MAX := by omega
  by_cases hp : height < tailEmissionStartHeight
  · exact ⟨(halvingReward height).getD 1,
      by simp [blockRewardAt, hn, hw, isHalvingPhase, hp], halving_value_bounds height hp⟩
  · have ht := tail_safe_domain supply hs
    exact ⟨tailPreCast supply,
      by simpa [blockRewardAt, hn, hw, isHalvingPhase, hp] using ht.2.2.2.1, ht.2.2.2.2⟩

/-- Safe one-step exact addition and widths, derived from h<M and S≤B.
No claim asserts nextGross≤B or any inductive/reachable horizon. -/
theorem successor_safe (height supply : Nat) (hh : height < U64_MAX)
    (hs : supply ≤ safeSupply) :
    ∃ after, issuanceSuccessor height supply = .ok after ∧
      after.height = height + 1 ∧ after.height ≤ U64_MAX ∧
      blockRewardAt after.height supply = .ok after.reward ∧
      1 ≤ after.reward ∧ after.reward ≤ U64_MAX ∧
      after.gross = supply + after.reward ∧ after.gross ≤ U128_MAX := by
  have hb : safeSupply + U64_MAX < 2 ^ 128 := by decide
  have hu : U128_MAX + 1 = 2 ^ 128 := by decide
  have hn : height + 1 ≤ U64_MAX := by omega
  obtain ⟨reward, hr, hlo, hhi⟩ := block_reward_safe (height + 1) supply hn hs
  have hg : supply + reward ≤ U128_MAX := by omega
  refine ⟨⟨height + 1, reward, supply + reward⟩, ?_, rfl, hn, hr, hlo, hhi, rfl, hg⟩
  simp [issuanceSuccessor, show ¬height > U64_MAX by omega, checkedAdd, hn, hr, hg,
    bind, Except.bind]

private theorem reward_not_height_error (height supply : Nat) :
    blockRewardAt height supply ≠ .error .heightSuccessor := by
  intro h
  unfold blockRewardAt tailRewardU128 at h
  split at h
  · cases h
  · split at h
    · cases h
    · split at h
      · cases h
      · split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · cases h

/-- On typed heights, max height is exactly the checked successor failure.
For a successful next reward, gross-add failure is exactly overflow of the
current gross plus that reward. This need not be reachable from genesis. -/
theorem successor_failures (height supply reward : Nat) (hh : height ≤ U64_MAX) :
    (issuanceSuccessor height supply = .error .heightSuccessor ↔ height = U64_MAX) ∧
    (height < U64_MAX → blockRewardAt (height + 1) supply = .ok reward →
      (issuanceSuccessor height supply = .error .grossAdd ↔ U128_MAX < supply + reward)) := by
  have hn : ¬height > U64_MAX := by omega
  constructor
  · by_cases hm : height = U64_MAX
    · subst height; simp [issuanceSuccessor, checkedAdd, show ¬U64_MAX + 1 ≤ U64_MAX by omega]
    · have hnext : height + 1 ≤ U64_MAX := by omega
      have hc : checkedAdd U64_MAX height 1 = some (height + 1) := by simp [checkedAdd, hnext]
      simp only [issuanceSuccessor, hn, ↓reduceIte, hc]
      cases hr : blockRewardAt (height + 1) supply with
      | error e =>
        have he : e ≠ .heightSuccessor := by
          intro h; subst e; exact reward_not_height_error _ _ hr
        simp [bind, Except.bind, he, hm]
      | ok r =>
        by_cases hg : supply + r ≤ U128_MAX <;> simp [bind, Except.bind, checkedAdd, hg, hm]
  · intro hlt hr
    have hnext : height + 1 ≤ U64_MAX := by omega
    by_cases hg : supply + reward ≤ U128_MAX <;>
      simp [issuanceSuccessor, hn, checkedAdd, hnext, hr, bind, Except.bind, hg] <;> omega

/-- Bind the actual checked production reward to minting/header height and
the existing emission model. Reward width is derived from the safe-domain
call, and every clamp/cast/product/split fact is reused from Supply.Block. -/
theorem lottery_emission_bound (headerHeight mintHeight supply reward : Nat)
    (hh : headerHeight ≤ U64_MAX) (hs : supply ≤ safeSupply)
    (hm : mintHeight = headerHeight)
    (hr : blockRewardAt headerHeight supply = .ok reward) :
    Botho.Supply.Block.productionInterval = halvingInterval ∧
    1 ≤ reward ∧ reward ≤ U64_MAX ∧
    Botho.Supply.Block.emissionEpoch mintHeight halvingInterval ≤ U32_MAX ∧
    Botho.Supply.Block.emissionEpoch mintHeight halvingInterval % 2 ^ 32 =
      Botho.Supply.Block.emissionEpoch mintHeight halvingInterval ∧
    Botho.Supply.Block.emissionBps mintHeight halvingInterval ≤ 5000 ∧
    reward * Botho.Supply.Block.emissionBps mintHeight halvingInterval ≤ U128_MAX ∧
    Botho.Supply.Block.emission mintHeight reward halvingInterval =
      reward * Botho.Supply.Block.emissionBps mintHeight halvingInterval / 10000 ∧
    Botho.Supply.Block.emission mintHeight reward halvingInterval ≤ reward ∧
    Botho.Supply.Block.coinbaseAmount mintHeight reward halvingInterval +
      Botho.Supply.Block.emission mintHeight reward halvingInterval = reward := by
  obtain ⟨r, hcall, hlo, hhi⟩ := block_reward_safe headerHeight supply hh hs
  have he : r = reward := Except.ok.inj (hcall.symm.trans hr)
  subst r
  have h := Botho.Supply.Block.emission_exact mintHeight reward halvingInterval
    (by omega) hhi (by decide)
  exact ⟨rfl, hlo, hhi, h⟩

/-- The actual helper reward, made total with zero only outside Phase 1. -/
def halvingAmount (height : Nat) : Nat := (halvingReward height).getD 0

/-- Efficient finite epoch computation. `phase1_exact_sum` proves its equality
to the actual accepted per-height schedule, with genesis excluded. -/
def nominalPhase1 : Nat :=
  ((List.range halvingCount).map (fun k => halvingInterval * (initialReward / 2 ^ k))).sum
def acceptedPhase1 : Nat := nominalPhase1 - initialReward

private theorem epoch_reward (epoch offset : Nat) (he : epoch < halvingCount)
    (hi : offset < halvingInterval) :
    halvingAmount (epoch * halvingInterval + offset) = initialReward / 2 ^ epoch := by
  have hq : (epoch * halvingInterval + offset) / halvingInterval = epoch := by
    rw [Nat.add_comm, Nat.add_mul_div_right _ _ (by decide : 0 < halvingInterval),
      Nat.div_eq_of_lt hi]
    simp
  have hp : epoch * halvingInterval + offset < tailEmissionStartHeight := by
    unfold tailEmissionStartHeight halvingInterval halvingCount at *; omega
  simp [halvingAmount, halvingReward, isHalvingPhase, hp, hq, Nat.shiftRight_eq_div_pow]


private theorem epoch_sum (epoch : Nat) (he : epoch < halvingCount) :
    ((List.range halvingInterval).map (fun i => halvingAmount (epoch * halvingInterval + i))).sum =
      halvingInterval * (initialReward / 2 ^ epoch) := by
  have hm : (List.range halvingInterval).map (fun i => halvingAmount (epoch * halvingInterval + i)) =
      (List.range halvingInterval).map (fun _ => initialReward / 2 ^ epoch) := by
    apply List.map_congr_left
    intro i hi
    exact epoch_reward epoch i he (List.mem_range.mp hi)
  rw [hm, List.map_const', List.length_range, List.sum_replicate_nat]


private theorem prefix_epochs (epochs : Nat) (he : epochs ≤ halvingCount) :
    ((List.range (epochs * halvingInterval)).map halvingAmount).sum =
      ((List.range epochs).map (fun k => halvingInterval * (initialReward / 2 ^ k))).sum := by
  induction epochs with
  | zero => simp
  | succ n ih =>
    have hn : n < halvingCount := by omega
    rw [Nat.succ_mul, List.range_add, List.map_append, List.sum_append, List.map_map,
      ih (by omega)]
    simp only [Function.comp_def]
    rw [epoch_sum n hn, List.range_succ, List.map_append, List.sum_append]
    simp



private theorem split_genesis (n : Nat) :
    ((List.range (n + 1)).map halvingAmount).sum =
      halvingAmount 0 + ((List.range n).map (fun i => halvingAmount (i + 1))).sum := by
  rw [List.range_succ_eq_map, List.map_cons, List.sum_cons, List.map_map]
  rfl



private theorem accepted_from_partition (inclusive accepted nominal genesisReward : Nat)
    (hn : inclusive = nominal) (ha : inclusive = genesisReward + accepted) :
    accepted = nominal - genesisReward := by omega

/-- The finite five-epoch calculation equals the actual sum of rewards at
accepted heights 1..5H-1. The proof partitions the per-height list rather
than evaluating thirty-one million cells. Genesis accounts for exactly 50 BTH. -/
theorem phase1_exact_sum :
    ((List.range (tailEmissionStartHeight - 1)).map (fun i => halvingAmount (i + 1))).sum =
      acceptedPhase1 ∧
    nominalPhase1 = 611010000000000000000 ∧
    acceptedPhase1 + initialReward = nominalPhase1 ∧
    acceptedPhase1 = 611009950000000000000 ∧
    acceptedPhase1 / 1000000000000 = 611009950 := by
  have idx : tailEmissionStartHeight = halvingCount * halvingInterval := by decide
  have transport := congrArg (fun n => ((List.range n).map halvingAmount).sum) idx
  have hp := prefix_epochs halvingCount (Nat.le_refl _)
  have nominal := transport.trans hp
  have hz : halvingAmount 0 = initialReward := by decide
  have hr : tailEmissionStartHeight = (tailEmissionStartHeight - 1) + 1 := by decide
  have splitIndex := congrArg (fun n => ((List.range n).map halvingAmount).sum) hr
  have splitRows := split_genesis (tailEmissionStartHeight - 1)
  have zero := congrArg (fun r => r +
    ((List.range (tailEmissionStartHeight - 1)).map (fun i => halvingAmount (i + 1))).sum) hz
  have partition := splitIndex.trans (splitRows.trans zero)
  have actual := accepted_from_partition _ _ _ _ nominal partition
  exact ⟨actual, by decide, by decide, by decide, by decide⟩


/-- Synthetic typed helper input with safe arithmetic but a zero narrowing.
This is not a genesis-reachable accepted-block witness. -/
def zeroCastSupply : Nat := (U64_MAX + 1) * halvingInterval * 40

/-- Numeric domain facts, including the strict headroom for one gross add. -/
theorem production_domain_numbers :
    safeSupply = 4653892168868035357845120000 ∧ U64_MAX ≤ safeSupply ∧
    safeSupply + U64_MAX < 2 ^ 128 ∧
    zeroCastSupply = 4653892168868035358097408000 := by decide

/-- Two independent refutations: safe arithmetic can narrow to zero; a typed
u128 supply can panic at the first product. max(1) is before the narrowing. -/
theorem synthetic_tail_counterexamples :
    zeroCastSupply ≤ U128_MAX ∧ zeroCastSupply * 200 ≤ U128_MAX ∧
    zeroCastSupply * 50 ≤ U128_MAX ∧ annualGross zeroCastSupply ≤ U128_MAX ∧
    tailPreCast zeroCastSupply = 2 ^ 64 ∧
    tailRewardU128 zeroCastSupply = .ok 0 ∧
    tailRewardU128 U128_MAX = .error .netMultiply := by
  exact ⟨by decide, by decide, by decide, by decide, by decide, rfl, rfl⟩

theorem unrestricted_tail_claims_refuted :
    ¬(∀ s, s ≤ U128_MAX → ∃ r, tailRewardU128 s = .ok r) ∧
    ¬(∀ s r, s ≤ U128_MAX → tailRewardU128 s = .ok r → 1 ≤ r) := by
  constructor
  · intro h
    obtain ⟨r, hr⟩ := h U128_MAX (Nat.le_refl _)
    rw [synthetic_tail_counterexamples.2.2.2.2.2.2] at hr
    cases hr
  · intro h
    have hz := h zeroCastSupply 0 synthetic_tail_counterexamples.1
      synthetic_tail_counterexamples.2.2.2.2.2.1
    omega

private def resultValue : Except Error Nat → Option Nat
  | .ok n => some n
  | .error _ => none

/-- Kernel values matching the actual-source Rust tests; B+1 is a sample,
not an extension of the quantified sufficient-domain theorem. -/
theorem tail_boundary_values :
    [0, 1, U64_MAX, safeSupply - 1, safeSupply, safeSupply + 1, zeroCastSupply].map
      (fun s => resultValue (tailRewardU128 s)) =
      [some 1, some 1, some 73117802169, some (U64_MAX - 1),
        some U64_MAX, some U64_MAX, some 0] := by decide

/-- Typed phase boundaries include the helper's reward at height zero,
every kH±1 boundary, the first tail height and maximum u64 height. -/
theorem phase_boundary_values :
    ([0, 1] ++ (List.range 5).flatMap (fun i =>
      [(i + 1) * halvingInterval - 1, (i + 1) * halvingInterval,
       (i + 1) * halvingInterval + 1]) ++ [U64_MAX]).map halvingReward =
      [some 50000000000000, some 50000000000000,
       some 50000000000000, some 25000000000000, some 25000000000000,
       some 25000000000000, some 12500000000000, some 12500000000000,
       some 12500000000000, some 6250000000000, some 6250000000000,
       some 6250000000000, some 3125000000000, some 3125000000000,
       some 3125000000000, none, none, none] := by decide

/-- Uses the derived accepted total, not the nominal inclusive-zero total. -/
theorem first_tail_value :
    blockRewardAt tailEmissionStartHeight acceptedPhase1 = .ok 2421874801813 := by rfl

/-- Synthetic scalar examples distinguish checked height/gross failures and
show B is not closed under even one safe halving step. They are not full
controller transitions or genesis-reachable states. -/
theorem successor_boundary_values :
    issuanceSuccessor (U64_MAX - 1) 0 = .ok ⟨U64_MAX, 1, 1⟩ ∧
    issuanceSuccessor U64_MAX 0 = .error .heightSuccessor ∧
    issuanceSuccessor 0 U128_MAX = .error .grossAdd ∧
    issuanceSuccessor 0 safeSupply = .ok ⟨1, initialReward, safeSupply + initialReward⟩ ∧
    safeSupply < safeSupply + initialReward := by
  exact ⟨rfl, rfl, rfl, rfl, by decide⟩

end Botho.Monetary
