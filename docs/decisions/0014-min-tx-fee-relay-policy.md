# ADR 0014: Structural Transaction Limits Are Consensus; `MIN_TX_FEE` Is Relay Policy

**Status**: Accepted (operator ruling 2026-11-09, recorded on #1684)
**Date**: 2026-11-09
**Decision Makers**: Operator
**Related**: issues [#1684](https://github.com/botho-project/botho/issues/1684) (this decision), [#1772](https://github.com/botho-project/botho/issues/1772) (output cap), [#1688](https://github.com/botho-project/botho/issues/1688) (fee floor never binds; not re-decided here), epic [#1683](https://github.com/botho-project/botho/issues/1683) (design confidence; protocol 10.0.0 batch); [ADR 0007](0007-bridge-import-cluster-tagging.md) (fee floor import term)

## Context

`Transaction::is_valid_structure` (`transaction/clsag/src/lib.rs`) checks five
limits:

| Limit | Constant | Value |
|-------|----------|-------|
| Minimum ring size | `MIN_RING_SIZE` | 20 |
| Maximum outputs per transaction | `MAX_OUTPUTS` | 16 |
| Dust threshold (minimum output) | `DUST_THRESHOLD` | 1,000,000 pico |
| Zero-amount outputs | — | rejected |
| Minimum fee | `MIN_TX_FEE` | 100,000,000 pico |

Through protocol 9.x its only production caller was mempool admission
(`Mempool::add_tx`). Neither block application (`validate_ordinary_body`) nor
SCP value validation (`TransactionValidator::validate_transfer_tx`) called it.
A validator that nominates its own transactions could therefore include a
ring of 2, a dust output, 1,000 outputs, or a fee below `MIN_TX_FEE`, and
every honest node would accept the block.

The consensus fee rule that does exist is the size-based
`consensus_fee_floor` (`botho/src/ledger/store/validation.rs`):
`CONSENSUS_FEE_BASE` (1 pico/byte) × size × cluster factor × output penalty,
plus the demurrage charge. For an ordinary transaction that is roughly
870–23,000 pico, far below `MIN_TX_FEE`.

Several design arguments assumed all five limits were consensus rules: the
cost of splitting to game the lottery (#1682), the cost of a decoy flood
(#1701), and the anonymity set a ring of 20 is supposed to guarantee.

## Problem Statement

For each limit, decide whether it is a consensus rule (a block containing a
violating transaction is invalid) or relay policy (default nodes refuse to
admit and relay it, but a block producer may include it).

## Decision

1. **Ring size ≥ `MIN_RING_SIZE`, at most `MAX_OUTPUTS` outputs, and no output
   below `DUST_THRESHOLD` (zero included) are consensus rules** from protocol
   10.0.0. Each has exactly one intended value, and each protects a privacy or
   anti-spam invariant that must hold against any proposer.
2. **They are checked by one shared predicate,
   `Transaction::check_consensus_structure`**, called by:
   - SCP value validation (`validate_transfer_tx`), so a violating
     transaction is never a valid nomination;
   - block application (`validate_ordinary_body`, check C9) and its
     per-transaction mirror `verify_transfer_for_block` (the block builder's
     seal-time filter and the SCP slot check);
   - mempool admission, through `is_valid_structure`.

   Relay and consensus therefore cannot drift. The predicate is a pure
   function of the transaction, so it keeps SCP's tip-agnostic validity
   invariant.
3. **`MIN_TX_FEE` stays relay/mempool policy.** It is checked only by
   `is_valid_structure` at mempool admission. The consensus fee rule is
   `consensus_fee_floor`, unchanged. Whether the flat minimum or the
   size-based floor should bind is #1688 and is not decided here.
4. **No exemptions.** Every `Transaction` in a block body is an ordinary
   transfer, settlements included, and is held to all three rules. The
   coinbase is a separate `MintingTx` type and lottery payouts are derived
   records; neither is a `Transaction`, so neither reaches the predicate.
   This matches the mempool, which has never exempted any transaction kind.
5. **Constant values are unchanged.** Only where they are enforced changes.

## Consequences

### Positive

1. A ring of 20 is now a guarantee: no block can carry a smaller ring.
2. Lottery-splitting and decoy-flood cost arguments that rely on the output
   cap and dust floor now hold against a self-mining attacker, not only
   against relay.
3. One predicate serves relay and consensus, so a future change to either
   applies to both.

### Negative

1. Consensus-breaking: protocol 10.0.0 (`PROTOCOL_VERSION` and
   `MIN_SUPPORTED_PROTOCOL_VERSION`), fresh chains from genesis, testnet
   reset. Acceptable pre-mainnet.
2. A block producer can still include transactions paying between
   `consensus_fee_floor` and `MIN_TX_FEE`. Any sim or doc that prices an
   attack by a self-mining adversary must use `consensus_fee_floor`, not
   `MIN_TX_FEE`.

### Neutral

1. Honest wallets already build rings of exactly 20, cap outputs at 16
   (the stress grant builder uses 15 grants plus change) and fold sub-dust
   change into the fee, so they produce no transaction the new rules reject.
2. The `MAX_INPUTS` (16) bound is not part of the predicate and is unchanged
   by this ADR.

## Alternatives Considered

### 1. Enforce all five limits, including `MIN_TX_FEE`, in consensus

- Pro: matches every doc and sim that assumed a 1e8 floor.
- Con: the flat 1e8 floor would dominate the size- and factor-based
  `consensus_fee_floor` for every ordinary transaction, so the progressive
  fee would never bind in consensus either. That couples this change to the
  fee-model question #1688 is meant to decide.

### 2. Declare all five limits relay policy

- Pro: no protocol bump.
- Con: ring-size privacy and the output and dust anti-spam limits would be
  unenforced against any proposer; every sim and doc would need repricing.

## Implementation

- `transaction/clsag/src/lib.rs`: `check_consensus_structure` (shared
  predicate); `is_valid_structure` composes it with the duplicate-key-image
  check and the `MIN_TX_FEE` relay check, in the same error order as before.
- `botho/src/ledger/store/validation.rs`: `check_transfer_structure` (C9),
  called from `validate_ordinary_body` and `verify_transfer_for_block`.
- `botho/src/consensus/validation.rs`: `validate_transfer_tx` calls the
  predicate; new `ValidationError::InvalidStructure`.
- `botho/src/network/discovery.rs`: protocol 10.0.0.
- Tests: `botho/src/ledger/store/structure_consensus_tests.rs` (block
  application, both sides of each boundary, plus `MIN_TX_FEE` not enforced),
  `consensus::validation` tests (SCP, same boundaries), and the existing
  mempool `MAX_OUTPUTS` tests.

## References

- Issue #1684 ruling comment (operator, 2026-11-09)
- Issue #1772 (Part A mempool cap, PR #1780; Part B consensus cap, this ADR)
- `docs/research/decoy-flood-cost.md`
