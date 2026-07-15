# ADR 0004: Bridge Privacy Semantics

**Status**: Accepted
**Date**: 2026-07-13
**Decision Makers**: Core Team
**Related**: [Epic #816](https://github.com/botho-project/botho/issues/816), issues #819, #822, #823, #826; ADR 0002, 0003, 0005

## Context

Botho is privacy-by-default: senders are hidden in CLSAG rings, recipients receive to one-time stealth addresses, and amounts are hidden in Pedersen commitments. wBTH on Ethereum and Solana is fully transparent. The bridge is therefore a deliberate privacy boundary, and its exact leakage semantics must be designed rather than left implicit.

## Problem Statement

What privacy does a user retain when wrapping BTH into wBTH and unwrapping back, and how hard should v1 work to limit deanonymization at the boundary?

## Decision

**v1 re-shields the recipient on unwrap by releasing to a fresh one-time stealth address, and accepts the residual amount + timing correlation across the bridge as documented behavior.** Denomination-bucketing and randomized-delay unlinkability is deferred to a later privacy enhancement.

Specifically:

1. **Lock reveals the amount.** To mint the correct wBTH, the bridge must learn the deposited amount — via a verified Pedersen commitment opening or a cleartext amount on the deposit to the bridge address. This is an unavoidable, deliberate privacy exit. The design must ensure it leaks only the amount, not the source ring.
2. **The wrapped side is public.** Anyone holding/moving wBTH is linkable on Ethereum/Solana. The wallet UX must warn clearly that **wrapping exits the anonymity set.**
3. **Unwrap re-shields the recipient.** A burn → release pays out to a **fresh one-time stealth address** (never a reused address), breaking the on-Botho link to the destination-chain burn. The `WrappedBTH.bridgeBurn` `bthAddress` path must resolve to a fresh stealth output.
4. **Residual leakage is documented and accepted for v1.** Amount correlation and timing correlation across the bridge remain. Bucketing + delay would break them but adds real UX friction and engine complexity; it is a later enhancement, not a v1 requirement.

## Consequences

### Positive

1. **Recipient unlinkability on the way back in.** Unwrapped funds land at a fresh stealth address, so the released BTH is not trivially tied to the burner's transparent-chain identity.
2. **Ships v1 without heavy machinery.** No denomination protocol or timing-mix engine is required to launch.
3. **Honest threat model.** The exact vectors are written down (see the privacy design note under `docs/security/`), so users and integrators understand the trade-off.

### Negative

1. **Amount + timing correlation persists.** An observer correlating a distinctive wrap amount with a same-size unwrap shortly after can link the two ends. Sophisticated deanonymization at the boundary is possible in v1.
2. **Wrapping is a one-way privacy exit** for the wrapped value while it stays on the transparent chain.

### Neutral

1. Because only factor-1/background coins are wrappable (ADR 0003), the amount revealed at lock is of a background coin — the cluster/wealth dimension is already the least sensitive class.
2. Bucketing/delay remains an open future enhancement, tracked separately if pursued.

## Alternatives Considered

### 1. Buckets + randomized delay to break correlation

- Pro: meaningfully breaks amount + timing linkage across the bridge.
- Con: real UX friction (fixed denominations, added latency) and engine complexity; deferred rather than blocking v1.

### 2. No re-shield promise (fully public)

- Pro: least work.
- Con: weakest privacy story for a privacy chain; rejected — re-shielding on release is cheap and worth it.

## Implementation

The lock-side amount-revelation mechanism (commitment opening vs. cleartext) is pinned in #823 (deposit watcher). Fresh-stealth release is enforced in #822. The user-facing "wrapping exits the anonymity set" warning is a wallet/UX task. A privacy design note documenting the accepted vectors goes under `docs/security/`.

## References

- Epic #816; issues #819, #822, #823
- ADR 0002 (custody), ADR 0003 (peg), ADR 0005 (chain scope)
