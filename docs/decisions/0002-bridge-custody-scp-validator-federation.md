# ADR 0002: Bridge Custody & Trust Model — SCP Validator Federation

**Status**: Accepted; signer-identity model superseded by [ADR 0010](0010-elected-bridge-multisig.md) (2026-07-17 — bridge custody is a small elected multisig decoupled from SCP quorum structure; the t-of-n threshold-signing mechanics ratified here are unchanged); Solana mint-execution model refined by [ADR 0012](0012-solana-squads-pda-mint-execution.md) (2026-07-20 — Squads-v4 vault-PDA `invoke_signed`, assembly-only)
**Date**: 2026-07-13
**Decision Makers**: Core Team
**Related**: [Epic #816](https://github.com/botho-project/botho/issues/816), issues #817, #824, #826, #827; ADR 0003–0005

## Context

Botho is adding a bridge that locks native BTH and mints a wrapped ERC-20/SPL token (**wBTH**) on Ethereum and Solana, so holders can access DeFi liquidity and move value in and out (see the epic #816). A bridge is the single highest-value attack target in the system: whoever controls the mint authority on the destination chain and the reserve-release authority on Botho can print or drain funds. The existing `bridge/service` scaffold signs releases with a **single hot wallet** (`engine.rs`: "Sign with hot wallet"; config exposes `spend_key_file` / `private_key_file` / `mnemonic_file`) — an unacceptable single point of catastrophic failure.

Botho has no EVM/SVM light client, so neither chain can natively verify the other. Every privileged cross-chain action (mint on a deposit, release on a burn) must therefore be authorized by **signatures from a trusted signer set**, not by on-chain proof of the counterparty chain's state.

## Problem Statement

Who holds the threshold keys that authorize wBTH minting and BTH reserve release, and how are those keys structured so that no single party can move funds?

## Decision

**The bridge federation is the SCP validator set, operating as a t-of-n threshold signer group.** The mechanism is threshold signing on both sides; the signer *identity* is the existing consensus validators rather than a separately-curated federation or a third-party custodian.

Concretely:

- **BTH reserve release** is authorized by a t-of-n threshold of the validators' **Ed25519** node keys, reusing the operator-signed-action machinery shipped in P4.4 (`operator_action.rs` domain-separated envelopes, `operator_nonce.rs` reserve-then-apply replay protection, `rpc/operator.rs` audit log). See ADR-linked issue #824 for the attestation protocol.
- **Solana wBTH mint** is authorized by the same validators' **Ed25519** keys — Solana uses Ed25519 natively, so no new key type is required.
- **Ethereum wBTH mint** requires **secp256k1**, which SCP node keys are not. Each validator therefore also operates a secp256k1 signer, and the Ethereum mint authority is a **Gnosis Safe** whose owners are those secp256k1 keys, holding `MINTER_ROLE` on `WrappedBTH.sol` (threshold-ECDSA/TSS is an acceptable alternative producing a single on-chain signature).
- The **bridge threshold `t` is set no lower than the SCP safety threshold** — it must never be easier to move the reserve than to move consensus.

## Consequences

### Positive

1. **Reuses an audited pattern.** The operator-signed-action envelope + nonce + audit-log design was security-reviewed in cycle-8; the attestation protocol (#824) mirrors it rather than inventing new signature-verification code.
2. **Fewer distinct trust roots.** Users already trust the validator set for chain safety; no new federation membership/governance surface is introduced.
3. **Natural Solana fit.** Validators sign Solana authorizations with their existing Ed25519 keys — the second chain adds little custody complexity (ADR 0005).

### Negative

1. **Bridge safety is coupled to consensus safety.** A validator-majority compromise now also drains the reserve, and conversely bridge-key operations must meet validator-grade opsec. The two risk domains are no longer isolated.
2. **The validator set is small.** It currently has n-of-n fragility below 4 nodes; a small `n` means a small bridge quorum. **Growing and hardening the validator set is now a soft prerequisite** to holding meaningful bridge value.
3. **Validators must run a second (secp256k1) signer** for the Ethereum side, with its own provisioning, storage, and rotation (tracked in the ops runbook, #827).

### Neutral

1. The Ethereum authority is a Gnosis Safe (v1) rather than threshold-ECDSA — a battle-tested choice that can migrate to TSS later without changing the Botho side.
2. Bridge threshold `t` and SCP threshold are configured together but remain distinct knobs.

## Alternatives Considered

### 1. Separate curated federation

- Pro: isolates blast radius — a bridge-key compromise does not touch consensus and vice versa.
- Con: introduces a new signer-set governance/membership surface and a second set of keys to trust; the maintainer preferred not to stand up a distinct trust root.

### 2. Third-party / MPC custody service

- Pro: offloads key-management risk to specialists.
- Con: adds a trusted third party, recurring cost, and an external dependency contrary to the project's self-sovereign posture.

### 3. Trustless light-client bridge

- Pro: no trusted signer set at all.
- Con: infeasible — Botho has no EVM/SVM VM to run a counterparty light client, and verifying SCP proofs on Ethereum is impractical. Not available near-term.

## Implementation

See #824 (attestation/authorization protocol), #826 (contract-side threshold mint authority), and #827 (secp256k1 key provisioning + rotation in the ops runbook). Follow-on sub-decisions: exact `t` value and the Safe-vs-TSS choice for the Ethereum side.

## References

- Epic #816; issues #817, #824, #826, #827
- `botho/src/operator_action.rs`, `botho/src/operator_nonce.rs`, `botho/src/rpc/operator.rs` (reused pattern)
- ADR 0003 (peg), ADR 0004 (privacy), ADR 0005 (chain scope)
