/-
JSON-lines codec and dispatch for the DRT executable (#1706, #1707).

Wire format (one JSON object per line, both directions):

- request: `{"fn": NAME, "args": {...}}`;
- response: `{"ok": VALUE}` or `{"err": MESSAGE}`.

Every integer is a decimal string (most exceed 2^53), byte strings are
lowercase hex, `Option` is `null` or the value. The Rust side
(`formal-drt/src/`) encodes its own results in exactly this shape and
compares the two JSON values.

To add a function: decode its arguments here, call the model, encode the
result, and add one `| "name" => ...` arm to `dispatch`.
-/
import Lean.Data.Json
import Botho
import Drt.Sha256
import Drt.Tags

namespace Drt

open Lean (Json)
open Botho.Lottery

abbrev Dec := Except String

/-! ## Decoding -/

def field (j : Json) (k : String) : Dec Json :=
  match j.getObjVal? k with
  | .ok v => .ok v
  | .error _ => .error s!"missing field '{k}'"

def asNat (j : Json) : Dec Nat := do
  let s ← j.getStr?
  match s.toNat? with
  | some n => pure n
  | none => throw s!"not a decimal string: {s}"

def hexVal (c : Char) : Dec Nat :=
  if '0' ≤ c ∧ c ≤ '9' then pure (c.toNat - '0'.toNat)
  else if 'a' ≤ c ∧ c ≤ 'f' then pure (c.toNat - 'a'.toNat + 10)
  else throw s!"bad hex digit {c}"

def hexBytes : List Char → Dec (List UInt8)
  | [] => pure []
  | a :: b :: rest => do
    let hi ← hexVal a
    let lo ← hexVal b
    let tl ← hexBytes rest
    pure ((hi * 16 + lo).toUInt8 :: tl)
  | [_] => throw "odd-length hex"

def asBytes (j : Json) : Dec Bytes := do hexBytes (← j.getStr?).toList

def asBool (j : Json) : Dec Bool := j.getBool?

def asArr (j : Json) : Dec (List Json) := do pure (← j.getArr?).toList

def nat (j : Json) (k : String) : Dec Nat := do asNat (← field j k)
def bytes (j : Json) (k : String) : Dec Bytes := do asBytes (← field j k)
def arr (j : Json) (k : String) : Dec (List Json) := do asArr (← field j k)

def decMode (j : Json) : Dec SelectionMode := do
  match ← (← field j "kind").getStr? with
  | "uniform" => pure .uniform
  | "value_weighted" => pure .valueWeighted
  | "cluster_weighted" => pure .clusterWeighted
  | "hybrid" => pure (.hybrid (← nat j "alpha_bps"))
  | k => throw s!"unknown selection mode {k}"

def decDrawConfig (j : Json) : Dec DrawConfig := do
  pure { winnersPerDraw := ← nat j "winners_per_draw", minUtxoAge := ← nat j "min_utxo_age",
         minUtxoValue := ← nat j "min_utxo_value",
         circulationWindow := ← nat j "circulation_window", mode := ← decMode (← field j "mode") }

def decFeeConfig (j : Json) : Dec FeeConfig := do
  pure { poolFractionPermille := ← nat j "pool_fraction_permille",
         draw := ← decDrawConfig (← field j "draw") }

def decCandidate (j : Json) : Dec Candidate := do
  pure { id := ← bytes j "id", value := ← nat j "value",
         clusterFactor := ← nat j "cluster_factor", creationBlock := ← nat j "creation_block" }

def decCandidates (j : Json) (k : String) : Dec (List Candidate) := do
  (← arr j k).mapM decCandidate

def decAccounting (j : Json) : Dec Accounting := do
  pure { feePool := ← nat j "fee_pool", feeBurn := ← nat j "fee_burn",
         emissionShare := ← nat j "emission_share", available := ← nat j "available",
         payout := ← nat j "payout" }

def decUtxo (j : Json) : Dec Utxo := do
  pure { id := ← bytes j "id", amount := ← nat j "amount", createdAt := ← nat j "created_at",
         clusterFactor := ← nat j "cluster_factor" }

def decHeader (j : Json) : Dec Header := do
  pure { version := ← nat j "version", prevBlockHash := ← bytes j "prev_block_hash",
         txRoot := ← bytes j "tx_root", timestamp := ← nat j "timestamp",
         height := ← nat j "height", difficulty := ← nat j "difficulty", nonce := ← nat j "nonce",
         minterViewKey := ← bytes j "minter_view_key",
         minterSpendKey := ← bytes j "minter_spend_key" }

/-! ## Encoding -/

def encNat (n : Nat) : Json := Json.str (toString n)

def encBytes (b : Bytes) : Json := Json.str (Sha256.toHex ⟨b.toArray⟩)

def encWinner (w : Winner) : Json :=
  Json.mkObj [("utxo_id", encBytes w.utxoId), ("payout", encNat w.payout)]

def encDrawResult (r : DrawResult) : Json :=
  Json.mkObj [("block_height", encNat r.blockHeight), ("pool_amount", encNat r.poolAmount),
    ("winners", Json.arr (r.winners.map encWinner).toArray), ("seed", encBytes r.seed)]

def encAccounting (a : Accounting) : Json :=
  Json.mkObj [("fee_pool", encNat a.feePool), ("fee_burn", encNat a.feeBurn),
    ("emission_share", encNat a.emissionShare), ("available", encNat a.available),
    ("payout", encNat a.payout)]

def encBlockResult (r : BlockLotteryResult) : Json :=
  Json.mkObj [("block_height", encNat r.blockHeight), ("total_fees", encNat r.totalFees),
    ("pool_amount", encNat r.poolAmount), ("burn_amount", encNat r.burnAmount),
    ("winners", Json.arr (r.winners.map encWinner).toArray), ("seed", encBytes r.seed)]

/-- A candidate as the window leg compares it: the cluster factor is the fee
curve's output, which the lottery model takes as an input. -/
def encWindowCandidate (c : Candidate) : Json :=
  Json.mkObj [("id", encBytes c.id), ("value", encNat c.value),
    ("creation_block", encNat c.creationBlock)]

def encDrawingError : DrawingError → String
  | .awardBound => "award_bound"
  | .seedParent => "seed_parent"
  | .feeOverflow => "fee_overflow"
  | .noWinners => "no_winners"

/-! ## Dispatch -/

/-- The primitives the DRT runs the model with: the real SHA-256, and the
RandomX output of the one header the request names (computed in Rust). -/
def prims (powHash : Bytes) : Prims :=
  { sha256 := Sha256.hashList, powHash := fun _ => powHash }

/-- Primitives for functions that never call RandomX. -/
def shaOnly : Prims := prims []

/-- Thin checked decoding into the landed monetary model; no alternate reward formula. -/
def monetaryResult (r : Except Botho.Monetary.Error Nat) : Dec Json :=
  match r with
  | .ok n => .ok (encNat n)
  | .error e => .error s!"monetary model error: {reprStr e}"

def monetary (a : Json) : Dec Json := do
  let height ← nat a "height"
  let reward ← nat a "reward"
  let wrapperSupply ← nat a "wrapper_supply"
  let supply ← nat a "supply"
  if height > U64_MAX ∨ reward > U64_MAX ∨ wrapperSupply > U64_MAX then
    throw "monetary input exceeds u64"
  if supply > U128_MAX / 200 then throw "monetary supply exceeds arithmetic-safe domain A"
  let blockReward ← monetaryResult (Botho.Monetary.blockRewardAt height supply)
  pure (Json.mkObj [
    ("halving_phase", Json.bool (Botho.Monetary.isHalvingPhase height)),
    ("halving_reward", match Botho.Monetary.halvingReward height with
      | some r => encNat r | none => Json.null),
    ("tail_u128", ← monetaryResult (Botho.Monetary.tailRewardU128 supply)),
    ("tail_u64", ← monetaryResult (Botho.Monetary.tailRewardU64 wrapperSupply)),
    ("library_reward", blockReward), ("node_reward", blockReward),
    ("lottery_bps", encNat (Botho.Supply.Block.emissionBps height Botho.Monetary.halvingInterval)),
    ("lottery_share", encNat (Botho.Supply.Block.emission height reward Botho.Monetary.halvingInterval)),
    ("demurrage_bps", encNat (Botho.Monetary.demurrageRateBps height))])

/-- Timestamp-only decoding: decimal ASCII strings in the full u64 domain. -/
def timestampNat (j : Json) : Dec Nat := do
  let s ← j.getStr?
  if s.isEmpty || !s.toList.all (fun c => '0' ≤ c && c ≤ '9') then
    throw "expected decimal u64 string"
  let n ← asNat j
  if n > Botho.Timestamps.U then throw "timestamp exceeds u64"
  pure n

/-- Five model helper outputs and two explicitly named scalar acceptance adapters. -/
def timestamps (a : Json) : Dec Json := do
  for (key, _) in (← a.getObj?).toList do
    if !["window", "parent", "stamp", "minter_now", "validator_now"].contains key then
      throw s!"unknown timestamp field {key}"
  let w ← (← arr a "window").mapM timestampNat
  let p ← timestampNat (← field a "parent")
  let t ← timestampNat (← field a "stamp")
  let n ← timestampNat (← field a "minter_now")
  let v ← timestampNat (← field a "validator_now")
  let h := Botho.Timestamps.honestStamp w n
  let lower t := Json.bool (decide (Botho.Timestamps.mtp w < t))
  let upper t := Json.bool (decide (t ≤ Botho.Timestamps.futureCap v))
  let accepts t := Json.bool (decide (Botho.Timestamps.accepts w t v))
  let minting := if v > Botho.Timestamps.U - 60 then
      Json.mkObj [("error", Json.str "validator_now_out_of_domain")]
    else Json.mkObj [("contextual_candidate", accepts t), ("contextual_honest", accepts h),
      ("intrinsic_candidate", upper t), ("intrinsic_honest", upper h)]
  pure (Json.mkObj [
    ("mtp", encNat (Botho.Timestamps.mtp w)),
    ("next_min", encNat (Botho.Timestamps.nextMin w)), ("honest_stamp", encNat h),
    ("rolled", Json.arr ((Botho.Timestamps.roll w t).map encNat).toArray),
    ("observed_gap", match Botho.Timestamps.observedGap p t with
      | some g => encNat g | none => Json.null),
    ("lower_candidate", lower t), ("lower_honest", lower h),
    ("ledger_future_candidate", upper t), ("ledger_future_honest", upper h),
    ("ledger_candidate", accepts t), ("ledger_honest", accepts h), ("minting", minting)])

/-- Stateful production-simulation comparison. Snapshots (including prior-award
references resolved by the independent Rust/node inventory), owners, fee-floor,
emission and V2 seeds are external assumptions. Only arithmetic carryover is
advanced here. Both named slots contain this SAME landed model, not two oracles.
The state slot specifies success for independently checked Rust public state. -/
def lotterySim (a : Json) : Dec Json := do
  let cfg : FeeConfig := { poolFractionPermille := 800, draw := DrawConfig.default }
  let mut carry ← nat a "carryover"
  let mut results : Array Json := #[]
  for b in ← arr a "blocks" do
    let candidates ← decCandidates b "candidates"
    let height ← nat b "height"
    let fees ← nat b "total_fees"
    let cap := rewardCap candidates height (← nat b "block_reward") cfg
    let accounting := computePoolAccounting fees (← nat b "emission_share") carry cap cfg
    let draw := drawLotteryWinnersSeeded shaOnly candidates fees accounting height
      (← bytes b "v2_seed") cfg
    let paid := draw.winners.foldl (fun total w => total + w.payout) 0
    carry := accounting.carryoverAfter paid
    results := results.push (Json.mkObj [("payout_cap", encNat cap),
      ("accounting", encAccounting accounting), ("draw", encBlockResult draw),
      ("paid", encNat paid), ("carryover", encNat carry)])
  pure (Json.mkObj [("node", Json.arr results), ("simulator", Json.arr results),
    ("simulator_state", Json.mkObj [("ok", Json.bool true), ("failed_checks", Json.arr #[])])])

def dispatch (fn : String) (a : Json) : Dec Json := do
  match fn with
  | "lottery_sim" => lotterySim a
  | "timestamps" => timestamps a
  | "monetary" => monetary a
  | "demurrage_charge" =>
    -- Protocol 9.0.0 exact kernel (#1875); `Botho.Demurrage.charge` is the
    -- legacy pre-9.0.0 model its counterexample theorems are about.
    pure (encNat (Botho.Demurrage.exactCharge (← nat a "value") (← nat a "factor")
      (← nat a "elapsed") (← nat a "rate") (← nat a "year")))
  | "demurrage_composition" =>
    let value ← nat a "value"
    let inputFactor ← nat a "input_factor"
    let outputFactor ← nat a "output_factor"
    let rate ← nat a "rate"
    let year ← nat a "year"
    pure (Json.mkObj [
      ("reset", encNat (Botho.Demurrage.exactResetCharge value inputFactor outputFactor
        (← nat a "horizon") rate year)),
      ("settlement", encNat (Botho.Demurrage.exactSettlementCharge value inputFactor rate year)),
      ("spend", encNat (Botho.Demurrage.exactSpendCharge value inputFactor outputFactor
        (← nat a "elapsed") rate year))])
  | "default_cluster_factor" =>
    pure (encNat (Botho.Demurrage.defaultFactor (← nat a "wealth")))
  | "ring_centroid_factor" =>
    let members ← (← arr a "members").mapM fun m => do
      pure (← nat m "value", ← nat m "wealth")
    -- Protocol 9.0.0 (#1876): the exact centroid. The pre-fix saturating
    -- `ringFactor` stays in the model as the historical witness only.
    pure (encNat (Botho.Demurrage.ringFactorExact members))
  | "lottery_seed_v2" =>
    pure (encBytes (lotterySeedV2 shaOnly (← bytes a "pow_hash") (← bytes a "prev_hash")
      (← nat a "height")))
  | "is_eligible" =>
    pure (Json.bool ((← decCandidate (← field a "candidate")).isEligible (← nat a "height")
      (← decDrawConfig (← field a "config"))))
  | "count_eligible" =>
    pure (encNat (countEligible (← decCandidates a "candidates") (← nat a "height")
      (← decDrawConfig (← field a "config"))))
  | "sybil_reward_cap" =>
    pure (encNat (sybilRewardCap (← nat a "count")))
  | "split_fees" =>
    let cfg : FeeConfig :=
      { poolFractionPermille := ← nat a "pool_fraction_permille", draw := DrawConfig.default }
    let (pool, burn) := splitFees cfg (← nat a "total_fees")
    pure (Json.arr #[encNat pool, encNat burn])
  | "reward_cap" =>
    pure (encNat (rewardCap (← decCandidates a "candidates") (← nat a "height")
      (← nat a "block_reward") (← decFeeConfig (← field a "config"))))
  | "compute_pool_accounting" =>
    pure (encAccounting (computePoolAccounting (← nat a "total_fees") (← nat a "emission_share")
      (← nat a "stored_pool") (← nat a "payout_cap") (← decFeeConfig (← field a "config"))))
  | "carryover_after" =>
    pure (encNat ((← decAccounting (← field a "accounting")).carryoverAfter
      (← nat a "distributed")))
  | "draw_winners_with_seed" =>
    let r := drawWinnersWithSeed shaOnly (← decCandidates a "candidates") (← nat a "pool_amount")
      (← nat a "height") (← bytes a "seed") (← decDrawConfig (← field a "config"))
    pure (match r with
      | some r => encDrawResult r
      | none => Json.null)
  | "draw_lottery_winners_seeded" =>
    pure (encBlockResult (drawLotteryWinnersSeeded shaOnly (← decCandidates a "candidates")
      (← nat a "total_fees") (← decAccounting (← field a "accounting")) (← nat a "height")
      (← bytes a "seed") (← decFeeConfig (← field a "config"))))
  | "lottery_candidates" =>
    let rows ← (← arr a "rows").mapM decUtxo
    let cs := lotteryCandidates shaOnly rows (← nat a "height") (← bytes a "seed")
      (← decDrawConfig (← field a "config"))
    pure (Json.arr (cs.map encWindowCandidate).toArray)
  | "drawing" =>
    let rows ← (← arr a "rows").mapM decUtxo
    let reads : Reads :=
      { lotteryPool := ← nat a "lottery_pool", tip := ← decHeader (← field a "tip"), rows }
    let b ← field a "block"
    let block : DrawBlock :=
      { height := ← nat b "height", prevBlockHash := ← bytes b "prev_block_hash",
        fees := ← (← arr b "fees").mapM asNat, reward := ← nat b "reward",
        emissionShare := ← nat b "emission_share" }
    let P := prims (← bytes a "tip_pow_hash")
    let windowSeed := match seed P reads block with
      | .ok s => [encBytes s]
      | .error _ => []
    pure (match drawing P reads block with
      | .ok (r, acc) =>
        Json.mkObj [("result", Json.mkObj [("draw", encBlockResult r),
          ("accounting", encAccounting acc)]), ("windows", Json.arr windowSeed.toArray)]
      | .error e =>
        Json.mkObj [("error", Json.str (encDrawingError e)),
          ("windows", Json.arr (if e == .seedParent || e == .awardBound then #[]
            else windowSeed.toArray))])
  | "difficulty_step" =>
    pure (encNat (Botho.Difficulty.step (← nat a "current") (← nat a "observed")
      (← nat a "target")))
  | "difficulty_run" =>
    pure (encNat (Botho.Difficulty.run (← nat a "start") (← (← arr a "gaps").mapM asNat)))
  | "slot_duration" =>
    let feed ← (← arr a "feed").mapM fun b => do
      pure ({ timestamp := ← nat b "ts", txCount := ← nat b "tx" } : Botho.Consensus.RecentBlockInfo)
    let w := feed.foldl Botho.Consensus.recordBlock []
    pure (encNat (Botho.Consensus.slotSecs (← asBool (← field a "dynamic")) (← nat a "cfg") w))
  | other =>
    -- The cluster-tag legs (#1840) live in `Drt/Tags.lean`.
    match Tags.dispatch other a with
    | some r => r
    | none => throw s!"unknown fn {other}"

/-- Handle one request line. -/
def handleLine (line : String) : Json :=
  let r : Dec Json := do
    let req ← Json.parse line
    let fn ← (← field req "fn").getStr?
    dispatch fn (← field req "args")
  match r with
  | .ok v => Json.mkObj [("ok", v)]
  | .error e => Json.mkObj [("err", Json.str e)]

end Drt
