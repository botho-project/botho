# ADR 0011: MetaMask Snap Wallet — Onboard MetaMask Users Without EVM Compromise

**Status**: Accepted (feasibility spike verdict **GO**, PR #1055; Phase-1 MVP shipped, PR #1075)
**Date**: 2026-07-18
**Decision Makers**: Core Team
**Related**: issue #815 (proposal umbrella — stays **open** for Phase-2/live-send per maintainer; this ADR documents the decision but does not close it); #1089 (Phase-2 tracker — the deferred work below); PR #1055 (Phase-0 spike, `web/packages/snap-spike`); PR #1075 (Phase-1 MVP, `web/packages/snap`); #811 (RPC endpoint / wrong-network validation, carried over); #1051 (betanet frozen — blocks live-testnet send validation); #474/#475 (web-wallet at-rest key-handling lessons — Phase-2 audit scope)

## Context

A large pool of prospective users already have MetaMask and trust its UX. Botho's
product vision is a low-friction on-ramp (mobile app / pay-from-phone). Offering a
**MetaMask [Snap](https://docs.metamask.io/snaps/)** lets those users manage a
privacy-by-default Botho wallet from inside the surface they already know.

MetaMask and Botho are incompatible at the base layer (Ed25519/Ristretto + CLSAG
ring signatures + stealth addresses + UTXO vs. secp256k1 + account-based +
transparent). A Snap is MetaMask's intended extension point for exactly this case:
non-EVM chains (Bitcoin, Solana, Starknet) ship Snaps that run their own crypto in
a sandbox. Botho already has the reusable client core — `@botho/wasm-signer`
(crate `bth-wasm-signer`), which the web wallet consumes today and which emits the
node's byte-for-byte bincode wire format — so a Snap is a **second consumer of an
already-audited core**, not a from-scratch wallet.

Issue #815 filed this as a product-direction proposal with three explicit spike
acceptance criteria and a proposed 0/1/2 phasing. This ADR records the ratified
decision and the boundary between what shipped and what is deferred; it does not
re-derive the analysis (see the two package READMEs).

## Problem Statement

Decide whether Botho ships a MetaMask Snap wallet, on what terms (crypto,
key-derivation, node-trust), and where the in-scope / deferred line sits — so the
#815 proposal umbrella has a ratified decision on record, with its remaining
Phase-2 / live-send work tracked explicitly (#1089) rather than left implicit.
(#815 itself stays open as the proposal umbrella per maintainer direction.)

## Decision

1. **Botho ships a MetaMask Snap.** The Phase-0 spike (PR #1055) returned a
   measured **GO**: `bth-wasm-signer` loads and runs correctly inside the real
   MetaMask Snaps SES executor, `buildAndSign` measured at **~26–28 ms** and the
   full client send pipeline at **~60 ms** inside the sandbox — latency is a
   non-issue. The spike funded a spike-derived wallet on a real node and the node
   **mined** a send built + CLSAG-signed + submitted from inside SES. All three
   #815 spike acceptance criteria are met (go/no-go with a latency number; a
   documented MetaMask-entropy → Botho-key derivation path with its trade-off; a
   working testnet send).

2. **The Snap is a pure client.** It makes **no change** to the Botho protocol,
   consensus, or wire format, and runs `bth-wasm-signer` for all Botho crypto —
   the same core as the web wallet. No EVM / `eth_*` compatibility, no secp256k1
   transparent txs, no bridge.

3. **Keys derive from the MetaMask SRP** via SIP-6 `snap_getEntropy` (version 1,
   salt `botho-root`), plugged in as the BIP39 entropy of the existing
   `RootIdentity` pipeline (SLIP-10 `m/44'/866'/0'` → Ristretto view/spend keys →
   node-identical ML-KEM-768 / ML-DSA-65 PQ keys). The same 24-word mnemonic
   recovers the **identical** wallet in the web / CLI / mobile wallet;
   `botho_showMnemonic` exports it (behind explicit confirmation) for an
   off-MetaMask backup. Keys never leave the sandbox.

4. **Node trust carries over from the web wallet.** The Snap talks to a
   **user-selected ingress node** and runs a wrong-network guard
   (`node_getStatus.network` must equal `botho-testnet`; loopback exempt),
   mirroring `validateRpcEndpointForNetwork` (#811).

5. **Phase-1 MVP is the shipped increment** (PR #1075, `web/packages/snap`):
   `botho_getAddress` / `botho_getBalance` / `botho_send` plus receive / balance /
   send-confirm / mnemonic-backup dialogs, tested under the official
   `@metamask/snaps-jest` SES executor against a mocked in-process node (no live
   betanet or `botho` binary required).

## Consequences

### Positive

1. MetaMask-comfortable users get a privacy-by-default Botho wallet in a surface
   they already trust, with **one backup** (the existing MetaMask SRP also
   recovers the Botho wallet) — matching the low-friction product goal.
2. Zero protocol/consensus/wire-format risk: the Snap is a pure client and a
   second consumer of the already-audited `@botho/wasm-signer` core.
3. Cross-wallet portability: the SRP-derived 24-word mnemonic recovers the same
   wallet across Snap / web / CLI / mobile.

### Negative

1. **Coupled secrets**: an SRP compromise now also compromises the Botho wallet
   (one secret, two chains).
2. **Snap-id pinning**: SIP-6 entropy is bound to the Snap's npm id; republishing
   under a different id derives a *different* wallet, so the production snap id is
   consensus-critical config. Mitigated by the `botho_showMnemonic` export.
3. A new key-handling surface widens the audit scope (Phase 2), cf. #474/#475.

### Neutral

1. The Snap still needs an ingress node for decoys + submit — it inherits the web
   wallet's node-trust model and its privacy caveats, not more, not less.
2. Distribution (npm publish + MetaMask allowlist) is a separate operational step,
   deferred with the rest of Phase 2.

## In-scope (shipped) vs. Deferred

**Shipped** (`web/packages/snap-spike` PR #1055, `web/packages/snap` PR #1075):
Phase-0 feasibility spike and Phase-1 MVP — SRP-derived keys, wasm signing under
SES, receive / balance / send with dialogs, wrong-network guard, green mocked-node
test suite.

**Deferred** (tracked in **#1089**, the Phase-2 follow-up; #815 remains the open
proposal umbrella per maintainer, not this ADR):

- **Live-testnet send validation** — a real on-chain send from the Snap needs
  owned-output fixtures only a live minting node produces, and public betanet is
  **frozen** (#1051). The send plumbing is fully tested against a mocked node; the
  end-to-end live-send is **blocked on betanet resume (#1051)**. (The spike did
  demonstrate a real mined send, but via spike-only hooks intentionally absent
  from the MVP.)
- **Phase-2 parity**: transaction history, contacts, in-Snap ingress-selection
  UX, i18n, incremental/windowed scanning with persisted scan state
  (`snap_manageState` — the shared scaling concern with the web wallet), a
  dedicated **security pass** for the new key-handling surface (cf. #474/#475),
  and **publishing** (npm + MetaMask allowlist, production snap-id pinning).

## Alternatives Considered

### 1. EVM-compatibility / bridge (make Botho speak `eth_*`)

- Pro: MetaMask works out of the box, no Snap needed.
- Con: contradicts Botho's whole design (would require secp256k1-signed
  transparent txs and a trust-bearing bridge at the base layer). **Rejected** — an
  explicit non-goal in #815.

### 2. Snap-local independent seed (`snap_manageState`, encrypted at rest)

- Pro: clean isolation from the MetaMask SRP.
- Con: needs its own backup/restore UX; losing Snap state on uninstall without a
  backup loses funds — reintroducing exactly the seed-management burden the Snap
  is meant to remove. **Rejected** for the MVP in favor of SRP-derived keys; the
  trade-off is documented in `web/packages/snap/README.md`.

### 3. Do nothing (web + mobile wallets only)

- Pro: no new surface, no added maintenance/audit burden.
- Con: forgoes the large pool of MetaMask-native users the product on-ramp
  targets. **Rejected** given the spike proved the approach cheap and feasible.

## References

- `web/packages/snap-spike/README.md` — Phase-0 spike, measured GO, exact SES
  measurement environment.
- `web/packages/snap/README.md` — Phase-1 MVP: RPC surface, derivation pipeline,
  SRP-vs-Snap-local trade-off, deferred scope.
- Issue #815 — original proposal, phasing, and spike acceptance criteria.
- ADR 0008 — Universal PQ address format (v2), the address the Snap derives.
