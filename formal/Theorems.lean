/-
Manifest of exported theorems (#1705).

Every theorem the project claims as "proven" must be listed here with a
`#print axioms` line. `scripts/check-axioms.sh` fails if a listed theorem
depends on an axiom outside `axioms-allowlist.txt` (including `sorryAx`), if
the manifest is empty, or if this file contains anything other than comments,
`import` lines and one `#print axioms NAME` per line (#1770). Prove theorems
in the `Botho` library, not here.

This file is not part of the `Botho` library. The gate reads it before
`lake build`, and compiles that copy separately with `lake env lean --json`
after the build. It checks each result against an independent walk of the
compiled library (`scripts/AxiomReport.lean`).
-/
import Botho

-- Botho.Basic (scaffold toy; status: proven)
#print axioms Botho.transfer_conserves
#print axioms Botho.transfer_isSome_iff
#print axioms Botho.satAdd_le_cap
#print axioms Botho.satAdd_eq_add

-- Botho.Demurrage.Counterexamples (#1711; current arithmetic, not a fix)
#print axioms Botho.Demurrage.six_block_dead_zone
#print axioms Botho.Demurrage.churn_counterexample
#print axioms Botho.Demurrage.charge_exceeds_value
#print axioms Botho.Demurrage.homogeneous_ring_scale_counterexample
#print axioms Botho.Demurrage.homogeneous_centroid_counterexample
#print axioms Botho.Demurrage.saturation_lowers_charge
