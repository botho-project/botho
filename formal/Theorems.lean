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

-- Botho.Demurrage.Counterexamples (#1711; current arithmetic, not a fix)
#print axioms Botho.Demurrage.six_block_dead_zone
#print axioms Botho.Demurrage.churn_counterexample
#print axioms Botho.Demurrage.charge_exceeds_value
#print axioms Botho.Demurrage.homogeneous_ring_scale_counterexample
#print axioms Botho.Demurrage.homogeneous_centroid_counterexample
#print axioms Botho.Demurrage.saturation_lowers_charge

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

-- Botho.Quorum.Fbas (#1714: flat FBAS quorum intersection, despite-f, curated
-- rotation; status: proven (model); Rust ties are tested, see the module docstring)
#print axioms Botho.Quorum.delete_satisfies_iff
#print axioms Botho.Quorum.sym_intersects_iff
#print axioms Botho.Quorum.sym_intersects_iff_pos
#print axioms Botho.Quorum.analytic_rule_eq
#print axioms Botho.Quorum.seed_fallback_intersects
#print axioms Botho.Quorum.sym_despite_iff
#print axioms Botho.Quorum.sym_available_iff
#print axioms Botho.Quorum.byzTol_iff_dset
#print axioms Botho.Quorum.byzTol_conservative_edge
#print axioms Botho.Quorum.byzTol_pos_iff
#print axioms Botho.Quorum.recommended_gate_never_refuses
#print axioms Botho.Quorum.recommended_crash_flag_overclaims
#print axioms Botho.Quorum.recommended_crash_flag_even_sound
#print axioms Botho.Quorum.recommended_bft_byzTol
#print axioms Botho.Quorum.checkDespite_sound
#print axioms Botho.Quorum.checker_sound
#print axioms Botho.Quorum.checker_complete_sym
#print axioms Botho.Quorum.checkDespite_sym_iff
#print axioms Botho.Quorum.bruteIntersects_iff
#print axioms Botho.Quorum.checker_not_complete
#print axioms Botho.Quorum.rotation_sound
#print axioms Botho.Quorum.rotation_condition_necessary
#print axioms Botho.Quorum.nested_rotation_safe
#print axioms Botho.Quorum.recommended_crash_bowtie_fork
#print axioms Botho.Quorum.recommended_bft_bowtie_fork
#print axioms Botho.Quorum.prod_4of5_intersects
#print axioms Botho.Quorum.prod_4of5_byzTol
#print axioms Botho.Quorum.prod_4of5_dset_one
#print axioms Botho.Quorum.prod_4of5_two_faults
#print axioms Botho.Quorum.prod_4of5_rotate_one_despite_one
#print axioms Botho.Quorum.prod_4of5_rotate_two_intersects
#print axioms Botho.Quorum.prod_4of5_rotate_two_not_despite_one
#print axioms Botho.Quorum.reroster_mixed_fork
-- Lottery seed independence and canonical ordering (#1850; proven (model)).
-- LMDB ordering and the Rust tie remain trusted/tested; see lottery.md.
#print axioms Botho.Lottery.seed_body_independent
#print axioms Botho.Lottery.seed_reads_tip_only
#print axioms Botho.Lottery.seed_factors
#print axioms Botho.Lottery.seed_eq_of_pow_fields
#print axioms Botho.Lottery.candidates_of_postState
#print axioms Botho.Lottery.drawLotteryWinnersSeeded_ids
#print axioms Botho.Lottery.winners_ids_independent_of_amounts
#print axioms Botho.Lottery.bytesLt_irrefl
#print axioms Botho.Lottery.bytesLt_trans
#print axioms Botho.Lottery.bytesLt_trichotomy
#print axioms Botho.Lottery.sortById_sorted
#print axioms Botho.Lottery.sortById_of_sorted
#print axioms Botho.Lottery.sortById_perm
#print axioms Botho.Lottery.drawing_perm_invariant
#print axioms Botho.Lottery.draw_order_dependent
#print axioms Botho.Lottery.scan_cap_order_dependent

-- Botho.Lottery.Sybil (#1708 part C1; status: proven (model), the lottery
-- model is tested by the #1818 DRT legs; ST-3 is about a stated economy
-- model, not the node)
#print axioms Botho.Lottery.TICKET_LIFE_eq
#print axioms Botho.Lottery.drawEligible_uniform_all_win
#print axioms Botho.Lottery.rotate_perm
#print axioms Botho.Lottery.ce_owner_wins_three_base_fees
#print axioms Botho.Lottery.sybil_net_zero_refuted
#print axioms Botho.Lottery.rewardCap_inert
#print axioms Botho.Lottery.lotteryCandidates_length
#print axioms Botho.Lottery.countEligible_lotteryCandidates
#print axioms Botho.Lottery.rewardCap_saturated
#print axioms Botho.Lottery.splitUnprofitable_iff
#print axioms Botho.Lottery.split_never_pays_iff
#print axioms Botho.Lottery.split_never_pays_iff_fixed
#print axioms Botho.Lottery.break_even_reference
#print axioms Botho.Lottery.break_even_backlog_reference
#print axioms Botho.Lottery.split_pays_at_reference
#print axioms Botho.Lottery.r3_no_split_profit
#print axioms Botho.Lottery.r3_eligibility_unpriced
#print axioms Botho.Lottery.r3_pricing_violated
