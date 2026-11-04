/-
Exact ring centroid (#1876, protocol 9.0.0). Models the fixed
`demurrage.rs::ring_centroid_implied_factor`: the value-weighted centroid is
the exact rational floor `Σ value·wealth / Σ value`, computed in Rust with a
256-bit numerator (`cluster-tax/src/wide.rs`) and clamped only at the end.

The pre-fix (8.x) saturating model, `centroid` / `ringFactor` in
`Counterexamples.lean`, is kept unchanged with its theorems as the historical
witnesses of the bug. The DRT leg `ring_centroid_factor` now compares the Rust
function against `ringFactorExact`. Status: proven (model).
-/
import Botho.Demurrage.Counterexamples

namespace Botho.Demurrage
open Botho.Lottery

/-- Exact accumulators of the protocol 9.0.0
`demurrage.rs::ring_centroid_implied_factor`, before the curve. No
intermediate saturates (the Rust numerator is 256 bits and its denominator is
a sum of u64 values, neither of which can overflow for an in-memory ring);
the result is clamped at `u128::MAX` once, at the end. -/
def exactCentroid (members : List (Nat × Nat)) : Nat :=
  let (weighted, total) := members.foldl (fun (a : Nat × Nat) m =>
    (a.1 + m.1 * m.2, a.2 + m.1)) (0, 0)
  if total = 0 then 0 else minN (weighted / total) U128_MAX

/-- Protocol 9.0.0 production helper with the default curve. -/
def ringFactorExact (members : List (Nat × Nat)) : Nat :=
  defaultFactor (exactCentroid members)

/-- The exact accumulators over a homogeneous ring. -/
theorem foldl_replicate_exact (n v w a b : Nat) :
    (List.replicate n (v, w)).foldl (fun (acc : Nat × Nat) m =>
      (acc.1 + m.1 * m.2, acc.2 + m.1)) (a, b) = (a + n * (v * w), b + n * v) := by
  induction n generalizing a b with
  | zero => simp
  | succ k ih =>
    rw [List.replicate_succ, List.foldl_cons]
    simp only
    rw [ih]
    simp only [Prod.mk.injEq]
    constructor <;> rw [Nat.succ_mul] <;> omega

/-- Scale invariance (the #1876 fix): a homogeneous ring's exact centroid is
its common wealth, for every member count and every positive value. -/
theorem exactCentroid_homogeneous (n v w : Nat) (hn : 0 < n) (hv : 0 < v)
    (hw : w ≤ U128_MAX) :
    exactCentroid (List.replicate n (v, w)) = w := by
  unfold exactCentroid
  rw [foldl_replicate_exact]
  simp only [Nat.zero_add]
  have hnv : 0 < n * v := Nat.mul_pos hn hv
  have hne : n * v ≠ 0 := Nat.pos_iff_ne_zero.mp hnv
  have hq : n * (v * w) / (n * v) = w := by
    rw [← Nat.mul_assoc]
    exact Nat.mul_div_cancel_left w hnv
  simp only [hne, ite_false, hq, minN, hw, ite_true]

/-- The two `homogeneous_ring_scale_counterexample` rings under the fix: both
floors are the common wealth's 5949. -/
theorem homogeneous_ring_scale_exact :
    ringFactorExact (List.replicate 20 (100000000000000000, 100000000000000000000)) = 5949 ∧
    ringFactorExact (List.replicate 20 (1000000000000000000, 100000000000000000000)) = 5949 := by
  decide

/-- The `homogeneous_centroid_counterexample` ring under the fix: the exact
centroid is the common wealth `10^20`, not the pre-fix
`17014118346046923173`. -/
theorem homogeneous_centroid_exact :
    exactCentroid (List.replicate 20 (1000000000000000000, 100000000000000000000)) =
      100000000000000000000 := by decide

/-- The pre-fix and fixed models agree whenever the pre-fix numerator does not
saturate, so the fix only changes rings whose `Σ value·wealth` exceeds
`u128::MAX`: here, the largest homogeneous 20-member ring at `W = 10^20` the
8.x code handled exactly, and the next value up, where they diverge. -/
theorem exact_versus_saturating_boundary :
    centroid (List.replicate 20 (170141183460469231, 100000000000000000000)) =
      exactCentroid (List.replicate 20 (170141183460469231, 100000000000000000000)) ∧
    centroid (List.replicate 20 (170141183460469232, 100000000000000000000)) <
      exactCentroid (List.replicate 20 (170141183460469232, 100000000000000000000)) := by
  decide

end Botho.Demurrage
