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

-- Botho.Supply.Accounting (#1839; scalar model, explicit ledger premises)
#print axioms Botho.Supply.fee_quotient_le
#print axioms Botho.Supply.splitFees_exact
#print axioms Botho.Supply.splitFees_total
#print axioms Botho.Supply.splitFees_conserves
#print axioms Botho.Supply.splitFees_rounding
#print axioms Botho.Supply.oversized_fraction_creates_value
#print axioms Botho.Supply.pool_inflow_fits
#print axioms Botho.Supply.apply_conserves
#print axioms Botho.Supply.counters_fit
#print axioms Botho.Supply.issuance_step_fits
#print axioms Botho.Supply.arbitrary_pool_saturates
