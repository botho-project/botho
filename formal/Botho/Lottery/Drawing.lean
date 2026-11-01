/-
The canonical LotteryV2 drawing of one block, end to end (#1707).

Mirrors `lottery_v2::seed` and `lottery_v2::drawing`
(`botho/src/ledger/lottery_v2.rs:220,238`): the PoW-bound seed from the
parent header, the seeded candidate window, the checked fee sum, the reward
cap, the pool accounting and the draw, in that order. Status: tested (DRT leg
`drawing`, against `drawing` over an in-memory `DerivationReads` double).

The pinned pre-state (`DerivationReads`) is modeled by `Reads`: the stored
pool, the committed tip header and the UTXO table rows. The block is modeled
by the fields `drawing` reads. `emissionShare` is
`MintingTx::lottery_emission_share()`, an input here (the emission schedule
is #1710).
-/
import Botho.Lottery.Seed
import Botho.Lottery.Window
import Botho.Lottery.Pool

namespace Botho.Lottery

/-- `CANDIDATE_MAX_AWARDS` (`transaction/clsag/src/lottery_v2.rs`). -/
def CANDIDATE_MAX_AWARDS : Nat := 4

/-- `canonical_config` (`botho/src/ledger/lottery_v2.rs`): 80% of fees to the
pool and the default draw configuration. -/
def canonicalConfig : FeeConfig :=
  { poolFractionPermille := 800, draw := DrawConfig.default }

/-- The pinned pre-state `drawing` reads through `DerivationReads`. -/
structure Reads where
  /-- `DerivationReads::lottery_pool` (a `u128`). -/
  lotteryPool : Nat
  /-- `DerivationReads::tip_header`: the parent of the drawn block. -/
  tip : Header
  /-- The UTXO table in key order (see `lotteryCandidates`). -/
  rows : List Utxo

/-- The fields of `Block` that `drawing` reads. -/
structure DrawBlock where
  height : Nat
  prevBlockHash : Bytes
  /-- `t.fee` of each of `block.transactions`, in order. -/
  fees : List Nat
  /-- `block.minting_tx.reward`. -/
  reward : Nat
  /-- `block.minting_tx.lottery_emission_share()`. -/
  emissionShare : Nat

/-- The errors `drawing` can return, in the order it checks them. -/
inductive DrawingError where
  /-- `invalid("experimental award bound")`. -/
  | awardBound
  /-- `invalid("V2 seed parent")`. -/
  | seedParent
  /-- `LedgerError::FeeOverflow`. -/
  | feeOverflow
  /-- `inconsistent("positive eligible draw returned no winners")`. -/
  | noWinners
  deriving Repr, DecidableEq

/-- Mirrors `lottery_v2::seed` (`botho/src/ledger/lottery_v2.rs:220`): the
block must sit at `tip.height + 1` (checked `u64` addition) on `tip.hash()`;
then `lottery_seed_v2(tip.pow_hash(), tip.prev_block_hash, height)`. -/
def seed (P : Prims) (r : Reads) (b : DrawBlock) : Except DrawingError Bytes :=
  if checkedAdd U64_MAX r.tip.height 1 = some b.height ∧ headerHash P r.tip = b.prevBlockHash then
    .ok (lotterySeedV2 P (P.powHash r.tip) r.tip.prevBlockHash b.height)
  else .error .seedParent

/-- The fee total: `try_fold` with `u64::checked_add`, left to right. -/
def sumFeesChecked : Nat → List Nat → Option Nat
  | acc, [] => some acc
  | acc, f :: fs =>
    match checkedAdd U64_MAX acc f with
    | some acc' => sumFeesChecked acc' fs
    | none => none

/-- Mirrors `lottery_v2::drawing` (`botho/src/ledger/lottery_v2.rs:238`). -/
def drawing (P : Prims) (r : Reads) (b : DrawBlock) :
    Except DrawingError (BlockLotteryResult × Accounting) :=
  let config := canonicalConfig
  if CANDIDATE_MAX_AWARDS < config.draw.winnersPerDraw then .error .awardBound
  else
    match seed P r b with
    | .error e => .error e
    | .ok s =>
      let candidates := lotteryCandidates P r.rows b.height s config.draw
      match sumFeesChecked 0 b.fees with
      | none => .error .feeOverflow
      | some fees =>
        let cap := rewardCap candidates b.height b.reward config
        let accounting := computePoolAccounting fees b.emissionShare r.lotteryPool cap config
        let draw := drawLotteryWinnersSeeded P candidates fees accounting b.height s config
        if accounting.payout ≠ 0 ∧ !candidates.isEmpty ∧ draw.winners.isEmpty then
          .error .noWinners
        else .ok (draw, accounting)

end Botho.Lottery
