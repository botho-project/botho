import MonetaryKernels
import BothoAeneas.Basic

/-!
# `MonetaryPolicy::calculate_tail_reward_u128`, proven about the extraction

Mirrors (by extraction, not by hand): `cluster-tax/src/monetary.rs`,
`MonetaryPolicy::calculate_tail_reward_u128`, as generated into
`generated/MonetaryKernels.lean` by `regen.sh`.

Label: **proven (extraction)**. "Returns `ok`" means the Rust function does
not panic: the extraction turns an overflowing `*` or `+` and a division by
zero into `fail`. The final `reward.max(1) as u64` is a truncating cast in
both Rust and the extraction (`UScalar.cast .U64`, value mod `2^64`).

Discrepancy with the statement in #1821: the issue lists "result ≥ 1" next
to "no failure for `target_block_time_secs ∈ [1, 31_536_000]` and supply
below the overflow bound". Under those hypotheses alone it is false, because
`as u64` can truncate a reward of `k * 2^64` to `0`
(`calculate_tail_reward_u128_truncates_to_zero`). What holds is "result ≥ 1
whenever the pre-cast reward is below `2^64`"
(`calculate_tail_reward_u128_ge_one`), and the mainnet parameters keep the
reward below `2^64` for every supply below `2^91`
(`calculate_tail_reward_u128_mainnet`). This is not a bug in the node at
realistic supplies (the design supply is about `6.1e20` picocredits, under
`2^70`), so no Rust change follows from it.
-/

open Aeneas Aeneas.Std Result
open bth_cluster_tax

namespace BothoAeneas.TailReward

abbrev Policy := monetary.MonetaryPolicy

/-- Seconds per year as the Rust code computes it (`365 * 24 * 3600`). -/
abbrev secsPerYear : Nat := 31536000

/-- The reward before the final `max(1)` and `as u64`, in `Nat`. -/
def rewardNat (p : Policy) (supply : Nat) : Nat :=
  (supply * p.tail_inflation_bps.val / 10000
    + supply * p.expected_fee_burn_rate_bps.val / 10000)
  / (secsPerYear / p.target_block_time_secs.val)

theorem calculate_tail_reward_u128_spec (p : Policy) (supply : U128)
    (ht₁ : 1 ≤ p.target_block_time_secs.val)
    (ht₂ : p.target_block_time_secs.val ≤ secsPerYear)
    (hs : supply.val * (p.tail_inflation_bps.val + p.expected_fee_burn_rate_bps.val)
      ≤ U128.max) :
    monetary.MonetaryPolicy.calculate_tail_reward_u128 p supply ⦃ r =>
      r.val = max (rewardNat p supply.val) 1 % 2 ^ 64 ⦄ := by
  unfold monetary.MonetaryPolicy.calculate_tail_reward_u128
  step*
  · have hi5 : i5.val = p.target_block_time_secs.val := by
      rw [i5_post, UScalar.cast_val_eq]; scalar_tac
    rw [blocks_per_year_post, secs_per_year_post, i4_post, hi5]
    exact Nat.ne_of_gt (Nat.div_pos ht₂ ht₁)
  · have hi5 : i5.val = p.target_block_time_secs.val := by
      rw [i5_post, UScalar.cast_val_eq]; scalar_tac
    have hi : i.val = p.tail_inflation_bps.val := by
      rw [i_post, UScalar.cast_val_eq]; scalar_tac
    have hi2 : i2.val = p.expected_fee_burn_rate_bps.val := by
      rw [i2_post, UScalar.cast_val_eq]; scalar_tac
    have hr : reward.val = rewardNat p supply.val := by
      rw [reward_post, gross_needed_post, target_net_post, expected_burns_post,
        i1_post, i3_post, blocks_per_year_post, secs_per_year_post, i4_post,
        hi, hi2, hi5]
      rfl
    rw [UScalar.cast_val_eq, i6_post, core.cmp.impls.OrdU128.max_val, hr]
    rfl

/-- No failure (no panic, no divergence) for `target_block_time_secs` in
`[1, 31_536_000]` and a supply below the overflow bound
`supply * (tail_inflation_bps + expected_fee_burn_rate_bps) ≤ u128::MAX`. -/
theorem calculate_tail_reward_u128_no_fail (p : Policy) (supply : U128)
    (ht₁ : 1 ≤ p.target_block_time_secs.val)
    (ht₂ : p.target_block_time_secs.val ≤ secsPerYear)
    (hs : supply.val * (p.tail_inflation_bps.val + p.expected_fee_burn_rate_bps.val)
      ≤ U128.max) :
    ∃ r, monetary.MonetaryPolicy.calculate_tail_reward_u128 p supply = ok r := by
  obtain ⟨r, hr, -⟩ := eq_ok_of_spec (calculate_tail_reward_u128_spec p supply ht₁ ht₂ hs)
  exact ⟨r, hr⟩

/-- A concrete supply bound that implies the overflow bound for every
policy: both rates are `u32`, so `supply < 2^95` suffices. -/
theorem overflow_bound_of_lt_two_pow_95 (p : Policy) (supply : U128)
    (hs : supply.val < 2 ^ 95) :
    supply.val * (p.tail_inflation_bps.val + p.expected_fee_burn_rate_bps.val)
      ≤ U128.max := by
  have ht : p.tail_inflation_bps.val < 2 ^ 32 := p.tail_inflation_bps.hBounds
  have hb : p.expected_fee_burn_rate_bps.val < 2 ^ 32 := p.expected_fee_burn_rate_bps.hBounds
  have hsum : p.tail_inflation_bps.val + p.expected_fee_burn_rate_bps.val ≤ 2 ^ 33 := by
    omega
  calc supply.val * (p.tail_inflation_bps.val + p.expected_fee_burn_rate_bps.val)
      ≤ supply.val * 2 ^ 33 := Nat.mul_le_mul_left _ hsum
    _ ≤ (2 ^ 95 - 1) * 2 ^ 33 := Nat.mul_le_mul_right _ (by omega)
    _ ≤ U128.max := by rw [U128.max_eq]; decide

