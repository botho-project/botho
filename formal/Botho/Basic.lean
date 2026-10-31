/-
Trivial scaffold model (#1705).

This is a toy, not a model of any Rust code. It exists so that the build, the
sorry gate and the axiom gate have something real to check. Status: proven
(it mirrors no Rust, so there is nothing to test differentially).

The toy is a two-account transfer that must conserve the total balance, plus a
saturating add that never exceeds its cap. Real models of the ledger and the
lottery will need the same two shapes: conservation and bounds.
-/
namespace Botho

/-- Move `amt` from balance `src` to balance `dst`. Returns `none` when `src`
cannot cover `amt`, so it never underflows. -/
def transfer (src dst amt : Nat) : Option (Nat × Nat) :=
  if amt ≤ src then some (src - amt, dst + amt) else none

/-- Addition that clamps at `cap`. -/
def satAdd (cap a b : Nat) : Nat :=
  min (a + b) cap

/-- A successful transfer conserves the total balance. -/
theorem transfer_conserves {src dst amt src' dst' : Nat}
    (h : transfer src dst amt = some (src', dst')) :
    src' + dst' = src + dst := by
  unfold transfer at h
  split at h
  · simp only [Option.some.injEq, Prod.mk.injEq] at h
    omega
  · contradiction

/-- A transfer succeeds exactly when the source balance covers the amount. -/
theorem transfer_isSome_iff (src dst amt : Nat) :
    (transfer src dst amt).isSome ↔ amt ≤ src := by
  unfold transfer
  split <;> simp_all

/-- Saturating addition never exceeds its cap. -/
theorem satAdd_le_cap (cap a b : Nat) : satAdd cap a b ≤ cap := by
  unfold satAdd
  omega

/-- Below the cap, saturating addition is ordinary addition. -/
theorem satAdd_eq_add {cap a b : Nat} (h : a + b ≤ cap) :
    satAdd cap a b = a + b := by
  unfold satAdd
  omega

end Botho

-- CI TEST (#1705): deliberate sorry; reverted in the next commit.
theorem Botho.ci_test_sorry : 1 = 2 := by
  sorry
