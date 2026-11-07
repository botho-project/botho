/-
The protocol 9.0.0 demurrage kernel (#1875): one floor of the full rational
charge. Status: proven (model), with the six claims below listed and
statement-bound; separately tested (DRT leg `demurrage_charge`).

`Botho.Demurrage.charge` in `Counterexamples.lean` is the LEGACY pre-9.0.0
kernel (time fraction floored to 1e6 units of a year before the value is
applied). Its theorems (`six_block_dead_zone`, `churn_counterexample`,
`charge_exceeds_value`, `saturation_lowers_charge`) are unchanged: they are
the machine-checked record of the #1875 finding against that kernel.
-/
import Botho.Lottery.Arith

namespace Botho.Demurrage
open Botho.Lottery

/-- `cluster-tax/src/demurrage.rs::demurrage_charge` from protocol 9.0.0:

    floor(value × rate × (clamp(factor, 1000, 6000) − 1000) × elapsed
          / (10_000 × 5_000 × year))

clamped to `u64::MAX`. This is the specification itself, on `Nat`, with no
intermediate rounding: the Rust kernel's u128 decomposition (quotient and
remainder of `value × rate × p` by the denominator, with `elapsed` split at
2^32) is not mirrored, so the DRT tests that the decomposition is exact.
The Rust zero guards (`rate = 0`, `elapsed = 0`, `year = 0`, progressivity
0) all agree with this formula: a zero factor in the numerator gives 0, and
`Nat` division by zero is 0. -/
def exactCharge (value factor elapsed rate year : Nat) : Nat :=
  let progressivity := clampN 1000 6000 factor - 1000
  minN (value * rate * progressivity * elapsed / (10000 * 5000 * year)) U64_MAX

private theorem ceil_le_iff (d k v : Nat) (hk : 0 < k) :
    (d + k - 1) / k ≤ v ↔ d ≤ v * k := by
  have h := Nat.div_lt_iff_lt_mul (x := d + k - 1) (y := v + 1) hk
  rw [Nat.add_mul, Nat.one_mul] at h
  omega

private theorem minN_positive_iff (a m : Nat) (hm : 0 < m) :
    0 < minN a m ↔ 0 < a := by
  unfold minN
  split <;> omega

/-- The first positive charge is exactly the ceiling of denominator / rate
product. The threshold is a Nat and need not fit a typed u64 input. -/
theorem exactCharge_positive_iff (value factor elapsed rate year : Nat)
    (hy : 0 < year) (hr : 0 < rate) (he : 0 < elapsed) (hf : 1000 < factor) :
    0 < exactCharge value factor elapsed rate year ↔
      (10000 * 5000 * year + rate * (clampN 1000 6000 factor - 1000) * elapsed - 1) /
        (rate * (clampN 1000 6000 factor - 1000) * elapsed) ≤ value := by
  have hp : 0 < clampN 1000 6000 factor - 1000 := by
    unfold clampN
    split <;> (try split) <;> omega
  have hk := Nat.mul_pos (Nat.mul_pos hr hp) he
  have hd : 0 < 10000 * 5000 * year := Nat.mul_pos (by decide) hy
  unfold exactCharge
  rw [minN_positive_iff _ _ (by decide), Nat.div_pos_iff]
  rw [ceil_le_iff _ _ _ hk]
  simp only [Nat.mul_assoc] at *
  exact and_iff_right hd

private theorem floor_add_le (a b d : Nat) (hd : 0 < d) :
    (a + b) / d ≤ a / d + b / d + 1 := by
  rw [Nat.add_div hd]
  split <;> omega

private theorem capped_floor_add_le (a b d m : Nat) (hd : 0 < d) :
    minN ((a + b) / d) m ≤ minN (a / d) m + minN (b / d) m + 1 := by
  have h := floor_add_le a b d hd
  have ha := Nat.zero_le (a / d)
  have hb := Nat.zero_le (b / d)
  unfold minN
  split <;> split <;> split <;> omega

