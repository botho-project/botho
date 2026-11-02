namespace Botho
-- The record binds this statement over `Nat`; the forged build states it
-- over `Fin 2`. Without the implicit type arguments both read
-- `Eq (instHAdd.hAdd 0 0) 0` (#1824).
theorem implicit_zero : (0 : Nat) + 0 = 0 := rfl
end Botho