/-- The result is at least 1 whenever the `as u64` cast does not truncate,
that is, whenever the pre-cast reward is below `2^64`. In that case the
result is exactly `max(reward, 1)`. Without that hypothesis the claim is
false: see `calculate_tail_reward_u128_truncates_to_zero`. -/
theorem calculate_tail_reward_u128_ge_one (p : Policy) (supply : U128)
    (ht₁ : 1 ≤ p.target_block_time_secs.val)
    (ht₂ : p.target_block_time_secs.val ≤ secsPerYear)
    (hs : supply.val * (p.tail_inflation_bps.val + p.expected_fee_burn_rate_bps.val)
      ≤ U128.max)
    (hfit : rewardNat p supply.val < 2 ^ 64) :
    ∃ r, monetary.MonetaryPolicy.calculate_tail_reward_u128 p supply = ok r ∧
      r.val = max (rewardNat p supply.val) 1 ∧ 1 ≤ r.val := by
  obtain ⟨r, hr, hv⟩ :=
    eq_ok_of_spec (calculate_tail_reward_u128_spec p supply ht₁ ht₂ hs)
  have hmax : max (rewardNat p supply.val) 1 < 2 ^ 64 := by
    rcases Nat.le_total (rewardNat p supply.val) 1 with h | h
    · rw [Nat.max_eq_right h]; decide
    · rw [Nat.max_eq_left h]; exact hfit
  rw [Nat.mod_eq_of_lt hmax] at hv
  exact ⟨r, hr, hv, hv ▸ Nat.le_max_right _ _⟩

/-- Counterexample to an unconditional "result ≥ 1": with
`target_block_time_secs = 31_536_000` (one block per year, inside the
allowed range), `tail_inflation_bps = 10_000`, no fee burn, and
`supply = 2^64` (far below the overflow bound), the pre-cast reward is
`2^64` and `as u64` truncates it to `0`. -/
theorem calculate_tail_reward_u128_truncates_to_zero (p : Policy) (supply : U128)
    (ht : p.target_block_time_secs.val = secsPerYear)
    (hi : p.tail_inflation_bps.val = 10000)
    (hb : p.expected_fee_burn_rate_bps.val = 0)
    (hs : supply.val = 2 ^ 64) :
    ∃ r, monetary.MonetaryPolicy.calculate_tail_reward_u128 p supply = ok r ∧
      r.val = 0 := by
  obtain ⟨r, hr, hv⟩ := eq_ok_of_spec
    (calculate_tail_reward_u128_spec p supply
      (by rw [ht]; decide) (by rw [ht]) (by rw [hs, hi, hb, U128.max_eq]; decide))
  refine ⟨r, hr, ?_⟩
  rw [hv, rewardNat, hs, hi, hb, ht]
  decide

/-! ## Mainnet

`botho/src/monetary.rs::mainnet_policy()` (mirrored by
`bth_cluster_tax::mainnet_monetary_policy()`) sets
`tail_inflation_bps = 200`, `expected_fee_burn_rate_bps = 50` and
`target_block_time_secs = 5`. The other fields do not enter
`calculate_tail_reward_u128`. -/

/-- Under the mainnet tail parameters and any supply below `2^91`
picocredits (about 2.5e15 BTH, over a million times the ~611M BTH design
supply), the function does not fail, `as u64` does not truncate, and the
result is `max(reward, 1) ≥ 1`. The bound is close to tight: at
`supply = 2^92` the pre-cast reward exceeds `2^64`. -/
theorem calculate_tail_reward_u128_mainnet (p : Policy) (supply : U128)
    (ht : p.target_block_time_secs.val = 5)
    (hi : p.tail_inflation_bps.val = 200)
    (hb : p.expected_fee_burn_rate_bps.val = 50)
    (hs : supply.val < 2 ^ 91) :
    ∃ r, monetary.MonetaryPolicy.calculate_tail_reward_u128 p supply = ok r ∧
      r.val = max ((supply.val * 200 / 10000 + supply.val * 50 / 10000) / 6307200) 1 ∧
      1 ≤ r.val := by
  have hrw : rewardNat p supply.val =
      (supply.val * 200 / 10000 + supply.val * 50 / 10000) / 6307200 := by
    rw [rewardNat, hi, hb, ht, show secsPerYear / 5 = 6307200 by decide]
  have hfit : rewardNat p supply.val < 2 ^ 64 := by
    rw [hrw]; omega
  obtain ⟨r, hr, hv, h1⟩ := calculate_tail_reward_u128_ge_one p supply
    (by omega) (by rw [ht]; decide) (by rw [hi, hb, U128.max_eq]; omega) hfit
  exact ⟨r, hr, hrw ▸ hv, h1⟩

/-- At `supply = 2^92` the mainnet pre-cast reward no longer fits in `u64`,
so the `2^91` bound above cannot be relaxed to `2^92`. -/
theorem mainnet_reward_exceeds_u64_at_two_pow_92 :
    2 ^ 64 ≤ ((2 ^ 92 : Nat) * 200 / 10000 + (2 ^ 92 : Nat) * 50 / 10000) / 6307200 := by
  decide

end BothoAeneas.TailReward
