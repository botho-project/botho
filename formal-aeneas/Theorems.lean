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