private theorem capped_split_loss (xs : List Nat) (k d m : Nat)
    (hne : xs ≠ []) (hd : 0 < d) :
    minN (xs.sum * k / d) m <
      (xs.map (fun v => minN (v * k / d) m)).sum + xs.length := by
  induction xs with
  | nil => exact False.elim (hne rfl)
  | cons x xs ih =>
    cases xs with
    | nil => simp
    | cons y ys =>
      have ht := ih (by simp)
      have h := capped_floor_add_le (x * k) ((y :: ys).sum * k) d m hd
      simp only [List.sum_cons, List.map_cons, List.length_cons, Nat.add_mul] at *
      omega

private theorem capped_split_le_raw (xs : List Nat) (k d m : Nat) :
    (xs.map (fun v => minN (v * k / d) m)).sum ≤ xs.sum * k / d := by
  induction xs with
  | nil => simp
  | cons x xs ih =>
    have hx : minN (x * k / d) m ≤ x * k / d := by
      unfold minN
      split <;> omega
    have h := Nat.div_add_div_le_add_div (x := x * k) (y := xs.sum * k) (z := d)
    simp only [List.sum_cons, List.map_cons, Nat.add_mul]
    omega

/-- Splitting loses fewer than one pico per piece, even if one or more
charges hit the u64 cap. Zero-valued pieces are allowed; the list is nonempty. -/
theorem exactCharge_split_loss_bound (xs : List Nat) (factor elapsed rate year : Nat)
    (hne : xs ≠ []) (hy : 0 < year) :
    exactCharge xs.sum factor elapsed rate year <
      (xs.map (fun v => exactCharge v factor elapsed rate year)).sum + xs.length := by
  have hd : 0 < 10000 * 5000 * year := Nat.mul_pos (by decide) hy
  simpa only [exactCharge, Nat.mul_assoc] using
    capped_split_loss xs (rate * (clampN 1000 6000 factor - 1000) * elapsed)
      (10000 * 5000 * year) U64_MAX hne hd

/-- When the aggregate raw quotient is within u64, splitting cannot increase
the charge and loses at most n - 1 pico across n pieces. -/
theorem exactCharge_split_rounding (xs : List Nat) (factor elapsed rate year : Nat)
    (hne : xs ≠ []) (hy : 0 < year)
    (hcap : xs.sum * (rate * (clampN 1000 6000 factor - 1000) * elapsed) /
      (10000 * 5000 * year) ≤ U64_MAX) :
    (xs.map (fun v => exactCharge v factor elapsed rate year)).sum ≤
        exactCharge xs.sum factor elapsed rate year ∧
      exactCharge xs.sum factor elapsed rate year <
        (xs.map (fun v => exactCharge v factor elapsed rate year)).sum + xs.length := by
  constructor
  · have h := capped_split_le_raw xs
      (rate * (clampN 1000 6000 factor - 1000) * elapsed)
      (10000 * 5000 * year) U64_MAX
    have hc : xs.sum * rate * (clampN 1000 6000 factor - 1000) * elapsed /
        (10000 * 5000 * year) ≤ U64_MAX := by
      simpa only [Nat.mul_assoc] using hcap
    have heq : exactCharge xs.sum factor elapsed rate year =
        xs.sum * (rate * (clampN 1000 6000 factor - 1000) * elapsed) /
          (10000 * 5000 * year) := by
      unfold exactCharge minN
      rw [ite_eq_left hc]
      simp only [Nat.mul_assoc]
    rw [heq]
    simpa only [exactCharge, Nat.mul_assoc] using h
  · exact exactCharge_split_loss_bound xs factor elapsed rate year hne hy

/-- A typed synthetic helper witness: the cap makes the split charge exceed
the whole charge. This does not assert a reachable ledger state. -/
theorem exactCharge_split_clipping_counterexample :
    let xs := [9223372036854775807, 9223372036854775808]
    (∀ v ∈ xs, v ≤ U64_MAX) ∧ xs.sum = U64_MAX ∧
      exactCharge xs.sum 6000 2 10000 1 = U64_MAX ∧
      (xs.map (fun v => exactCharge v 6000 2 10000 1)).sum = 36893488147419103229 ∧
      exactCharge xs.sum 6000 2 10000 1 <
        (xs.map (fun v => exactCharge v 6000 2 10000 1)).sum := by decide

