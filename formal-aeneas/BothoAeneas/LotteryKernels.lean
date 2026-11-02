import MonetaryKernels
import BothoAeneas.Basic

/-!
# Lottery and emission kernels, proven about the extraction

Mirrors (by extraction, not by hand), as generated into
`generated/MonetaryKernels.lean` by `regen.sh` after the #1820 rewrites:

* `cluster-tax/src/lottery.rs`: `sybil_reward_cap`, `is_eligible_at`;
* `cluster-tax/src/monetary.rs`: `MonetaryPolicy::lottery_emission_bps`
  and `lottery_emission_share`;
* `cluster-tax/src/demurrage.rs`: `capitalized_reset_charge` (its
  no-downgrade case only).

`count_eligible_in` and the `demurrage_charge` arithmetic are extracted (and
covered by the axiom gate) but have no theorems here yet.

Label: **proven (extraction)**. "Returns `ok`" means the Rust function does
not panic.
-/

open Aeneas Aeneas.Std Result
open bth_cluster_tax

namespace BothoAeneas.LotteryKernels

/-- `sybil_reward_cap` is exactly `ρ · base_fee` (the `checked_mul` never
overflows: `usize` is at most 64 bits). -/
theorem sybil_reward_cap_spec (k : Usize) :
    lottery.sybil_reward_cap k ⦃ r => r.val = k.val * 250000000000 ⦄ := by
  unfold lottery.sybil_reward_cap
  step*
  have hk : k.val < 2 ^ 64 := by
    have h := k.hmax
    simp only [UScalarTy.numBits] at h
    rcases System.Platform.numBits_eq with e | e <;> rw [e] at h <;> omega
  have hi : i.val = k.val := by
    rw [i_post, UScalar.cast_val_eq]; exact Nat.mod_eq_of_lt (by simp; omega)
  have hi1 : i1.val = 250000000000 := by
    rw [i1_post, UScalar.cast_val_eq]; simp [lottery.LOTTERY_BASE_FEE_PICO]
  rcases o with _ | z
  · simp only [hi, hi1, U128.max, U128.numBits, UScalarTy.numBits] at o_post
    omega
  · obtain ⟨-, hz, -⟩ := o_post
    simp [hz, hi, hi1]

