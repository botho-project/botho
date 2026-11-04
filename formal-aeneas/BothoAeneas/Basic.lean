import Aeneas

/-!
# Shared lemmas for proofs about the Aeneas extraction

`Result` in the Aeneas library is an interaction tree (`ok`, `fail` as a
visible effect, `div`), and its specs are stated with the weakest-precondition
predicate `spec x P` (notation `x ⦃ r => P r ⦄`). The exported theorems in
this package are stated as plain equations `f args = ok r`, which say directly
that the extracted function returns and does not panic or diverge. The lemma
below converts a spec into that form.
-/

open Aeneas Aeneas.Std Result

namespace BothoAeneas

/-- A satisfied spec means the computation returns `ok` with a value that
meets the postcondition (it neither fails nor diverges). -/
theorem eq_ok_of_spec {α : Type} {x : Result α} {P : α → Prop}
    (h : x ⦃ r => P r ⦄) : ∃ r, x = ok r ∧ P r := by
  cases x with
  | ret r => exact ⟨r, rfl, (WP.spec_ok r).mp h⟩
  | vis i k => exact ((WP.spec_vis i k).mp h).elim
  | div => exact (WP.spec_div.mp h).elim

/-- `saturating_sub` on `u64` (which the Aeneas library defines but states
no lemma for) is truncated natural-number subtraction. -/
theorem u64_saturating_sub_val (x y : U64) :
    (core.num.U64.saturating_sub x y).val = x.val - y.val := by
  have hx : x.bv.toNat < 2 ^ 64 := x.bv.isLt
  simp only [core.num.U64.saturating_sub, UScalar.saturating_sub, UScalar.val,
    BitVec.toNat_ofNat, UScalarTy.numBits]
  rw [Nat.mod_eq_of_lt] <;> omega

/-- `saturating_add` on `u128` is the sum clamped at `u128::MAX`. -/
theorem u128_saturating_add_val (x y : U128) :
    (core.num.U128.saturating_add x y).val = min (x.val + y.val) U128.max := by
  simp only [core.num.U128.saturating_add, UScalar.saturating_add, UScalar.val,
    BitVec.toNat_ofNat, UScalarTy.numBits, UScalar.max, U128.max, U128.numBits]
  rw [Nat.mod_eq_of_lt] <;> omega

end BothoAeneas
