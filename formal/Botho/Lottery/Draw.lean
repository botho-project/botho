/-
The seeded LotteryV2 winner draw (#1707).

Mirrors `draw_winners_with_seed` and `draw_eligible` in
`cluster-tax/src/lottery.rs`: per-candidate weights, weighted sampling
without replacement against the weight of the candidates not yet drawn, at
most `winners_per_draw` winners, an equal split of the pool and the division
remainder to the last winner. Status: tested (DRT leg
`draw_winners_with_seed`).
-/
import Botho.Lottery.Prims
import Botho.Lottery.Candidate

namespace Botho.Lottery

/-- `LotteryWinner` (`cluster-tax/src/lottery.rs`). `payout` is a `u64`. -/
structure Winner where
  utxoId : Bytes
  payout : Nat
  deriving Repr

/-- `LotteryResult` (`cluster-tax/src/lottery.rs`). -/
structure DrawResult where
  blockHeight : Nat
  poolAmount : Nat
  winners : List Winner
  seed : Bytes
  deriving Repr

/-- Mirrors `verifiable_random_u128` (`cluster-tax/src/lottery.rs:498`): the
little-endian `u128` in the first 16 bytes of `SHA256(seed ‖ index_le)`. -/
def verifiableRandomU128 (P : Prims) (seed : Bytes) (index : Nat) : Nat :=
  ofLeBytes ((P.sha256 (seed ++ leBytes 8 index)).take 16)

/-- One eligible candidate during the draw: its weight and whether it has
already won (the Rust `used_indices` set, kept next to the candidate). -/
structure Slot where
  cand : Candidate
  weight : Nat
  used : Bool

/-- `remaining_total` in `draw_eligible`: the weight of the candidates not yet
drawn. (A `u128` sum in Rust; it cannot overflow, see `Candidate.weight`.) -/
def remainingTotal : List Slot → Nat
  | [] => 0
  | s :: ss => (if s.used then 0 else s.weight) + remainingTotal ss

/-- The cumulative walk in `draw_eligible`: skip drawn candidates, add each
other weight to `cumulative`, and stop at the first candidate where
`cumulative > roll`. Returns that candidate, if any, and the slots with it
marked drawn. -/
def selectAt (roll : Nat) : Nat → List Slot → Option Candidate × List Slot
  | _, [] => (none, [])
  | cumulative, s :: ss =>
    if s.used then
      let r := selectAt roll cumulative ss
      (r.1, s :: r.2)
    else
      let cumulative' := cumulative + s.weight
      if roll < cumulative' then
        (some s.cand, { s with used := true } :: ss)
      else
        let r := selectAt roll cumulative' ss
        (r.1, s :: r.2)

/-- The `for i in 0..num_winners` loop of `draw_eligible`. `i` is the roll
index, `left` the number of iterations still to run. Stops early when every
remaining weight is zero (the Rust `break`). -/
def pickWinners (P : Prims) (seed : Bytes) (payoutPerWinner : Nat) :
    Nat → Nat → List Slot → List Winner
  | _, 0, _ => []
  | i, left + 1, slots =>
    let remaining := remainingTotal slots
    if remaining = 0 then []
    else
      let roll := verifiableRandomU128 P seed i % remaining
      match selectAt roll 0 slots with
      | (some c, slots') =>
        { utxoId := c.id, payout := payoutPerWinner } ::
          pickWinners P seed payoutPerWinner (i + 1) left slots'
      | (none, slots') => pickWinners P seed payoutPerWinner (i + 1) left slots'

/-- Add `extra` to the payout of the last winner (`winners.last_mut()`). -/
def addToLast (extra : Nat) : List Winner → List Winner
  | [] => []
  | [w] => [{ w with payout := w.payout + extra }]
  | w :: ws => w :: addToLast extra ws

/-- Sum of the payouts (`u64` in Rust; bounded by the pool amount). -/
def totalPaid : List Winner → Nat
  | [] => 0
  | w :: ws => w.payout + totalPaid ws

/-- `eligible.iter().map(|c| c.value).max()`. -/
def maxValue? : List Candidate → Option Nat
  | [] => none
  | c :: cs =>
    match maxValue? cs with
    | none => some c.value
    | some m => some (if c.value < m then m else c.value)

/-- Mirrors `draw_eligible` (`cluster-tax/src/lottery.rs:361`).

Precondition: `cfg.winnersPerDraw > 0` (the Rust divides by
`min(winners_per_draw, eligible.len())`, which panics at 0; the consensus
configuration is fixed at 4). The callers guarantee a non-empty `eligible`
list and a positive `poolAmount`. -/
def drawEligible (P : Prims) (eligible : List Candidate) (poolAmount blockHeight : Nat)
    (seed : Bytes) (cfg : DrawConfig) : Option DrawResult :=
  let mv := (maxValue? eligible).getD 1
  let slots := eligible.map fun c => ({ cand := c, weight := c.weight cfg.mode mv, used := false } : Slot)
  if remainingTotal slots = 0 then none
  else
    let numWinners := minN cfg.winnersPerDraw eligible.length
    let payoutPerWinner := poolAmount / numWinners
    let winners := pickWinners P seed payoutPerWinner 0 numWinners slots
    -- `pool_amount - total_paid` cannot underflow: at most `numWinners`
    -- winners are paid `poolAmount / numWinners` each.
    some { blockHeight, poolAmount,
           winners := addToLast (poolAmount - totalPaid winners) winners, seed }

/-- Mirrors `draw_winners_with_seed` (`cluster-tax/src/lottery.rs:341`): filter
to the eligible candidates, then draw; `none` when nobody is eligible or the
pool is empty. -/
def drawWinnersWithSeed (P : Prims) (candidates : List Candidate)
    (poolAmount blockHeight : Nat) (seed : Bytes) (cfg : DrawConfig) :
    Option DrawResult :=
  let eligible := candidates.filter fun c => c.isEligible blockHeight cfg
  if eligible.isEmpty || poolAmount = 0 then none
  else drawEligible P eligible poolAmount blockHeight seed cfg

end Botho.Lottery