/-- `is_eligible_at` is the Path C window test, with `age` the truncated
difference `current_block - creation_block` (Rust's `saturating_sub`). -/
theorem is_eligible_at_spec (cb v cur age win minv : U64) :
    lottery.is_eligible_at cb v cur age win minv ⦃ r =>
      r = decide (age.val ≤ cur.val - cb.val ∧ cur.val - cb.val ≤ win.val ∧
        minv.val ≤ v.val) ⦄ := by
  unfold lottery.is_eligible_at
  simp [lift, u64_saturating_sub_val]
  split_ifs <;> simp_all

/-- The lottery share of the block reward in basis points, in `Nat`. -/
def emissionBps (p : monetary.MonetaryPolicy) (height : Nat) : Nat :=
  if p.halving_interval.val = 0 then 0 else min (height / p.halving_interval.val * 1000) 5000

/-- `lottery_emission_bps` is `min(epoch · 1000, 5000)`, and 0 when
`halving_interval = 0`. The `min(u32::MAX)` and the `checked_mul` fallback to
`u32::MAX` never change the result, because the cap of 5000 is below both. -/
theorem lottery_emission_bps_spec (p : monetary.MonetaryPolicy) (height : U64) :
    monetary.MonetaryPolicy.lottery_emission_bps p height ⦃ r =>
      r.val = emissionBps p height.val ⦄ := by
  unfold monetary.MonetaryPolicy.lottery_emission_bps
  split
  · simp_all [emissionBps]
  · rename_i hI0
    step*
    have hI : p.halving_interval.val ≠ 0 := by
      intro h0; apply hI0; scalar_tac
    have hstep : monetary.MonetaryPolicy.lottery_emission_bps.LOTTERY_EMISSION_STEP_BPS.val
        = 1000 := by simp [monetary.MonetaryPolicy.lottery_emission_bps.LOTTERY_EMISSION_STEP_BPS]
    have hmaxb : monetary.MonetaryPolicy.lottery_emission_bps.LOTTERY_EMISSION_MAX_BPS.val
        = 5000 := by simp [monetary.MonetaryPolicy.lottery_emission_bps.LOTTERY_EMISSION_MAX_BPS]
    have hmaxv : core.num.U32.MAX.val = 4294967295 := by
      simp [core.num.U32.MAX, U32.rMax]
    have hi1 : i1.val = 4294967295 := by
      rw [i1_post, UScalar.cast_val_eq, hmaxv]; simp [UScalarTy.numBits]
    have hep : epoch.val = min (height.val / p.halving_interval.val) 4294967295 := by
      rw [epoch_post, UScalar.cast_val_eq, i2_post, core.cmp.impls.OrdU64.min_val, hi1, i_post]
      exact Nat.mod_eq_of_lt (by simp)
    simp only [emissionBps, hI, ↓reduceIte, core.cmp.impls.OrdU32.min_val, hmaxb, i3_post]
    rcases o with _ | z
    · simp only [hstep, hep, U32.max, U32.numBits, UScalarTy.numBits] at o_post
      simp only [core.option.Option.unwrap_or_none, hmaxv]
      omega
    · obtain ⟨-, hz, -⟩ := o_post
      simp only [core.option.Option.unwrap_or_some, hz, hstep, hep]
      omega

theorem emissionBps_le (p : monetary.MonetaryPolicy) (height : Nat) :
    emissionBps p height ≤ 5000 := by
  unfold emissionBps; split <;> omega

/-- `lottery_emission_share` is `reward · bps / 10000`, with no overflow and
no truncation by the final `as u64`. -/
theorem lottery_emission_share_spec (p : monetary.MonetaryPolicy) (height reward : U64) :
    monetary.MonetaryPolicy.lottery_emission_share p height reward ⦃ r =>
      r.val = reward.val * emissionBps p height.val / 10000 ⦄ := by
  unfold monetary.MonetaryPolicy.lottery_emission_share
  step
  step with lottery_emission_bps_spec as ⟨ bps, hbps ⟩
  step*
  have hr : reward.val < 2 ^ 64 := by scalar_tac
  have hb : bps.val < 2 ^ 32 := by scalar_tac
  have hi : i.val = reward.val := by
    rw [i_post, UScalar.cast_val_eq]; exact Nat.mod_eq_of_lt (by simp; omega)
  have hi2 : i2.val = bps.val := by
    rw [i2_post, UScalar.cast_val_eq]; exact Nat.mod_eq_of_lt (by simp; omega)
  have hle : reward.val * emissionBps p height.val / 10000 ≤ reward.val :=
    Nat.div_le_of_le_mul (by
      rw [Nat.mul_comm 10000]
      exact Nat.mul_le_mul_left _ (by have := emissionBps_le p height.val; omega))
  rw [UScalar.cast_val_eq, i4_post, i3_post, hi, hi2, hbps]
  exact Nat.mod_eq_of_lt (by simp; omega)

/-- No downgrade (`output_factor ≥ input_floor_factor`), no charge. -/
theorem capitalized_reset_charge_no_downgrade (v fi fo h : U64) (rate : U32) (bpy : U64)
    (hle : fi.val ≤ fo.val) :
    demurrage.capitalized_reset_charge v fi fo h rate bpy = ok 0#u64 := by
  unfold demurrage.capitalized_reset_charge
  simp [hle]

/-- `sybil_reward_cap` never fails and returns `ρ · 250_000_000_000`. -/
theorem sybil_reward_cap_eq (k : Usize) :
    ∃ r, lottery.sybil_reward_cap k = ok r ∧ r.val = k.val * 250000000000 :=
  eq_ok_of_spec (sybil_reward_cap_spec k)

/-- `is_eligible_at` never fails and returns the window test. -/
theorem is_eligible_at_eq (cb v cur age win minv : U64) :
    lottery.is_eligible_at cb v cur age win minv =
      ok (decide (age.val ≤ cur.val - cb.val ∧ cur.val - cb.val ≤ win.val ∧
        minv.val ≤ v.val)) := by
  obtain ⟨r, hr, h⟩ := eq_ok_of_spec (is_eligible_at_spec cb v cur age win minv)
  rw [hr, h]

/-- The lottery share never exceeds half the block reward (the 50% cap),
and `lottery_emission_share` never fails. -/
theorem lottery_emission_share_le_half (p : monetary.MonetaryPolicy) (height reward : U64) :
    ∃ r, monetary.MonetaryPolicy.lottery_emission_share p height reward = ok r ∧
      r.val ≤ reward.val / 2 := by
  obtain ⟨r, hr, h⟩ := eq_ok_of_spec (lottery_emission_share_spec p height reward)
  refine ⟨r, hr, ?_⟩
  rw [h]
  have := emissionBps_le p height.val
  calc reward.val * emissionBps p height.val / 10000
      ≤ reward.val * 5000 / 10000 := Nat.div_le_div_right (Nat.mul_le_mul_left _ this)
    _ = reward.val / 2 := by omega

/-- Epoch 0 (`height < halving_interval`): no lottery share. -/
theorem lottery_emission_share_epoch0 (p : monetary.MonetaryPolicy) (height reward : U64)
    (h0 : height.val < p.halving_interval.val) :
    monetary.MonetaryPolicy.lottery_emission_share p height reward = ok 0#u64 := by
  obtain ⟨r, hr, h⟩ := eq_ok_of_spec (lottery_emission_share_spec p height reward)
  have he : emissionBps p height.val = 0 := by
    unfold emissionBps
    split
    · rfl
    · rw [Nat.div_eq_of_lt h0]; rfl
  rw [hr]
  congr
  apply UScalar.eq_of_val_eq
  rw [h, he]
  simp

end BothoAeneas.LotteryKernels
