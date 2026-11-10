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

end Botho.Demurrage
