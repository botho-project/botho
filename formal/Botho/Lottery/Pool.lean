/-
Fee split, reward cap, pool accounting and the consensus draw entry point
(#1707).

Mirrors `botho/src/consensus/lottery.rs`. Status: tested (DRT legs
`split_fees`, `reward_cap`, `compute_pool_accounting`, `carryover_after`,
`draw_lottery_winners_seeded`).
-/
import Botho.Lottery.Draw

namespace Botho.Lottery

/-- `LotteryFeeConfig` (`botho/src/consensus/lottery.rs`).
`poolFractionPermille` is a `u32`. -/
structure FeeConfig where
  poolFractionPermille : Nat
  draw : DrawConfig
  deriving Repr

/-- Mirrors `LotteryFeeConfig::split_fees` (`botho/src/consensus/lottery.rs:167`):
`pool = (total_fees as u128 * permille as u128 / 1000) as u64` and
`burn = total_fees.saturating_sub(pool)`. The `as u64` keeps the low 64 bits;
it only truncates when `permille > 1000`, which no configuration uses. -/
def splitFees (cfg : FeeConfig) (totalFees : Nat) : Nat × Nat :=
  -- `total_fees * permille < 2^96`: no `u128` overflow.
  let pool := truncU64 (totalFees * cfg.poolFractionPermille / 1000)
  (pool, satSub totalFees pool)

/-- `LotteryPoolAccounting` (`botho/src/consensus/lottery.rs`). `available` is
a `u128`; the other fields are `u64`. -/
structure Accounting where
  feePool : Nat
  feeBurn : Nat
  emissionShare : Nat
  available : Nat
  payout : Nat
  deriving Repr, DecidableEq

/-- Mirrors `compute_pool_accounting` (`botho/src/consensus/lottery.rs:326`):
`available = stored_pool ⊕ emission_share ⊕ fee_pool` (saturating `u128`
additions) and `payout = min(available, payout_cap)`. -/
def computePoolAccounting (totalFees emissionShare storedPool payoutCap : Nat)
    (cfg : FeeConfig) : Accounting :=
  let (feePool, feeBurn) := splitFees cfg totalFees
  let available := satAdd U128_MAX (satAdd U128_MAX storedPool emissionShare) feePool
  -- `min(available, payout_cap) as u64` is lossless: it is at most the
  -- `u64` cap.
  { feePool, feeBurn, emissionShare, available,
    payout := minN available payoutCap }

/-- Mirrors `LotteryPoolAccounting::carryover_after`
(`botho/src/consensus/lottery.rs:283`): `available.saturating_sub(distributed)`. -/
def Accounting.carryoverAfter (a : Accounting) (distributed : Nat) : Nat :=
  satSub a.available distributed

/-- Mirrors `reward_cap` (`botho/src/consensus/lottery.rs:306`):
`sybil_reward_cap(ρ).min(block_reward as u128) as u64`, with ρ from
`count_eligible`. The final cast is lossless (the minimum is at most the
`u64` block reward). -/
def rewardCap (candidates : List Candidate) (blockHeight blockReward : Nat)
    (cfg : FeeConfig) : Nat :=
  minN (sybilRewardCap (countEligible candidates blockHeight cfg.draw)) blockReward

/-- `BlockLotteryResult` (`botho/src/consensus/lottery.rs`). -/
structure BlockLotteryResult where
  blockHeight : Nat
  totalFees : Nat
  poolAmount : Nat
  burnAmount : Nat
  winners : List Winner
  seed : Bytes
  deriving Repr

/-- Mirrors `BlockLotteryResult::no_winners` (`botho/src/consensus/lottery.rs`):
nothing paid, only the fee burn share burned, a zero seed. -/
def BlockLotteryResult.noWinners (blockHeight totalFees : Nat) (a : Accounting) :
    BlockLotteryResult :=
  { blockHeight, totalFees, poolAmount := 0, burnAmount := a.feeBurn,
    winners := [], seed := zeros 32 }

/-- Mirrors `draw_lottery_winners_seeded` and `finish_draw`
(`botho/src/consensus/lottery.rs:391,410`): no draw when the payout is zero;
otherwise draw `accounting.payout` with `draw_winners_with_seed`, and fall
back to `no_winners` when that finds no eligible candidate. -/
def drawLotteryWinnersSeeded (P : Prims) (candidates : List Candidate)
    (totalFees : Nat) (a : Accounting) (blockHeight : Nat) (seed : Bytes)
    (cfg : FeeConfig) : BlockLotteryResult :=
  if a.payout = 0 then BlockLotteryResult.noWinners blockHeight totalFees a
  else
    match drawWinnersWithSeed P candidates a.payout blockHeight seed cfg.draw with
    | some r =>
      { blockHeight, totalFees, poolAmount := a.payout, burnAmount := a.feeBurn,
        winners := r.winners, seed := r.seed }
    | none => BlockLotteryResult.noWinners blockHeight totalFees a

end Botho.Lottery
