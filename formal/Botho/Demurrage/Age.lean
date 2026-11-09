import Botho.Basic

/-!
Executable age order statistic for `cluster-tax::ring_elapsed_quantile` (#1993).
Values are retained in the input but ignored. Natural subtraction models
saturated ages, and standard list sorting is independent of Rust's sort.
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

end Botho.Demurrage
