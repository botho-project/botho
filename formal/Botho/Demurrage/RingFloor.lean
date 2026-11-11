/-
Amount-matched ring floor (#1686, protocol 10.0.0, ADR 0015). Models
`cluster-tax/src/ring_floor.rs::amount_matched_ring_floor_wealth` and
`amount_matched_ring_floor_factor`, the floor `consensus_fee_floor` applies to
the demurrage factor since 10.0.0 (through
`ValidationReads::ring_floored_factor_for_tx`). It replaces the pooled
value-weighted centroid `ringFactorExact` (`Exact.lean`) on the consensus
path; that model stays as the model of `ring_centroid_implied_factor`, which
the simulations still call.

Per input: the members whose amount equals the input's pseudo-output amount
(every member if none does), as `tag_retention.rs::amount_matched` selects
them; the input's floor wealth is the largest of their wealths. Across inputs:
the exact mean of those floor wealths weighted by pseudo-output amount
(`wide::WeightedMean`, 256-bit numerator), clamped at `u128::MAX` once, then
the default curve.

Status: model; tested (finite DRT, leg `ring_floor_factor`). No theorems are
stated here yet: new statements need their records approved on the base
commit first (`formal/statements.txt`), so the dilution-resistance and
no-over-charge properties follow in a separate change.
-/
import Botho.Demurrage.Exact

namespace Botho.Demurrage
open Botho.Lottery

/-- `tag_retention.rs::amount_matched` over `(amount, wealth)` members: the
members of the pseudo-output amount, or every member if none matches. -/
def amountMatched (pseudo : Nat) (members : List (Nat × Nat)) : List (Nat × Nat) :=
  let matching := members.filter (fun m => m.1 == pseudo)
  if matching.isEmpty then members else matching

/-- `ring_floor.rs::input_floor_wealth`: the largest wealth among the
amount-matched members; 0 for an empty ring. -/
def inputFloorWealth (pseudo : Nat) (members : List (Nat × Nat)) : Nat :=
  (amountMatched pseudo members).foldl (fun acc m => Nat.max acc m.2) 0

/-- `ring_floor.rs::amount_matched_ring_floor_wealth` over
`(pseudo amount, members)` inputs: the pseudo-weighted exact mean of the
per-input floor wealths, clamped once at the end; 0 with no value. -/
def ringFloorWealth (inputs : List (Nat × List (Nat × Nat))) : Nat :=
  let (weighted, total) := inputs.foldl (fun (a : Nat × Nat) i =>
    (a.1 + i.1 * inputFloorWealth i.1 i.2, a.2 + i.1)) (0, 0)
  if total = 0 then 0 else minN (weighted / total) U128_MAX

/-- `ring_floor.rs::amount_matched_ring_floor_factor` with the default curve. -/
def ringFloorFactor (inputs : List (Nat × List (Nat × Nat))) : Nat :=
  defaultFactor (ringFloorWealth inputs)

end Botho.Demurrage
