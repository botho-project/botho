# ADR 0005: Bridge v1 Chain Scope — Ethereum and Solana

**Status**: Accepted
**Date**: 2026-07-13
**Decision Makers**: Core Team
**Related**: [Epic #816](https://github.com/botho-project/botho/issues/816), issue #820; ADR 0002–0004

## Context

The existing bridge scaffold already models both Ethereum (`contracts/ethereum/contracts/WrappedBTH.sol`, `EthereumConfig`) and Solana (`contracts/solana/programs/wbth`, `SolanaConfig`). The first bridge release must pick which chains it ships and audits.

## Problem Statement

Does bridge v1 launch Ethereum-only, or Ethereum and Solana together?

## Decision

**Bridge v1 ships both Ethereum and Solana.** Every downstream work item (mint, release, watchers, contract hardening, reserve accounting, testing, audit) covers both chains.

## Consequences

### Positive

1. **Both major DeFi ecosystems at launch.** Users reach Ethereum and Solana liquidity from day one.
2. **Solana is comparatively cheap on the custody axis.** Solana uses **Ed25519**, the same key type as Botho node keys, so the SCP-validator federation (ADR 0002) signs Solana mint authorizations natively — only the Ethereum side needs the secp256k1/Gnosis-Safe detour. The added chain is lighter than a second EVM chain would be.
3. **The peg invariant generalizes cleanly:** `Σ(wBTH on ETH) + Σ(wBTH on SOL) == locked BTH reserve` (ADR 0003, #825).

### Negative

1. **Larger surface.** Implementation, testing, and audit roughly double versus Ethereum-only: two mint paths (alloy + Anchor CPI), two burn watchers, two token programs to harden, and two contract audits.
2. **Slower to a safe launch.** Both chains must clear the Phase-3 security gate (#830) before any mainnet value — the audit scope now spans Solidity and the Anchor program.

### Neutral

1. Solana finality/commitment semantics differ from Ethereum confirmations; the watchers (#823) handle each chain's reorg/finality model separately.
2. If schedule pressure emerges, Ethereum can still ship first with Solana following — the decision commits to both in scope, not to simultaneous release under all conditions.

## Alternatives Considered

### 1. Ethereum-only for v1 (original recommendation)

- Pro: minimal custody/testing/audit surface; fastest path to a safe, audited launch; Ethereum is where the deepest DeFi liquidity is.
- Con: leaves the already-scaffolded Solana program unused and Solana users unserved at launch. The maintainer chose to serve both.

## Implementation

Ripples into #821 (mint on both chains), #822/#823 (release + watchers handle burns from either chain), #826 (harden both `WrappedBTH.sol` and the `wbth` Anchor program), #825 (two-chain peg invariant), #828/#829 (test both — EVM fork tests and Solana test-validator), and #830 (audit both, including an external audit of each token program).

## References

- Epic #816; issue #820
- ADR 0002 (custody), ADR 0003 (peg), ADR 0004 (privacy)
