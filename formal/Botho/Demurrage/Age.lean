import Botho.Basic

/-!
Executable elapsed-age helpers: value-independent quantile (#1993) and exact
value-weighted centroid (#1998). Natural subtraction models saturated ages;
standard list sorting is independent of Rust's sort.
The centroid has four full-Nat model proofs: pair permutation, zero-value
insertion, positive value scaling and the current-height bound. Finite DRT
correspondence with Rust remains separate; the quantile has no property proof here.
-/
namespace Botho.Demurrage

/-- Empty rings yield zero; otherwise use the clamped nearest rank of ages. -/
def ringElapsedQuantile (members : List (Nat × Nat))
    (currentHeight quantileBps : Nat) : Nat :=
  let ages := (members.map fun m => currentHeight - m.2).mergeSort
  let n := ages.length
  let rank := (min quantileBps 10000 * n + 9999) / 10000
  ages.getD (min (rank - 1) (n - 1)) 0

/-- Exact value-weighted saturated age, flooring once; Nat division by zero is zero. -/
def ringElapsedCentroid (members : List (Nat × Nat)) (currentHeight : Nat) : Nat :=
  let weighted := (members.map fun m => m.1 * (currentHeight - m.2)).foldl (· + ·) 0
  let totalValue := (members.map Prod.fst).foldl (· + ·) 0
  weighted / totalValue

private theorem centroid_sum_scale {α : Type} (xs : List α) (f : α → Nat)
    (k : Nat) :
    (xs.map (fun x => k * f x)).sum = k * (xs.map f).sum := by
  induction xs with
  | nil => simp
  | cons x xs ih =>
    simp only [List.map_cons, List.sum_cons, ih, Nat.mul_add]

/-- Reordering complete value/creation-height pairs preserves the centroid. -/
theorem ringElapsedCentroid_perm
    (members reordered : List (Nat × Nat)) (currentHeight : Nat)
    (hp : List.Perm members reordered) :
    ringElapsedCentroid members currentHeight =
      ringElapsedCentroid reordered currentHeight := by
  simp only [ringElapsedCentroid, ← List.sum_eq_foldl_nat]
  rw [(List.Perm.map (fun m => m.1 * (currentHeight - m.2)) hp).sum_nat,
    (List.Perm.map Prod.fst hp).sum_nat]

/-- A zero-value member contributes to neither sum, at any list position. -/
theorem ringElapsedCentroid_insert_zero
    (left right : List (Nat × Nat)) (creationHeight currentHeight : Nat) :
    ringElapsedCentroid (left ++ ((0, creationHeight) :: right)) currentHeight =
      ringElapsedCentroid (left ++ right) currentHeight := by
  simp [ringElapsedCentroid, ← List.sum_eq_foldl_nat, List.sum_append]

/-- A positive common value factor cancels, including when the total is zero. -/
theorem ringElapsedCentroid_scale_values
    (members : List (Nat × Nat)) (currentHeight k : Nat) (hk : 0 < k) :
    ringElapsedCentroid (members.map (fun m => (k * m.1, m.2))) currentHeight =
      ringElapsedCentroid members currentHeight := by
  simp only [ringElapsedCentroid, ← List.sum_eq_foldl_nat, List.map_map,
    Function.comp_def, Nat.mul_assoc, centroid_sum_scale]
  exact Nat.mul_div_mul_left _ _ hk

/-- The weighted sum is bounded even for future creation heights and zero values. -/
private theorem centroid_weighted_le
    (members : List (Nat × Nat)) (currentHeight : Nat) :
    (members.map (fun m => m.1 * (currentHeight - m.2))).sum ≤
      (members.map Prod.fst).sum * currentHeight := by
  induction members with
  | nil => simp
  | cons m members ih =>
    simp only [List.map_cons, List.sum_cons, Nat.add_mul]
    exact Nat.add_le_add (Nat.mul_le_mul_left m.1 (Nat.sub_le _ _)) ih

/-- Natural division returns zero at zero total; all other centroids stay in range. -/
theorem ringElapsedCentroid_le_height
    (members : List (Nat × Nat)) (currentHeight : Nat) :
    ringElapsedCentroid members currentHeight ≤ currentHeight := by
  simp only [ringElapsedCentroid, ← List.sum_eq_foldl_nat]
  exact Nat.div_le_of_le_mul (centroid_weighted_le members currentHeight)

-- Kernel-checked examples supplement the universal proofs; they are not listed claims.
example : ringElapsedCentroid [] 42 = 0 := by decide

example : ringElapsedCentroid [(0, 0), (0, 50)] 42 = 0 := by decide

example : ringElapsedCentroid [(2, 0), (3, 7)] 0 = 0 := by decide

example : ringElapsedCentroid [(2, 11), (3, 20)] 10 = 0 := by decide

-- The zero-value pair is inserted between positive-weight members.
example : ringElapsedCentroid ([(2, 1)] ++ [(0, 99), (3, 7)]) 10 =
    ringElapsedCentroid [(2, 1), (3, 7)] 10 := by decide

example : ringElapsedCentroid [(2, 1), (3, 7), (4, 12)] 10 =
    ringElapsedCentroid [(4, 12), (2, 1), (3, 7)] 10 := by decide

-- Unequal weights yield 27 / 5: scaling preserves the floor of a fractional mean.
example : ringElapsedCentroid ([(2, 1), (3, 7)].map (fun m => (7 * m.1, m.2))) 10 =
    ringElapsedCentroid [(2, 1), (3, 7)] 10 ∧
    ringElapsedCentroid [(2, 1), (3, 7)] 10 = 5 := by decide

-- Both scaled values exceed u64; the model contracts have no machine-width premise.
example : ringElapsedCentroid
    ([(2, 1), (3, 7)].map (fun m => (18446744073709551616 * m.1, m.2))) 10 =
    ringElapsedCentroid [(2, 1), (3, 7)] 10 := by decide


private theorem quantile_sorted (ages : List Nat) :
    ages.mergeSort.Pairwise (· ≤ ·) := by
  have h := List.pairwise_mergeSort (le := fun a b : Nat => a ≤ b)
    (by intros; simp_all; omega) (by intros; simp; omega) ages
  simpa using h

/-- Each value may change independently; only the creation-height sequence matters. -/
theorem ringElapsedQuantile_value_independent
    (members revalued : List (Nat × Nat)) (currentHeight quantileBps : Nat)
    (hheights : members.map Prod.snd = revalued.map Prod.snd) :
    ringElapsedQuantile members currentHeight quantileBps =
      ringElapsedQuantile revalued currentHeight quantileBps := by
  have hages := congrArg (List.map (fun h => currentHeight - h)) hheights
  simp only [List.map_map, Function.comp_def] at hages
  simp only [ringElapsedQuantile, hages]

/-- Sorting removes the ordering of complete value/creation-height pairs. -/
theorem ringElapsedQuantile_perm
    (members reordered : List (Nat × Nat)) (currentHeight quantileBps : Nat)
    (hp : List.Perm members reordered) :
    ringElapsedQuantile members currentHeight quantileBps =
      ringElapsedQuantile reordered currentHeight quantileBps := by
  have hsort : (members.map (fun m => currentHeight - m.2)).mergeSort =
      (reordered.map (fun m => currentHeight - m.2)).mergeSort := by
    apply List.Perm.eq_of_pairwise (le := fun a b : Nat => a ≤ b)
    · intro a b _ _ hab hba
      exact Nat.le_antisymm hab hba
    · exact quantile_sorted _
    · exact quantile_sorted _
    · exact (List.mergeSort_perm _ _).trans
        ((hp.map (fun m => currentHeight - m.2)).trans (List.mergeSort_perm _ _).symm)
  simp only [ringElapsedQuantile, hsort]

/-- Clamping and saturated rank subtraction preserve quantile order. -/
theorem ringElapsedQuantile_mono_quantile
    (members : List (Nat × Nat)) (currentHeight q1 q2 : Nat)
    (hq : q1 ≤ q2) :
    ringElapsedQuantile members currentHeight q1 ≤
      ringElapsedQuantile members currentHeight q2 := by
  let ages := (members.map (fun m => currentHeight - m.2)).mergeSort
  let index := fun q => min (((min q 10000 * ages.length + 9999) / 10000) - 1)
    (ages.length - 1)
  change ages.getD (index q1) 0 ≤ ages.getD (index q2) 0
  by_cases hempty : ages = []
  · simp [hempty]
  · have hn : 0 < ages.length := by
      cases ages <;> simp_all
    have hi1 : index q1 < ages.length := by
      have := Nat.min_le_right (((min q1 10000 * ages.length + 9999) / 10000) - 1)
        (ages.length - 1)
      dsimp [index]
      omega
    have hi2 : index q2 < ages.length := by
      have := Nat.min_le_right (((min q2 10000 * ages.length + 9999) / 10000) - 1)
        (ages.length - 1)
      dsimp [index]
      omega
    have hclamp : min q1 10000 ≤ min q2 10000 := by omega
    have hdiv : (min q1 10000 * ages.length + 9999) / 10000 ≤
        (min q2 10000 * ages.length + 9999) / 10000 :=
      Nat.div_le_div_right (Nat.add_le_add_right
        (Nat.mul_le_mul_right ages.length hclamp) 9999)
    have hi : index q1 ≤ index q2 := by
      dsimp [index]
      omega
    rw [← List.getElem_eq_getD (h := hi1) 0, ← List.getElem_eq_getD (h := hi2) 0]
    by_cases heq : index q1 = index q2
    · simp [heq]
    · exact List.Pairwise.rel_getElem_of_lt hi1 hi2 (quantile_sorted _)
        (by omega)

/-- Every saturated age, and the empty-list fallback, is at most the current height. -/
theorem ringElapsedQuantile_le_height
    (members : List (Nat × Nat)) (currentHeight quantileBps : Nat) :
    ringElapsedQuantile members currentHeight quantileBps ≤ currentHeight := by
  let ages := (members.map (fun m => currentHeight - m.2)).mergeSort
  let i := min (((min quantileBps 10000 * ages.length + 9999) / 10000) - 1)
    (ages.length - 1)
  change ages.getD i 0 ≤ currentHeight
  by_cases hi : i < ages.length
  · rw [← List.getElem_eq_getD (h := hi) 0]
    have hm : ages[i] ∈ members.map (fun m => currentHeight - m.2) :=
      List.mem_mergeSort.mp (List.getElem_mem hi)
    obtain ⟨m, _, he⟩ := List.mem_map.mp hm
    rw [← he]
    exact Nat.sub_le _ _
  · simp [List.getD, hi]

end Botho.Demurrage
