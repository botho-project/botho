import PoolAccounting
import Botho.Lottery.Pool
import BothoAeneas.Basic

/-!
# Lottery pool accounting, proven about the extraction

Mirrors (by extraction, not by hand): `botho/src/consensus/lottery.rs`,
`compute_pool_accounting_permille` and `split_fees_permille`, as generated
into `generated/PoolAccounting.lean` by `regen.sh`.
`compute_pool_accounting` (which takes a `LotteryFeeConfig`) is a thin
wrapper that passes `config.pool_fraction_permille` to the first (#1820).

Label: **proven (extraction)**. The hypothesis `pool_fraction_permille ≤
1000` holds for the ratified 800; above 1000 the final `as u64` of the fee
split can truncate (not covered here).
-/

open Aeneas Aeneas.Std Result
open botho

namespace BothoAeneas.PoolAccounting

/-- The fee split: the pool gets `⌊total · permille / 1000⌋` and the burn the
rest, so nothing is created or lost. -/
theorem split_fees_permille_spec (total : U64) (pm : U32) (h : pm.val ≤ 1000) :
    consensus.lottery.split_fees_permille total pm ⦃ r =>
      r.1.val = total.val * pm.val / 1000 ∧ r.1.val + r.2.val = total.val ⦄ := by
  unfold consensus.lottery.split_fees_permille
  step*
  simp [lift, u64_saturating_sub_val, pool_amount_post, i3_post, i2_post, i_post, i1_post, UScalar.cast_val_eq]
  have ht : total.val < 2 ^ 64 := by scalar_tac
  have hp : pm.val < 2 ^ 32 := by scalar_tac
  have hle : total.val * pm.val / 1000 ≤ total.val :=
    Nat.div_le_of_le_mul (by rw [Nat.mul_comm 1000]; exact Nat.mul_le_mul_left _ h)
  have h1 : total.val % 340282366920938463463374607431768211456 = total.val :=
    Nat.mod_eq_of_lt (by omega)
  have h2 : pm.val % 340282366920938463463374607431768211456 = pm.val :=
    Nat.mod_eq_of_lt (by omega)
  have h3 : total.val * pm.val / 1000 % 18446744073709551616 = total.val * pm.val / 1000 :=
    Nat.mod_eq_of_lt (by omega)
  rw [h1, h2, h3]
  omega

/-- The whole accounting: fee conservation, the pool share, the saturating
carryover `available = min(stored + emission + fee_pool, u128::MAX)` and
`payout = min(available, payout_cap)`. -/
theorem compute_pool_accounting_permille_spec (total emission : U64) (stored : U128)
    (cap : U64) (pm : U32) (h : pm.val ≤ 1000) :
    consensus.lottery.compute_pool_accounting_permille total emission stored cap pm ⦃ r =>
      r.fee_pool.val + r.fee_burn.val = total.val ∧
      r.fee_pool.val = total.val * pm.val / 1000 ∧
      r.emission_share = emission ∧
      r.available.val = min (stored.val + emission.val + r.fee_pool.val) U128.max ∧
      r.payout.val = min r.available.val cap.val ⦄ := by
  unfold consensus.lottery.compute_pool_accounting_permille
  step with split_fees_permille_spec as ⟨ fp, fb, hpool, hsum ⟩
  simp [lift, u128_saturating_add_val, UScalar.cast_val_eq]
  have he : emission.val < 2 ^ 64 := by scalar_tac
  have hf : fp.val < 2 ^ 64 := by scalar_tac
  have hc : cap.val < 2 ^ 64 := by scalar_tac
  have hs : stored.val ≤ U128.max := by scalar_tac
  have e1 : emission.val % 340282366920938463463374607431768211456 = emission.val :=
    Nat.mod_eq_of_lt (by omega)
  have e2 : fp.val % 340282366920938463463374607431768211456 = fp.val :=
    Nat.mod_eq_of_lt (by omega)
  have e3 : cap.val % 340282366920938463463374607431768211456 = cap.val :=
    Nat.mod_eq_of_lt (by omega)
  rw [e1, e2, e3]
  exact ⟨hsum, hpool, by omega, Nat.mod_eq_of_lt (by omega)⟩

/-- `compute_pool_accounting_permille` never fails for
`pool_fraction_permille ≤ 1000`; fees are conserved
(`fee_pool + fee_burn = total_fees`), and the payout is at most both the cap
and what is available. -/
theorem compute_pool_accounting_permille_conserves (total emission : U64) (stored : U128)
    (cap : U64) (pm : U32) (h : pm.val ≤ 1000) :
    ∃ r, consensus.lottery.compute_pool_accounting_permille total emission stored cap pm = ok r ∧
      r.fee_pool.val + r.fee_burn.val = total.val ∧
      r.payout.val ≤ cap.val ∧ r.payout.val ≤ r.available.val := by
  obtain ⟨r, hr, hsum, -, -, -, hpay⟩ :=
    eq_ok_of_spec (compute_pool_accounting_permille_spec total emission stored cap pm h)
  exact ⟨r, hr, hsum, by omega, by omega⟩

/-- Below saturation the carryover is exact: `available = stored + emission +
fee_pool` whenever that sum fits in `u128`. -/
theorem compute_pool_accounting_permille_available (total emission : U64) (stored : U128)
    (cap : U64) (pm : U32) (h : pm.val ≤ 1000)
    (hfit : stored.val + emission.val + total.val ≤ U128.max) :
    ∃ r, consensus.lottery.compute_pool_accounting_permille total emission stored cap pm = ok r ∧
      r.available.val = stored.val + emission.val + r.fee_pool.val := by
  obtain ⟨r, hr, hsum, -, -, havail, -⟩ :=
    eq_ok_of_spec (compute_pool_accounting_permille_spec total emission stored cap pm h)
  exact ⟨r, hr, by omega⟩

/-- Structural representation map only: no arithmetic is reimplemented here. -/
def toModel (r : consensus.lottery.LotteryPoolAccounting) : Botho.Lottery.Accounting :=
  { feePool := r.fee_pool.val, feeBurn := r.fee_burn.val,
    emissionShare := r.emission_share.val, available := r.available.val,
    payout := r.payout.val }

/-- The extracted integer kernel returns successfully and equals the actual
canonical model, on the entire typed domain with a valid fee fraction.
In particular, stored carryover may saturate; `draw` is arbitrary. -/
theorem compute_pool_accounting_permille_model
    (total emission : U64) (stored : U128) (cap : U64) (pm : U32)
    (draw : Botho.Lottery.DrawConfig) (h : pm.val ≤ 1000) :
    ∃ r, consensus.lottery.compute_pool_accounting_permille total emission stored cap pm = ok r ∧
      toModel r = Botho.Lottery.computePoolAccounting total.val emission.val stored.val cap.val
        { poolFractionPermille := pm.val, draw := draw } := by
  obtain ⟨r, hr, hsum, hpool, hemi, havail, hpay⟩ :=
    eq_ok_of_spec (compute_pool_accounting_permille_spec total emission stored cap pm h)
  refine ⟨r, hr, ?_⟩
  have hmod : r.fee_pool.val % 2 ^ 64 = r.fee_pool.val :=
    Nat.mod_eq_of_lt (by scalar_tac)
  have hmax : U128.max = Botho.Lottery.U128_MAX := by
    simp [U128.max, U128.numBits, Botho.Lottery.U128_MAX]
  rw [hmax] at havail
  have hmodelAvail : Botho.Lottery.satAdd Botho.Lottery.U128_MAX
      (Botho.Lottery.satAdd Botho.Lottery.U128_MAX stored.val emission.val) r.fee_pool.val =
      r.available.val := by
    simp only [Botho.Lottery.satAdd]
    split_ifs <;> omega
  simp only [toModel, Botho.Lottery.computePoolAccounting, Botho.Lottery.splitFees,
    Botho.Lottery.truncU64, ← hpool, hmod, hemi, hmodelAvail]
  congr 1
  · simp only [Botho.Lottery.satSub]; split <;> omega

/-- The ratified 800-permille specialization; every draw configuration and
all typed inputs, including saturated carryover, remain in the domain. -/
theorem compute_pool_accounting_permille_model_800
    (total emission : U64) (stored : U128) (cap : U64) (draw : Botho.Lottery.DrawConfig) :
    ∃ r, consensus.lottery.compute_pool_accounting_permille total emission stored cap 800#u32 = ok r ∧
      toModel r = Botho.Lottery.computePoolAccounting total.val emission.val stored.val cap.val
        { poolFractionPermille := 800, draw := draw } := by
  exact compute_pool_accounting_permille_model total emission stored cap 800#u32 draw (by decide)

/-- Successful return from the bridge explicitly excludes panic and divergence. -/
theorem compute_pool_accounting_permille_no_fail
    (total emission : U64) (stored : U128) (cap : U64) (pm : U32)
    (draw : Botho.Lottery.DrawConfig) (h : pm.val ≤ 1000) :
    ∃ r, consensus.lottery.compute_pool_accounting_permille total emission stored cap pm = ok r := by
  obtain ⟨r, hr, _⟩ := compute_pool_accounting_permille_model total emission stored cap pm draw h
  exact ⟨r, hr⟩

end BothoAeneas.PoolAccounting
