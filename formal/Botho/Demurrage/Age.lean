import Botho.Basic

/-!
Executable elapsed-age helpers: value-independent quantile (#1993) and exact
value-weighted centroid (#1998). Natural subtraction models saturated ages;
standard list sorting is independent of Rust's sort.
Finite DRT correspondence is separate from model proofs; no theorem is added.
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

end Botho.Demurrage
