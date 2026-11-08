-- Exported theorems about the Aeneas extraction (#1821). Each listed theorem
-- may depend only on the axioms in axioms-allowlist.txt; scripts/check-gates.sh
-- checks that. Only `import` lines and `#print axioms NAME` lines belong here.
import BothoAeneas

-- MonetaryPolicy::halving_reward (cluster-tax/src/monetary.rs)
#print axioms BothoAeneas.HalvingReward.halving_reward_no_fail
#print axioms BothoAeneas.HalvingReward.halving_reward_phase1
#print axioms BothoAeneas.HalvingReward.halving_reward_phase2
#print axioms BothoAeneas.HalvingReward.halving_reward_antitone
#print axioms BothoAeneas.HalvingReward.halving_reward_none_stays

-- MonetaryPolicy::calculate_tail_reward_u128 (cluster-tax/src/monetary.rs)
#print axioms BothoAeneas.TailReward.calculate_tail_reward_u128_spec
#print axioms BothoAeneas.TailReward.calculate_tail_reward_u128_no_fail
#print axioms BothoAeneas.TailReward.overflow_bound_of_lt_two_pow_95
#print axioms BothoAeneas.TailReward.calculate_tail_reward_u128_ge_one
#print axioms BothoAeneas.TailReward.calculate_tail_reward_u128_truncates_to_zero
#print axioms BothoAeneas.TailReward.calculate_tail_reward_u128_mainnet
#print axioms BothoAeneas.TailReward.mainnet_reward_exceeds_u64_at_two_pow_92

-- Lottery and emission kernels (cluster-tax/src/lottery.rs, monetary.rs,
-- demurrage.rs), extracted after the #1820 rewrites
#print axioms BothoAeneas.LotteryKernels.sybil_reward_cap_eq
#print axioms BothoAeneas.LotteryKernels.is_eligible_at_eq
#print axioms BothoAeneas.LotteryKernels.lottery_emission_bps_spec
#print axioms BothoAeneas.LotteryKernels.lottery_emission_share_spec
#print axioms BothoAeneas.LotteryKernels.lottery_emission_share_le_half
#print axioms BothoAeneas.LotteryKernels.lottery_emission_share_epoch0
#print axioms BothoAeneas.LotteryKernels.capitalized_reset_charge_no_downgrade

-- compute_pool_accounting_permille (botho/src/consensus/lottery.rs, #1820)
#print axioms BothoAeneas.PoolAccounting.split_fees_permille_spec
#print axioms BothoAeneas.PoolAccounting.compute_pool_accounting_permille_spec
#print axioms BothoAeneas.PoolAccounting.compute_pool_accounting_permille_conserves
#print axioms BothoAeneas.PoolAccounting.compute_pool_accounting_permille_available

-- The extracted kernels themselves (#1820): each must reach no axiom at all
#print axioms bth_cluster_tax.lottery.sybil_reward_cap
#print axioms bth_cluster_tax.lottery.is_eligible_at
#print axioms bth_cluster_tax.lottery.count_eligible_in
#print axioms bth_cluster_tax.monetary.MonetaryPolicy.lottery_emission_share
#print axioms bth_cluster_tax.demurrage.demurrage_charge
#print axioms bth_cluster_tax.demurrage.capitalized_reset_charge
#print axioms botho.consensus.lottery.compute_pool_accounting_permille

-- Equality to the canonical model, including saturated u128 carryover (#1713)
#print axioms BothoAeneas.PoolAccounting.compute_pool_accounting_permille_model
#print axioms BothoAeneas.PoolAccounting.compute_pool_accounting_permille_model_800
#print axioms BothoAeneas.PoolAccounting.compute_pool_accounting_permille_no_fail
