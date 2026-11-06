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

-- Protocol 9.0.0 exact semantics (#1916; proven model, separately DRT-tested).
#print axioms Botho.Demurrage.foldl_replicate_exact
#print axioms Botho.Demurrage.exactCentroid_homogeneous
#print axioms Botho.Demurrage.homogeneous_ring_scale_exact
#print axioms Botho.Demurrage.homogeneous_centroid_exact
#print axioms Botho.Demurrage.exact_versus_saturating_boundary
#print axioms Botho.Tags.whale_claimed_wealth_exact
#print axioms Botho.Demurrage.exactCharge_positive_iff
#print axioms Botho.Demurrage.exactCharge_split_loss_bound
#print axioms Botho.Demurrage.exactCharge_split_rounding
#print axioms Botho.Demurrage.exactCharge_split_clipping_counterexample

-- Botho.Basic (scaffold toy; status: proven)
#print axioms Botho.transfer_conserves
#print axioms Botho.transfer_isSome_iff
#print axioms Botho.satAdd_le_cap
#print axioms Botho.satAdd_eq_add

-- Botho.Lottery.SybilExpectation (#1851; proven ideal model/counting).
-- The SHA bridge explicitly assumes IID uniform u128 words; see lottery.md.
#print axioms Botho.Lottery.pickWinners_eq_pickWinnersF
#print axioms Botho.Lottery.Ideal.payoutMass_eq
#print axioms Botho.Lottery.Ideal.expected_share
#print axioms Botho.Lottery.Ideal.paths_length
#print axioms Botho.Lottery.Ideal.payoutMass_paths
#print axioms Botho.Lottery.Ideal.selectAt_uniform_index
#print axioms Botho.Lottery.Ideal.uniform_path_loop
#print axioms Botho.Lottery.Ideal.draw_path
#print axioms Botho.Lottery.Ideal.exact_share
#print axioms Botho.Lottery.Ideal.base_fee_bound
#print axioms Botho.Lottery.drawEligible_eq_drawEligibleF
#print axioms Botho.Lottery.drawEligibleF_uniform
#print axioms Botho.Lottery.Modulo.residue_count
#print axioms Botho.Lottery.Modulo.residue_total
#print axioms Botho.Lottery.Modulo.pathWeight_total
#print axioms Botho.Lottery.Modulo.expectation_slack
#print axioms Botho.Lottery.Modulo.u128_iid_bridge
#print axioms Botho.Lottery.lotteryCandidates_complete
#print axioms Botho.Lottery.Ideal.observation_share
#print axioms Botho.Lottery.Ideal.r3_from_model
#print axioms Botho.Lottery.Ideal.splitUnprofitable_model_iff
#print axioms Botho.Lottery.Ideal.marginal_common_budget
#print axioms Botho.Lottery.Ideal.marginal_cap_only_refuted
#print axioms Botho.Lottery.Ideal.ce2_expected_profit
#print axioms Botho.Lottery.ideal_draw_regressions
#print axioms Botho.Lottery.modulo_bias_regression

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

-- Botho.Supply.Transactions (#1894; checked finite model, named correspondence premises)
#print axioms Botho.Supply.Transactions.checked_sum_exact
#print axioms Botho.Supply.Transactions.transaction_balance
#print axioms Botho.Supply.Transactions.batch_balance
#print axioms Botho.Supply.Transactions.consume_partition
#print axioms Botho.Supply.Transactions.checked_keys_sources
#print axioms Botho.Supply.Transactions.insertion_preserves
#print axioms Botho.Supply.Transactions.ordinary_step_preserves
#print axioms Botho.Supply.Transactions.ordinary_step_correspondence
#print axioms Botho.Supply.Transactions.checked_ordinary_flow
#print axioms Botho.Supply.Transactions.arithmetic_success_boundaries
#print axioms Botho.Supply.Transactions.arithmetic_rejection_boundaries
#print axioms Botho.Supply.Transactions.identity_rejection_boundaries
#print axioms Botho.Supply.Transactions.fresh_key_needs_correspondence
#print axioms Botho.Supply.Transactions.valid_update_boundary

#print axioms Botho.Supply.Transactions.consume_insert_preserves
#print axioms Botho.Supply.Transactions.consume_insert_correspondence

-- Botho.Supply.Block (#1895; structured model composition, explicit production boundaries)
#print axioms Botho.Supply.Block.emission_exact
#print axioms Botho.Supply.Block.prepared_payouts
#print axioms Botho.Supply.Block.step_projects
#print axioms Botho.Supply.Block.step_cross_addition
#print axioms Botho.Supply.Block.step_frame
#print axioms Botho.Supply.Block.prepared_widths
#print axioms Botho.Supply.Block.step_invariant
#print axioms Botho.Supply.Block.genesis_invariant
#print axioms Botho.Supply.Block.reachable_invariants
#print axioms Botho.Supply.Block.canonical_no_winner_error
#print axioms Botho.Supply.Block.emission_boundaries
#print axioms Botho.Supply.Block.first_successor_boundary
#print axioms Botho.Supply.Block.award_count_boundaries
#print axioms Botho.Supply.Block.payout_rejection_boundaries
#print axioms Botho.Supply.Block.domain_rejection_boundaries
#print axioms Botho.Supply.Block.shared_identity_boundaries
#print axioms Botho.Supply.Block.height_and_loss_boundaries
#print axioms Botho.Supply.Block.summary_and_wire_rejection_boundaries

-- Botho.Monetary (#1921; checked production policy, explicit supply domain/refutations)
#print axioms Botho.Monetary.phase_safe
#print axioms Botho.Monetary.halving_nonincreasing
#print axioms Botho.Monetary.tail_success_iff
#print axioms Botho.Monetary.tail_safe_domain
#print axioms Botho.Monetary.tail_u64_domain
#print axioms Botho.Monetary.demurrage_current
#print axioms Botho.Monetary.block_reward_safe
#print axioms Botho.Monetary.successor_safe
#print axioms Botho.Monetary.successor_failures
#print axioms Botho.Monetary.lottery_emission_bound
#print axioms Botho.Monetary.phase1_exact_sum
#print axioms Botho.Monetary.production_domain_numbers
#print axioms Botho.Monetary.synthetic_tail_counterexamples
#print axioms Botho.Monetary.unrestricted_tail_claims_refuted
#print axioms Botho.Monetary.tail_boundary_values
#print axioms Botho.Monetary.phase_boundary_values
#print axioms Botho.Monetary.first_tail_value
#print axioms Botho.Monetary.successor_boundary_values

-- Botho.Lottery.Conservation (#1849; proven (model), including saturation loss).
#print axioms Botho.Lottery.splitFees_conserves
#print axioms Botho.Lottery.splitFees_canonical_conserves
#print axioms Botho.Lottery.totalPaid_drawEligible
#print axioms Botho.Lottery.drawing_paid
#print axioms Botho.Lottery.drawing_conserves
#print axioms Botho.Lottery.drawing_conserves_no_loss
#print axioms Botho.Lottery.drawing_widths

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

-- Botho.Tags.Vector (the wallet's merge_weighted, #1840; status: proven (model) + tested)
#print axioms Botho.Tags.mergeWeighted_sub_convex
#print axioms Botho.Tags.mergeWeighted_mass_le_scale
#print axioms Botho.Tags.mergeWeighted_isSome
#print axioms Botho.Tags.decayEntry_le
#print axioms Botho.Tags.decayEntry_antitone
#print axioms Botho.Tags.mergeWeighted_truncation_halves_mass
#print axioms Botho.Tags.mergeWeighted_overflow_panics
#print axioms Botho.Tags.mergeWeighted_no_panic_4294
#print axioms Botho.Tags.mergeWeighted_decay_not_antitone

-- Botho.Tags.Inherit (consensus C6 and the fee-floor tag reads, #1840;
-- status: proven (model) + tested, except tag_drop_escapes_import_floor:
-- trusted, hypotheses on the fee curve and the ring-centroid factor)
#print axioms Botho.Tags.inheritOk_bound
#print axioms Botho.Tags.inheritOk_cons_fresh
#print axioms Botho.Tags.inheritOk_of_untagged
#print axioms Botho.Tags.inherit_admits_fresh_mass_above_one
#print axioms Botho.Tags.fresh_cluster_credit_unbounded
#print axioms Botho.Tags.importFloor_of_untagged
#print axioms Botho.Tags.tag_drop_demurrage_factor
#print axioms Botho.Tags.tag_drop_escapes_import_floor
#print axioms Botho.Tags.import_floor_invariant_refuted

-- Botho.Difficulty.Step (#1841; status: proven (model); tested by
-- Botho/Difficulty/Vectors.lean against compute_time_adjusted_difficulty)
#print axioms Botho.Difficulty.max_difficulty_value
#print axioms Botho.Difficulty.step_in_bounds
#print axioms Botho.Difficulty.step_no_overflow
#print axioms Botho.Difficulty.step_truncates_at_max
#print axioms Botho.Difficulty.step_ratio
#print axioms Botho.Difficulty.step_mono_current
#print axioms Botho.Difficulty.step_mono_observed
#print axioms Botho.Difficulty.step_slow
#print axioms Botho.Difficulty.step_fast
#print axioms Botho.Difficulty.step_on_target
#print axioms Botho.Difficulty.step_max_absorbing
#print axioms Botho.Difficulty.floor_entry_requires_hard_block
#print axioms Botho.Difficulty.step_hardening_bounded

-- Botho.Difficulty.Warp (#1841, #1842; status: proven (model); `run` tested
-- against EmissionController::record_block; header timestamp rules and the
-- validator clock `now` are trusted inputs read from the code. The header
-- rules are protocol <= 7.x: the lockout/time-warp theorems are the record
-- of finding #1848, fixed in 8.0.0 (#1869))
#print axioms Botho.Difficulty.run_le_pow
#print axioms Botho.Difficulty.run_ge_div_pow
#print axioms Botho.Difficulty.run_slow_exact
#print axioms Botho.Difficulty.run_fast_exact
#print axioms Botho.Difficulty.easing_to_max_in_8
#print axioms Botho.Difficulty.no_max_in_7
#print axioms Botho.Difficulty.sustained_9s_reaches_max_in_10
#print axioms Botho.Difficulty.sustained_8s_reaches_max_in_12
#print axioms Botho.Difficulty.sustained_7s_reaches_max_in_17
#print axioms Botho.Difficulty.sustained_6s_reaches_max_in_31
#print axioms Botho.Difficulty.sustained_le_5s_never_max
#print axioms Botho.Difficulty.max_absorbing_run
#print axioms Botho.Difficulty.applyChain_bounds
#print axioms Botho.Difficulty.forward_dating_reaches_max_in_8
#print axioms Botho.Difficulty.time_warp_to_floor_in_55
#print axioms Botho.Difficulty.no_floor_in_54
#print axioms Botho.Difficulty.hardening_from_max_in_63
#print axioms Botho.Difficulty.floor_needs_attacker_block_at_3
#print axioms Botho.Difficulty.alternating_pair_bound
#print axioms Botho.Difficulty.forward_date_locks_out_honest
#print axioms Botho.Difficulty.forward_dated_chain_valid

-- Botho.Consensus.SlotDuration (#1841; status: proven (model); tested by
-- Botho/Difficulty/Vectors.lean against current_slot_duration)
#print axioms Botho.Consensus.slot_in_tiers
#print axioms Botho.Consensus.tierOf_idle_iff
#print axioms Botho.Consensus.forward_dated_block_forces_idle

-- Botho.Timestamps (#1941; status: proven (model), with zero-skew acceptance
-- refuted in the timestamp-rule model; Rust mapping read from source, no DRT yet)
#print axioms Botho.Timestamps.short_window_zero
#print axioms Botho.Timestamps.rolling_mtp_monotone
#print axioms Botho.Timestamps.honest_stamp_headroom_iff
#print axioms Botho.Timestamps.zero_skew_counterexample
#print axioms Botho.Timestamps.some_observed_gap_floor
