# ADR 0012: Solana wBTH Mint Execution — Squads-PDA `invoke_signed`, Assembly-Only

**Status**: Accepted (addendum to [ADR 0002](0002-bridge-custody-scp-validator-federation.md); custody-identity model per [ADR 0010](0010-elected-bridge-multisig.md))
**Date**: 2026-07-20
**Decision Makers**: Core Team
**Related**: [ADR 0002](0002-bridge-custody-scp-validator-federation.md), [ADR 0010](0010-elected-bridge-multisig.md); issues [#1087](https://github.com/botho-project/botho/issues/1087) (implementation), [#1086](https://github.com/botho-project/botho/issues/1086)/[#1052](https://github.com/botho-project/botho/issues/1052) (operator), [#868](https://github.com/botho-project/botho/issues/868) (drill), [#850](https://github.com/botho-project/botho/issues/850) (order-marker replay guard), [#824](https://github.com/botho-project/botho/issues/824) (attestation transport)

## Context

ADR 0002 established that the Solana wBTH mint authority is a validator/federation multisig whose members hold Ed25519 keys. It did **not** pin *which kind* of Solana multisig, nor *how* a multi-party multisig reconciles with the bridge service's single-shot mint contract. Both must be resolved before [#1087](https://github.com/botho-project/botho/issues/1087) (the `invoke_signed` mint-assembly work) can be implemented and verified. This ADR is the addendum ADR 0002's "Implementation → Follow-on sub-decisions" line anticipated. Per the repository convention that accepted ADRs are immutable, it is a new record rather than an edit to 0002.

### The immovable on-chain target

The hardened program (`contracts/solana/programs/wbth/src/lib.rs`, `struct BridgeMint`, #850) constrains the mint:

- `#[account(mut, seeds=[b"bridge"], bump, has_one=mint, has_one=mint_authority)] pub bridge` — the presented `mint_authority` account must byte-equal the stored `bridge.mint_authority`.
- `pub mint_authority: Signer<'info>` — and it must be a **real transaction signer**.
- `order_marker`: `#[account(init, payer = mint_authority, seeds=[b"order", order_id], ...)]` — `mint_authority` is *also the rent payer* for the per-order replay-guard PDA. Whatever account signs as `mint_authority` must therefore be **funded**.
- `bridge_mint(amount: u64, order_id: [u8;32])` — discriminator and borsh arg layout are pinned by `encode_bridge_mint_instruction_data`; the account-meta order is pinned by `build_bridge_mint_instruction` (`bridge/service/src/mint/solana.rs`). This inner instruction is fixed and is what any wrapping flow must carry byte-for-byte.

Consequence: if `bridge.mint_authority` is set to a program-derived address (a Squads **vault PDA**, which is off-curve and cannot sign with a private key), the *only* way to satisfy the `Signer` constraint is a program CPI that `invoke_signed`s on the PDA's behalf — i.e. the multisig program itself must submit the `bridge_mint` call. This is precisely the shape #1087's title names.

### The service-side mismatch

The `Minter` trait (`bridge/service/src/mint/mod.rs`) is single-shot: `prepare_mint` (build + sign one transaction locally) → `broadcast` → `check_confirmation`. It returns one `PreparedMint` and assumes the local relayer key can sign it alone. A Squads mint is inherently multi-transaction, multi-party, and asynchronous (create → approvals by distinct members on distinct machines → execute). The two must be reconciled.

## Problem Statement

1. **Model.** Is the Solana mint authority a **Squads v4 vault PDA** (mint via `invoke_signed` CPI) or an **SPL Token multisig** (mint via one co-signed transaction)?
2. **Trait reconciliation.** How does the chosen model map onto the three-stage `Minter` contract without weakening the exactly-once guarantee?
3. **Test harness.** How is the `invoke_signed` mint path verified, given the localnet validator currently loads no multisig program?

## Decision

### 1. Model — Option A: Squads-v4 vault PDA, assembly-only (`invoke_signed`)

**The Solana `bridge.mint_authority` is a Squads Protocol v4 vault PDA.** The federated mint is assembled as a Squads `vault_transaction` whose wrapped inner instruction is the existing `bridge_mint`, and is executed via the Squads program's `invoke_signed` CPI. The relayer's local Ed25519 key is a Squads **member** (one approver), never the standalone authority.

We reject **Option B (SPL Token multisig, one co-signed transaction)**:

- An SPL Token multisig `mint_authority` requires `t` members to co-sign the *same outer Solana transaction message*. Squads vaults do **not** support raw co-signing of an arbitrary outer message, so B is not the Squads-PDA shape at all — it is a distinct, weaker custody primitive.
- The operator track is already committed to Squads: #1086/#1052 provision a **Squads** devnet multisig, and the startup custody guard (`verify_mint_authority_is_not_local_key`, pinned by `test_guard_passes_with_distinct_squads_pda_authority_2of3` in PR #1088) already treats an off-curve Squads PDA as the authority.
- ADR 0010's "small elected multisig with rotation" maps naturally onto Squads' on-chain membership/threshold management (add/remove member, change-threshold, timelock) — SPL multisig offers none of this.
- SPL multisig caps at 11 signers and has no proposal/governance surface; Squads is the ecosystem-standard for exactly this custody role.

The cost we accept: Option A is a multi-transaction workflow and does touch the mint engine (below). We take that cost deliberately rather than adopt a custody primitive that contradicts the already-chosen operator direction.

**Program version to pin:** Squads Protocol **v4**. Program id `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf` (the canonical mainnet/devnet v4 deployment). This id and the exact instruction discriminators **must be verified against the live on-chain program / the published v4 IDL at implementation time** (see Test Harness) and pinned as constants — do not trust this document as the source of truth for byte layout.

### 2. Trait reconciliation — idempotent state-advancing, no new global order state

Keep the existing three-stage `Minter` contract and the engine's retry loop. Do **not** add an "awaiting quorum" order state to the global mint engine. Instead, make the Solana minter's stages *idempotent and state-advancing* against the on-chain Squads proposal, because exactly-once is already anchored where it belongs — at the inner `bridge_mint`'s `order_marker` PDA (#850), which fails `init` on a duplicate order id no matter how many Squads proposals or executes are attempted.

Mapping:

- **`prepare_mint`** assembles and signs **this node's single contribution**: a transaction that (idempotently) creates the `vault_transaction` + `proposal` for this order if absent, and records this member's `proposal_approve`. Fully signed by the local member key. Returns it as the `PreparedMint`.
- **`broadcast`** submits it. "Already created / already approved" responses are treated as success, exactly as the current Ethereum/Solana idempotent-rebroadcast paths treat `ALREADY_PROCESSED_MARKER`.
- **`check_confirmation`** polls the Squads `proposal` account. Below threshold → `Pending`. At threshold → submit `vault_transaction_execute` (which `invoke_signed`s the inner `bridge_mint`), then report `Confirmed` only once the `BridgeMintEvent` bound to this `order_id` is on-chain at the configured commitment. `execute` is safe to submit from any/all relayer nodes: the first wins and the `order_marker` PDA + Squads' own executed-state flag make the rest no-ops.

This keeps the engine's persistence table (`mints`) and the `MintPending → Completed`/`Reorged` transitions intact; the multi-party wait is absorbed inside `check_confirmation`'s existing "not yet confirmed, keep polling" semantics.

**Required new persistence — the `transaction_index` wrinkle.** Squads derives the `vault_transaction`/`proposal` PDAs from `(multisig, transaction_index)`, where `transaction_index` is a **mutable monotonic counter on the multisig account**, not a function of `order_id`. A naive retry of `prepare_mint` would mint a *new* proposal at a *new* index for the same order, splitting approvals across duplicate proposals. Therefore `prepare_mint` must be order-idempotent via one of:

- **(preferred)** persist an `order_id → squads_transaction_index` mapping in the bridge `mints` table on first creation, and on retry reuse the recorded index (derive the same PDAs, skip create, go straight to approve); or
- read the multisig, enumerate live transaction PDAs, and match the one whose wrapped inner instruction carries this `order_id` before creating.

The preferred mapping is the smaller, deterministic change and is the recommended implementation for #1087.

**Funding.** Because the vault PDA is the inner-`bridge_mint` rent payer for `order_marker`, the operator setup (#1086) must fund the **vault PDA** with enough SOL for order-marker rent; the relayer member key funds only the *outer* Squads transactions' fees. Call this out in the ops runbook.

### 3. Test harness — three tiers

The localnet validator (`contracts/solana/Anchor.toml`) currently clones nothing, so the `invoke_signed` path has no execution coverage. Establish:

- **Tier 1 — pinned byte-layout unit tests (blocking, runs in normal Rust CI; a Builder can do this with no Solana toolchain).** For each Squads instruction the flow emits (`vault_transaction_create`, `proposal_create`, `proposal_approve`, `vault_transaction_execute` — final set per implementation), add discriminator + borsh-arg layout unit tests asserting against known vectors captured from the pinned Squads v4 IDL, mirroring `test_bridge_mint_instruction_data_layout` / `test_anchor_discriminator_known_vector`. Add a test asserting the wrapped inner instruction is **byte-identical** to today's `build_bridge_mint_instruction` output. These tests are what make #1087 mergeable — a silent Squads-encoding drift breaks CI, not a mainnet mint. All PDA derivations (multisig, vault, transaction, proposal) get derivation unit tests with pinned expected addresses.
- **Tier 2 — localnet e2e (`solana-contracts-ci.yml`).** Load the Squads v4 program into `solana-test-validator` **by committed `.so` at the pinned program id** via Anchor.toml `[[test.genesis]]` (preferred over `[[test.validator.clone]]`, which needs live-cluster network access and makes CI flaky). Commit the `.so` under `contracts/solana/` pinned to the audited v4 release, documenting its provenance/hash. Extend the ts-mocha suite with: create multisig → set vault PDA as `bridge.mint_authority` → assemble a Squads `vault_transaction` wrapping `bridge_mint` → collect `t` approvals → `execute` → assert the wBTH balance and that a **replay** (second execute / duplicate order id) fails at the `order_marker` `init`.
- **Tier 3 — devnet drill (operator-gated, #1086/#868).** Real Squads v4 program on devnet, real multisig, real threshold. Out of scope for automated CI; covered by the operator federation drill.

Tier 1 is the merge gate for #1087. Tiers 2–3 deepen assurance and are sequenced with the operator work.

## Consequences

### Positive

1. Custody uses the ecosystem-standard Squads v4 governance surface (membership, threshold, timelocks, rotation) — aligned with ADR 0010's elected-multisig-with-rotation model.
2. Exactly-once is unchanged: it stays anchored at the inner `order_marker` PDA, so the multi-party Squads workflow cannot double-mint regardless of retries or execute races.
3. The `Minter` trait and mint engine are untouched — the multi-party wait lives inside `check_confirmation`, and only the Solana minter gains an `order_id → tx_index` persistence hook.
4. The inner `bridge_mint` instruction and program are unchanged (byte-identical, reused).

### Negative

1. Squads instruction encoding + PDA derivations are net-new hand-rolled code (kept in the lightweight raw-JSON-RPC `solana_rpc` style, no `solana-sdk`), pinned only by Tier-1 vectors — encoding drift risk is real and is why the byte-layout tests are the merge gate.
2. A new persisted `order_id → squads_transaction_index` mapping is required for order-idempotent proposal creation; forgetting it splits approvals across duplicate proposals.
3. The vault PDA must be funded for order-marker rent (new operator responsibility).
4. A committed Squads `.so` must be tracked and refreshed if the pinned v4 release changes.

### Neutral

1. The single-key path (`prepare_mint` as-is) remains valid for a distinct off-curve *single-key* authority; Option A adds the multisig-PDA path alongside it rather than replacing it.
2. The choice binds the bridge to Squads v4 specifically; a future migration to another multisig program is a fresh sub-decision.

## Alternatives Considered

- **Option B — SPL Token multisig, one co-signed transaction.** Rejected: not the Squads-PDA/`invoke_signed` shape, contradicts the committed operator direction (#1086/#1052), 11-signer cap, and no governance/rotation surface. See Decision §1.
- **Add an explicit "awaiting quorum" order state to the mint engine.** Rejected as the default: larger blast radius on the shared engine/state machine for no exactly-once benefit, since `order_marker` already guarantees it. Reconsider only if absorbing the wait inside `check_confirmation` proves operationally opaque (e.g. observability demands a first-class quorum-pending status).
- **Clone Squads from source into the Anchor workspace.** Rejected for the harness: Squads v4 is a large program with its own toolchain expectations that risk conflicting with the load-bearing Solana 1.18.26 / Anchor 0.29 / lockfile-v3 pins in `solana-contracts-ci.yml`. Loading a pinned `.so` via `[[test.genesis]]` gives the same coverage without that risk.

## Implementation

Unblocks [#1087](https://github.com/botho-project/botho/issues/1087). Grounding: `bridge/service/src/mint/solana.rs` (`prepare_mint`, `build_bridge_mint_instruction`, `encode_bridge_mint_instruction_data`), `bridge/service/src/solana_rpc.rs` (Pubkey/AccountMeta/Instruction/LegacyMessage primitives), `contracts/solana/programs/wbth/src/lib.rs` (`struct BridgeMint`), `contracts/solana/Anchor.toml`, `.github/workflows/solana-contracts-ci.yml`. Operator prerequisites: #1086/#1052 (create Squads multisig, rotate authority, fund vault PDA). Verify against the deployed Squads v4 program / published v4 IDL before pinning any discriminator or program id.

## References

- [ADR 0002](0002-bridge-custody-scp-validator-federation.md) (federation custody), [ADR 0010](0010-elected-bridge-multisig.md) (elected multisig)
- Issues #1087, #1086, #1052, #868, #850, #824
- Squads Protocol v4 (verify program id / IDL against the live deployment)
