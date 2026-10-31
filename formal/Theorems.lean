/-
Manifest of exported theorems (#1705).

Every theorem the project claims as "proven" must be listed here with a
`#print axioms` line. `scripts/check-axioms.sh` compiles this file and fails
if a listed theorem depends on an axiom outside `axioms-allowlist.txt`
(including `sorryAx`), if the number of results does not match the number of
`#print axioms` lines, or if the manifest is empty.

This file is not part of the `Botho` library. The gate compiles it separately
with `lake env lean Theorems.lean` after `lake build`.
-/
import Botho

-- Botho.Basic (scaffold toy; status: proven)
#print axioms Botho.transfer_conserves
#print axioms Botho.transfer_isSome_iff
#print axioms Botho.satAdd_le_cap
#print axioms Botho.satAdd_eq_add
