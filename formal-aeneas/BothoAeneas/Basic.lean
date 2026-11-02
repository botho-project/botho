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

end BothoAeneas
