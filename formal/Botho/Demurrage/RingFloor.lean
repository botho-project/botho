/-
Amount-matched ring floor (#1686, protocol 10.0.0, ADR 0015). Models
`cluster-tax/src/ring_floor.rs::amount_matched_ring_floor_wealth` and
`amount_matched_ring_floor_factor`, the floor `consensus_fee_floor` applies to
the demurrage factor since 10.0.0 (through
`ValidationReads::ring_floored_factor_for_tx`). It replaces the pooled
value-weighted centroid `ringFactorExact` (`Exact.lean`) on the consensus
path; that model stays as the model of `ring_centroid_implied_factor`, which
the simulations still call.

Per input: the members whose amount equals the input's pseudo-output amount
(every member if none does), as `tag_retention.rs::amount_matched` selects
them; the input's floor wealth is the largest of their wealths. Across inputs:
the exact mean of those floor wealths weighted by pseudo-output amount
(`wide::WeightedMean`, 256-bit numerator), clamped at `u128::MAX` once, then
the default curve.

Status: proposal of five full-Nat model properties for #2032/#2017; tested
correspondence remains finite DRT (leg `ring_floor_factor`). These proposed
proofs do not land until their statement records are approved on the base.
They do not establish Rust equivalence or a general no-overcharge property.
-/
import Botho.Demurrage.Exact

namespace Botho.Demurrage
open Botho.Lottery

/-- `tag_retention.rs::amount_matched` over `(amount, wealth)` members: the
members of the pseudo-output amount, or every member if none matches. -/
def amountMatched (pseudo : Nat) (members : List (Nat × Nat)) : List (Nat × Nat) :=
  let matching := members.filter (fun m => m.1 == pseudo)
  if matching.isEmpty then members else matching

/-- `ring_floor.rs::input_floor_wealth`: the largest wealth among the
amount-matched members; 0 for an empty ring. -/
def inputFloorWealth (pseudo : Nat) (members : List (Nat × Nat)) : Nat :=
  (amountMatched pseudo members).foldl (fun acc m => Nat.max acc m.2) 0

/-- `ring_floor.rs::amount_matched_ring_floor_wealth` over
`(pseudo amount, members)` inputs: the pseudo-weighted exact mean of the
per-input floor wealths, clamped once at the end; 0 with no value. -/
def ringFloorWealth (inputs : List (Nat × List (Nat × Nat))) : Nat :=
  let (weighted, total) := inputs.foldl (fun (a : Nat × Nat) i =>
    (a.1 + i.1 * inputFloorWealth i.1 i.2, a.2 + i.1)) (0, 0)
  if total = 0 then 0 else minN (weighted / total) U128_MAX

/-- `ring_floor.rs::amount_matched_ring_floor_factor` with the default curve. -/
def ringFloorFactor (inputs : List (Nat × List (Nat × Nat))) : Nat :=
  defaultFactor (ringFloorWealth inputs)

end Botho.Demurrage

namespace Botho.Demurrage
open Botho.Lottery

-- Private facts about the canonical maximum fold, including its accumulator.
private theorem floor_fold_ge_acc (members : List (Nat × Nat)) (a : Nat) :
    a ≤ members.foldl (fun acc m => Nat.max acc m.2) a := by
  induction members generalizing a with
  | nil => simp
  | cons m ms ih =>
    exact Nat.le_trans (Nat.le_max_left a m.2) (ih (Nat.max a m.2))

private theorem floor_fold_ge_member (members : List (Nat × Nat))
    (m : Nat × Nat) (a : Nat) (hm : m ∈ members) :
    m.2 ≤ members.foldl (fun acc m => Nat.max acc m.2) a := by
  induction members generalizing a with
  | nil => simp at hm
  | cons x xs ih =>
    rcases List.mem_cons.mp hm with h | h
    · subst m
      exact Nat.le_trans (Nat.le_max_right a x.2)
        (floor_fold_ge_acc xs (Nat.max a x.2))
    · exact ih (Nat.max a x.2) h

private theorem floor_fold_le (members : List (Nat × Nat)) (a bound : Nat)
    (ha : a ≤ bound) (hm : ∀ m ∈ members, m.2 ≤ bound) :
    members.foldl (fun acc m => Nat.max acc m.2) a ≤ bound := by
  induction members generalizing a with
  | nil => exact ha
  | cons m ms ih =>
    apply ih (Nat.max a m.2)
    · exact Nat.max_le.mpr ⟨ha, hm m (by simp)⟩
    · intro x hx
      exact hm x (by simp [hx])

private theorem matched_of_match (p : Nat) (members : List (Nat × Nat))
    (hm : ∃ w, (p, w) ∈ members) :
    amountMatched p members = members.filter (fun m => m.1 == p) := by
  obtain ⟨w, hw⟩ := hm
  have hmem : (p, w) ∈ members.filter (fun m => m.1 == p) := by
    simp [hw]
  have hn : (members.filter (fun m => m.1 == p)).isEmpty = false :=
    List.isEmpty_eq_false_iff_exists_mem.mpr ⟨(p, w), hmem⟩
  simp [amountMatched, hn]

theorem inputFloorWealth_ge_spent
    (p w : Nat) (members : List (Nat × Nat))
    (hspent : (p, w) ∈ members) :
    w ≤ inputFloorWealth p members := by
  rw [inputFloorWealth, matched_of_match p members ⟨w, hspent⟩]
  exact floor_fold_ge_member _ (p, w) 0 (by simp [hspent])

theorem inputFloorWealth_insert_other
    (p a v : Nat) (left right : List (Nat × Nat))
    (hmatch : ∃ w, (p, w) ∈ left ++ right)
    (hother : a ≠ p) :
    inputFloorWealth p (left ++ (a, v) :: right) =
      inputFloorWealth p (left ++ right) := by
  have hm : ∃ w, (p, w) ∈ left ++ (a, v) :: right := by
    obtain ⟨w, hw⟩ := hmatch
    refine ⟨w, ?_⟩
    rcases List.mem_append.mp hw with hl | hr
    · exact List.mem_append.mpr (Or.inl hl)
    · exact List.mem_append.mpr (Or.inr (List.mem_cons_of_mem _ hr))
  simp only [inputFloorWealth, matched_of_match p _ hm,
    matched_of_match p _ hmatch]
  simp [List.filter_append, hother]

theorem inputFloorWealth_insert_same_mono
    (p v : Nat) (left right : List (Nat × Nat))
    (hmatch : ∃ w, (p, w) ∈ left ++ right) :
    inputFloorWealth p (left ++ right) ≤
      inputFloorWealth p (left ++ (p, v) :: right) := by
  rw [inputFloorWealth, matched_of_match p _ hmatch]
  apply floor_fold_le _ 0 _ (Nat.zero_le _)
  intro m hm
  have hparts := List.mem_filter.mp hm
  have heq : m.1 = p := by simpa using hparts.2
  have hmem : (p, m.2) ∈ left ++ (p, v) :: right := by
    have hm' : m ∈ left ++ (p, v) :: right := by
      rcases List.mem_append.mp hparts.1 with hl | hr
      · exact List.mem_append.mpr (Or.inl hl)
      · exact List.mem_append.mpr (Or.inr (List.mem_cons_of_mem _ hr))
    simpa only [← heq] using hm'
  exact inputFloorWealth_ge_spent p m.2 _ hmem

-- This is an equality proof about the existing fold, not a replacement model.
private theorem floor_fold_sums (inputs : List (Nat × List (Nat × Nat)))
    (a b : Nat) :
    inputs.foldl (fun (acc : Nat × Nat) i =>
      (acc.1 + i.1 * inputFloorWealth i.1 i.2, acc.2 + i.1)) (a, b) =
      (a + (inputs.map (fun i => i.1 * inputFloorWealth i.1 i.2)).sum,
        b + (inputs.map (fun i => i.1)).sum) := by
  induction inputs generalizing a b with
  | nil => simp
  | cons i inputs ih =>
    simp only [List.foldl_cons]
    rw [ih]
    simp [Nat.add_assoc]

private theorem floor_as_sums (inputs : List (Nat × List (Nat × Nat))) :
    ringFloorWealth inputs =
      if (inputs.map (fun i => i.1)).sum = 0 then 0
      else minN ((inputs.map (fun i => i.1 * inputFloorWealth i.1 i.2)).sum /
        (inputs.map (fun i => i.1)).sum) U128_MAX := by
  unfold ringFloorWealth
  rw [floor_fold_sums]
  simp

private theorem floor_spent_sum_le
    (samples : List (Nat × (List (Nat × Nat) × Nat)))
    (hspent : ∀ s ∈ samples, (s.1, s.2.2) ∈ s.2.1) :
    (samples.map (fun s => s.1 * s.2.2)).sum ≤
      (samples.map (fun s => s.1 * inputFloorWealth s.1 s.2.1)).sum := by
  induction samples with
  | nil => simp
  | cons s samples ih =>
    simp only [List.map_cons, List.sum_cons]
    apply Nat.add_le_add
    · exact Nat.mul_le_mul_left s.1
        (inputFloorWealth_ge_spent s.1 s.2.2 s.2.1 (hspent s (by simp)))
    · apply ih
      intro t ht
      exact hspent t (by simp [ht])

theorem ringFloorWealth_ge_spent_mean
    (samples : List (Nat × (List (Nat × Nat) × Nat)))
    (hspent : ∀ s ∈ samples, (s.1, s.2.2) ∈ s.2.1)
    (hclamp : (samples.map (fun s => s.1 * s.2.2)).sum /
      (samples.map (fun s => s.1)).sum ≤ U128_MAX) :
    (samples.map (fun s => s.1 * s.2.2)).sum /
      (samples.map (fun s => s.1)).sum ≤
      ringFloorWealth (samples.map (fun s => (s.1, s.2.1))) := by
  rw [floor_as_sums]
  simp only [List.map_map, Function.comp_def]
  by_cases hz : (samples.map (fun s => s.1)).sum = 0
  · simp [hz]
  · simp only [hz, ite_false]
    have hmean := Nat.div_le_div_right (floor_spent_sum_le samples hspent)
      (c := (samples.map (fun s => s.1)).sum)
    unfold minN
    split <;> assumption

theorem ringFloorWealth_insert_zero
    (left right : List (Nat × List (Nat × Nat)))
    (members : List (Nat × Nat)) :
    ringFloorWealth (left ++ (0, members) :: right) =
      ringFloorWealth (left ++ right) := by
  simp [floor_as_sums, List.map_append, List.sum_append]

-- Unnamed kernel-checked boundaries: no additional manifest claims.
example : inputFloorWealth 7 [] = 0 ∧ ringFloorWealth [] = 0 := by decide
example : ringFloorWealth [(0, []), (0, [(0, U128_MAX + 1)])] = 0 := by decide
example : inputFloorWealth 5 [(5, 0), (6, 999), (5, 0)] = 0 := by decide
example : inputFloorWealth 5 [(5, 7), (6, 999), (5, 11), (5, 7)] = 11 := by decide

-- Other-amount insertion at the beginning, middle and end.
example : inputFloorWealth 5 ([(8, 1000)] ++ [(5, 7), (5, 11)]) = 11 ∧
    inputFloorWealth 5 ([(5, 7)] ++ [(8, 1000), (5, 11)]) = 11 ∧
    inputFloorWealth 5 ([(5, 7), (5, 11)] ++ [(8, 1000)]) = 11 := by decide
-- Same-amount insertion at the beginning, middle and end.
example : inputFloorWealth 5 ([(5, 13)] ++ [(5, 7), (5, 11)]) = 13 ∧
    inputFloorWealth 5 ([(5, 7)] ++ [(5, 13), (5, 11)]) = 13 ∧
    inputFloorWealth 5 ([(5, 7), (5, 11)] ++ [(5, 13)]) = 13 := by decide

-- Unequal weights and a nonexact division: (2*3 + 3*6)/5 = 24/5 = 4.
example : ringFloorWealth [(2, [(2, 3)]), (3, [(3, 6)])] = 4 := by decide
example : ringFloorWealth [(0, [(0, U128_MAX + 1)]), (2, [(2, 3)]),
    (3, [(3, 6)])] = 4 ∧
    ringFloorWealth [(2, [(2, 3)]), (0, []), (3, [(3, 6)])] = 4 ∧
    ringFloorWealth [(2, [(2, 3)]), (3, [(3, 6)]), (0, [(1, 99)])] = 4 := by decide
-- The clamp premise bounds the spent mean, not each individual wealth.
example : ringFloorWealth [(1, [(1, U128_MAX + 1)]), (1, [(1, 0)])] =
    (U128_MAX + 1) / 2 := by decide
example : ringFloorWealth [(1, [(1, U128_MAX)])] = U128_MAX := by decide
example : inputFloorWealth 18446744073709551616
    [(18446744073709551616, 18446744073709551617)] = 18446744073709551617 ∧
    ringFloorWealth [(18446744073709551616,
      [(18446744073709551616, 18446744073709551617)])] =
      18446744073709551617 := by decide
-- A zero-valued selected spent member still meets the membership premise.
example : 0 ≤ ringFloorWealth [(2, [(2, 0), (2, 7)]), (3, [(3, 0)])] :=
  ringFloorWealth_ge_spent_mean
    [(2, ([(2, 0), (2, 7)], 0)), (3, ([(3, 0)], 0))]
    (by simp) (by decide)

-- Four negative controls delimit the contracts.
-- First match switches away from the fallback maximum and can lower the floor.
example : inputFloorWealth 1 [(2, 100)] = 100 ∧
    inputFloorWealth 1 [(1, 1), (2, 100)] = 1 := by decide
-- Without an existing match, an other-amount insertion changes the fallback.
example : inputFloorWealth 1 [(2, 100)] = 100 ∧
    inputFloorWealth 1 [(2, 100), (3, 200)] = 200 := by decide
-- A spent mean beyond the clamp defeats the unconditional transaction bound.
example : (1 * (U128_MAX + 1)) / 1 = U128_MAX + 1 ∧
    ringFloorWealth [(1, [(1, U128_MAX + 1)])] = U128_MAX ∧
    ¬ (U128_MAX + 1 ≤ ringFloorWealth [(1, [(1, U128_MAX + 1)])]) := by decide
-- Richer same-amount decoys can raise the floor above the spent wealth.
example : (1, 1) ∈ ([(1, 1), (1, 100)] : List (Nat × Nat)) ∧
    inputFloorWealth 1 [(1, 1), (1, 100)] = 100 := by decide

end Botho.Demurrage
