/-
Cryptographic primitives of the LotteryV2 model (#1707), as parameters.

The lottery calls two hash functions: SHA-256 (`sha2::Sha256`, for the seed,
the per-winner rolls, the candidate-window offset and the block header hash)
and RandomX (`BlockHeader::pow_hash`, for the parent's PoW output that seeds
block N). Neither is modeled. Every model function that hashes takes a
`Prims` argument instead, so:

- proofs (#1708) quantify over every choice of `Prims`: a theorem about the
  model holds for any hash function, and needs no axiom. Where a theorem needs
  a property of the hash (for example that the seed changes with the nonce),
  it takes that property as a hypothesis, labelled **trusted**;
- the DRT executable (`formal-drt/lean/`) supplies a real SHA-256, written
  in Lean there, and takes the RandomX output from the Rust side as data (it
  is never computed in Lean). The DRT leg for `lottery_seed_v2` therefore
  tests the real SHA-256 composition bit for bit.

No axiom is declared here, so `axioms-allowlist.txt` is unchanged.
-/
import Botho.Lottery.Arith

namespace Botho.Lottery

/-- A block header (`BlockHeader`, `botho/src/block.rs`). Byte fields are 32
bytes; `version` is a `u32`; the other numbers are `u64`. -/
structure Header where
  version : Nat
  prevBlockHash : Bytes
  txRoot : Bytes
  timestamp : Nat
  height : Nat
  difficulty : Nat
  nonce : Nat
  minterViewKey : Bytes
  minterSpendKey : Bytes
  deriving Repr

/-- The hash primitives the lottery uses. -/
structure Prims where
  /-- SHA-256 (`sha2::Sha256`): any byte string to 32 bytes. -/
  sha256 : Bytes → Bytes
  /-- RandomX PoW output of a header (`BlockHeader::pow_hash`,
  `botho/src/block.rs`): 32 bytes. -/
  powHash : Header → Bytes

/-- Mirrors `BlockHeader::hash` (`botho/src/block.rs`): SHA-256 over every
header field in declaration order, integers little-endian. -/
def headerHash (P : Prims) (h : Header) : Bytes :=
  P.sha256 (leBytes 4 h.version ++ h.prevBlockHash ++ h.txRoot ++
    leBytes 8 h.timestamp ++ leBytes 8 h.height ++ leBytes 8 h.difficulty ++
    leBytes 8 h.nonce ++ h.minterViewKey ++ h.minterSpendKey)

end Botho.Lottery
