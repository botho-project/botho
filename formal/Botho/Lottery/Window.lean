/-
The seeded candidate window (#573, #1676, #1707).

Mirrors the candidate read `lottery_candidates`
(`botho/src/ledger/store/validation.rs:733`), reached by consensus through
`DerivationReads::lottery_candidates` (`botho/src/ledger/lottery_v2.rs:168`),
and its offset `Ledger::lottery_candidate_offset_key`
(`botho/src/ledger/store.rs:2811`). Status: tested (DRT leg
`lottery_candidates`, against the production reader over a real LMDB
ledger).

The UTXO set is modeled as the list of rows of the UTXO table in key order,
which is how LMDB iterates it. So the model's input is sorted strictly by
`id`. The per-UTXO cluster factor is an input (the fee curve is outside this
model; see #1711).

Not modeled: the strict read policy's failures (a corrupt or inconsistent
row is an error there, and is skipped under the legacy policy). The model
assumes every row decodes.
-/
import Botho.Lottery.Candidate
import Botho.Lottery.Prims

namespace Botho.Lottery

/-- `MAX_LOTTERY_CANDIDATES` (`botho/src/ledger/store/validation.rs`). -/
def MAX_LOTTERY_CANDIDATES : Nat := 10000

/-- One row of the UTXO table, as far as the candidate read uses it.
`id` is the 36-byte key (`tx_hash ‖ output_index_le`). -/
structure Utxo where
  id : Bytes
  amount : Nat
  createdAt : Nat
  clusterFactor : Nat
  deriving Repr

/-- Mirrors `Ledger::lottery_candidate_offset_key`
(`botho/src/ledger/store.rs:2811`): the 36-byte start key of the window.
`head = SHA256("LOTTERY_CANDIDATE_OFFSET_V1" ‖ seed ‖ height_le)`, then four
bytes of `SHA256("LOTTERY_CANDIDATE_OFFSET_V1_TAIL" ‖ head)`. -/
def offsetKey (P : Prims) (seed : Bytes) (blockHeight : Nat) : Bytes :=
  let head := P.sha256 (ascii "LOTTERY_CANDIDATE_OFFSET_V1" ++ seed ++ leBytes 8 blockHeight)
  let tail := P.sha256 (ascii "LOTTERY_CANDIDATE_OFFSET_V1_TAIL" ++ head)
  head ++ tail.take 4

/-- The wraparound walk: the rows with key `≥ offset`, then the rows with key
`< offset` (`upper.chain(lower)` over the two LMDB ranges). -/
def rotate (offset : Bytes) (rows : List Utxo) : List Utxo :=
  rows.filter (fun u => !bytesLt u.id offset) ++ rows.filter (fun u => bytesLt u.id offset)

/-- The candidate a row becomes (`LotteryCandidate::new`, which clamps the
factor into `[FACTOR_SCALE, MAX_FACTOR_SCALED]`). -/
def Utxo.toCandidate (u : Utxo) : Candidate :=
  { id := u.id, value := u.amount,
    clusterFactor := clampN FACTOR_SCALE MAX_FACTOR_SCALED u.clusterFactor,
    creationBlock := u.createdAt }

/-- The scan loop: stop once `cap` candidates are collected (checked before
each row, as in the Rust), keep each eligible row. `taken` is
`candidates.len()`. The eligibility test is the inline one in the Rust, which
is `LotteryCandidate::is_eligible` on the row's fields. -/
def scan (cap blockHeight : Nat) (cfg : DrawConfig) : List Utxo → Nat → List Candidate
  | [], _ => []
  | u :: us, taken =>
    if cap ≤ taken then []
    else if u.toCandidate.isEligible blockHeight cfg then
      u.toCandidate :: scan cap blockHeight cfg us (taken + 1)
    else scan cap blockHeight cfg us taken

/-- Mirrors `lottery_candidates` (`botho/src/ledger/store/validation.rs:733`):
the first `MAX_LOTTERY_CANDIDATES` eligible rows in the rotated key order.
`rows` must be sorted strictly by `id`. -/
def lotteryCandidates (P : Prims) (rows : List Utxo) (blockHeight : Nat)
    (seed : Bytes) (cfg : DrawConfig) : List Candidate :=
  scan MAX_LOTTERY_CANDIDATES blockHeight cfg (rotate (offsetKey P seed blockHeight) rows) 0

end Botho.Lottery
