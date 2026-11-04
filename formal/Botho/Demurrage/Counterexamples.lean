/-
Counterexample search for #1711. Models the current demurrage kernel and
default ring factor; no proposed consensus change. Status: proven (model).
Rust correspondence is tested separately by the DRT legs.

#1876 (protocol 9.0.0): `centroid` / `ringFactor` below model the PRE-FIX
(8.x) saturating ring centroid and are kept unchanged, with their theorems, as
historical witnesses of the bug. The fixed exact centroid is `exactCentroid` /
`ringFactorExact` in `Exact.lean`, which the `ring_centroid_factor` DRT leg now
compares against the Rust function.
-/
import Botho.Lottery.Arith

namespace Botho.Demurrage
open Botho.Lottery

/-- `cluster-tax/src/demurrage.rs::demurrage_charge`. Inputs have Rust widths
u64/u64/u64/u32/u64. The first product fits u128 (< 2^109); the time-scaled
product saturates before division. The return conversion clamps to u64. -/
def charge (value factor elapsed rate year : Nat) : Nat :=
  if rate = 0 ∨ elapsed = 0 ∨ year = 0 then 0 else
  let progressivity := clampN 1000 6000 factor - 1000
  if progressivity = 0 then 0 else
  let time := elapsed * 1000000 / year
  let annual := value * rate * progressivity / 10000 / 5000
  minN (satMul U128_MAX annual time / 1000000) U64_MAX

/-- Production timing: the time quantization zeros all charges through six
blocks, independently of value, factor and rate. No value threshold repairs it. -/
theorem six_block_dead_zone (value factor rate elapsed : Nat) (ht : elapsed ≤ 6) :
    charge value factor elapsed rate 6307200 = 0 := by
  have htime : elapsed * 1000000 / 6307200 = 0 := by omega
  simp [charge, htime, satMul, minN, U128_MAX]

/-- The documented same-class churn identity fails even with fixed principal:
two six-block intervals charge zero, one twelve-block interval charges 0.02 BTH.
This is a kernel counterexample, not a constructed accepted-block attack. -/
theorem churn_counterexample :
    charge 1000000000000000000 6000 6 200 6307200 = 0 ∧
    charge 1000000000000000000 6000 12 200 6307200 = 20000000000 := by decide

/-- The #1711 unconditional charge ≤ value target is also false at 51 years
with the production 2% annual rate. Transaction balance is a separate gate. -/
theorem charge_exceeds_value :
    charge 1000000 6000 (51 * 6307200) 200 6307200 = 1020000 := by decide

/-- Saturating weighted and value accumulators of
`demurrage.rs::ring_centroid_implied_factor`, before the curve. -/
def centroid (members : List (Nat × Nat)) : Nat :=
  let (weighted, total) := members.foldl (fun (a : Nat × Nat) m =>
    (satAdd U128_MAX a.1 (satMul U128_MAX m.1 m.2), satAdd U128_MAX a.2 m.1)) (0, 0)
  if total = 0 then 0 else weighted / total

/-- `fee_curve.rs::log2_fp` for positive u128 wealth. Nat.log2 is the index
of the most significant bit; the remainder models the 16-bit mantissa mask. -/
def log2Fp (w : Nat) : Nat :=
  let msb := w.log2
  let frac := if 16 ≤ msb then (w / 2 ^ (msb - 16)) % 65536
              else (w * 2 ^ (16 - msb)) % 65536
  msb * 65536 + frac

def knots : List (Int × Nat) :=
  [(-6000, 131), (-4000, 1180), (-2000, 7798), (0, 32768),
   (2000, 57738), (4000, 64356), (6000, 65405)]

/-- Interior loop of `lut_sigmoid`; the knots are ordered and increasing. -/
def interpolate (x : Int) : List (Int × Nat) → Nat
  | (x0, y0) :: (x1, y1) :: rest =>
    if x0 ≤ x ∧ x < x1 then
      y0 + (y1 - y0) * (x - x0).toNat / (x1 - x0).toNat
    else interpolate x ((x1, y1) :: rest)
  | _ => 32768

/-- `fee_curve.rs::ClusterFactorCurve::factor` with default parameters only.
Int.tdiv preserves Rust's signed division toward zero below the midpoint. -/
def defaultFactor (wealth : Nat) : Nat :=
  if wealth = 0 then 1000 else
  let z := (log2Fp wealth : Int) - 3695429
  let x := Int.tdiv (z * 1000) 131072
  let sig := if x ≤ -6000 then 0 else if 6000 ≤ x then 65536 else interpolate x knots
  1000 + 5000 * sig / 65536

/-- Current production helper with the default curve. -/
def ringFactor (members : List (Nat × Nat)) : Nat := defaultFactor (centroid members)

/-- Every member has exactly the same wealth, yet scaling every value by ten
lowers the floor. Both are 20-member rings, with u64 values and u128 wealth. -/
theorem homogeneous_ring_scale_counterexample :
    ringFactor (List.replicate 20 (100000000000000000, 100000000000000000000)) = 5949 ∧
    ringFactor (List.replicate 20 (1000000000000000000, 100000000000000000000)) = 5841 := by
  decide

/-- The saturated numerator divided by an unsaturated denominator undershoots
the common wealth; it does not pin the centroid to u128::MAX as documented. -/
theorem homogeneous_centroid_counterexample :
    centroid (List.replicate 20 (1000000000000000000, 100000000000000000000)) =
      17014118346046923173 := by decide

/-- At one policy year, that lower floor lowers the accrued fee on the same
1,000,000-BTH transfer (integer outputs are picocredits). -/
theorem saturation_lowers_charge :
    charge 1000000000000000000 5841 6307200 200 6307200 <
      charge 1000000000000000000 5949 6307200 200 6307200 := by decide

end Botho.Demurrage