-- The sharp one-block threshold, including its immediately preceding value.
example : (10000 * 5000 * 6307200 + 200 * 5000 - 1) / (200 * 5000) =
    315360000 := by decide
example : exactCharge 315359999 6000 1 200 6307200 = 0 := by decide
example : exactCharge 315360000 6000 1 200 6307200 = 1 := by decide

-- Zero guards remain outside the positive theorem's hypotheses.
example (v f e r y : Nat) (hf : f ≤ 1000) : exactCharge v f e r y = 0 := by
  have hp : clampN 1000 6000 f - 1000 = 0 := by
    unfold clampN
    split <;> (try split) <;> omega
  simp [exactCharge, hp, minN]
example (v f r y : Nat) : exactCharge v f 0 r y = 0 := by simp [exactCharge, minN]
example (v f e y : Nat) : exactCharge v f e 0 y = 0 := by simp [exactCharge, minN]
example (v f e r : Nat) : exactCharge v f e r 0 = 0 := by simp [exactCharge, minN]

-- Two individually sub-threshold pieces lose one pico without clipping.
example : exactCharge ([315359999, 1].sum) 6000 1 200 6307200 = 1 ∧
    ([315359999, 1].map (fun v => exactCharge v 6000 1 200 6307200)).sum = 0 := by
  decide

private theorem minN_mono_left (a b cap : Nat) (h : a ≤ b) :
    minN a cap ≤ minN b cap := by
  unfold minN
  split <;> split <;> omega

/-- Increasing elapsed time cannot decrease the total Nat charge, including
zero-year division and the final u64 cap plateau. -/
theorem exactCharge_mono_elapsed
    (value factor e1 e2 rate year : Nat) (h : e1 ≤ e2) :
    exactCharge value factor e1 rate year ≤
      exactCharge value factor e2 rate year := by
  have hn := Nat.mul_le_mul_left
    (value * rate * (clampN 1000 6000 factor - 1000)) h
  have hd := Nat.div_le_div_right hn (c := 10000 * 5000 * year)
  dsimp only [exactCharge]
  exact minN_mono_left _ _ _ hd

/-- Increasing the input factor cannot decrease the total Nat charge, including
both clamp regions, zero-year division and the final u64 cap plateau. -/
theorem exactCharge_mono_factor
    (value f1 f2 elapsed rate year : Nat) (h : f1 ≤ f2) :
    exactCharge value f1 elapsed rate year ≤
      exactCharge value f2 elapsed rate year := by
  have hp : clampN 1000 6000 f1 - 1000 ≤ clampN 1000 6000 f2 - 1000 := by
    unfold clampN
    split <;> (try split) <;> (try split) <;> (try split) <;> omega
  have hn := Nat.mul_le_mul_right elapsed (Nat.mul_le_mul_left (value * rate) hp)
  have hd := Nat.div_le_div_right hn (c := 10000 * 5000 * year)
  dsimp only [exactCharge]
  exact minN_mono_left _ _ _ hd

-- Kernel-checked boundary examples supplement the full-Nat proofs.
example : exactCharge 1000000 6000 1 200 0 = 0 ∧
    exactCharge 1000000 6000 2 200 0 = 0 := by decide

-- Below, at and immediately above the lower factor clamp.
example : exactCharge 5000 999 1 10000 1 = 0 ∧
    exactCharge 5000 1000 1 10000 1 = 0 ∧
    exactCharge 5000 1001 1 10000 1 = 1 := by decide

-- Below, at and immediately above the upper factor clamp.
example : exactCharge 5000 5999 1 10000 1 = 4999 ∧
    exactCharge 5000 6000 1 10000 1 = 5000 ∧
    exactCharge 5000 6001 1 10000 1 = 5000 := by decide

-- Increasing elapsed time on the final cap plateau preserves the charge.
example : exactCharge U64_MAX 6000 1 10000 1 = U64_MAX ∧
    exactCharge U64_MAX 6000 2 10000 1 = U64_MAX := by decide

end Botho.Demurrage
