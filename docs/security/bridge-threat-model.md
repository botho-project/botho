# BTH Bridge Threat Model & Adversarial Test Map

_Part of bridge epic #816 (Phase 3, issue #829). Companion to
[`docs/bridge/security.md`](../bridge/security.md), the ADRs
[0002](../decisions/0002-bridge-custody-scp-validator-federation.md)–[0005](../decisions/0005-bridge-v1-chain-scope-ethereum-and-solana.md),
and [`docs/security/threat-model.md`](./threat-model.md) (node/consensus)._

Bridges are the most-attacked component in crypto: almost every large loss has
come from a broken mint authorization, a replayed message, a reorg
double-count, or a peg that silently drifted. This document enumerates the
**named threats** against the BTH ↔ wBTH bridge and maps each to the specific
test(s) that defend against it, so the adversarial suite is auditable against a
concrete list rather than a vibe.

The bridge's security rests on five load-bearing invariants:

1. **Exactly-once mint** — one confirmed BTH deposit mints wBTH at most once.
2. **Exactly-once release** — one confirmed wBTH burn releases reserve BTH at
   most once.
3. **Threshold authorization** — no privileged action (mint/release) happens
   without a `t`-of-`n` federation signature set, domain-separated and
   order-bound (ADR 0002).
4. **Peg solvency** — `Σ(wBTH outstanding) == locked BTH reserve` within
   tolerance at all times, and any drift trips the circuit breaker (ADR 0003).
5. **Finality safety** — an action only fires against a block that is final on
   its chain (SCP finality on BTH; `confirmations_required` depth + canonical
   re-check on Ethereum; `Finalized` commitment on Solana).

## Test surfaces

| Surface | Location |
| --- | --- |
| Attestation crypto (envelopes, domains, thresholding) | `bridge/core/src/attestation.rs` |
| Cross-domain / equivocation adversarial | `bridge/core/src/adversarial_tests.rs` |
| Attestation ingest pipeline (service) | `bridge/service/src/attestation.rs` |
| Peg reconciliation + drift/breaker | `bridge/service/src/reserve.rs` |
| Peg-invariant + reorg property test | `bridge/service/src/adversarial_tests.rs` |
| Ethereum watcher reorg/finality | `bridge/service/src/watchers/ethereum.rs` |
| Reorg + finality fuzz (property test) | `bridge/service/src/watchers/adversarial_tests.rs` |
| Order-engine chaos / restart / load | `bridge/service/src/chaos_tests.rs` |
| Ledger exactly-once accounting | `bridge/service/src/db.rs` |
| wBTH contract (supply + rate-limit) | `contracts/ethereum/test/WrappedBTH.test.ts` |

## Threat → test map

Each row is a named attack, the invariant it targets, and the test(s) that
demonstrate the defense. "Pre-existing" = landed in an earlier Phase-3 wave
(#841/#844/#847/#851/#854); "new (#829)" = added by this issue.

### 1. Double-mint (replayed / duplicated deposit → mint)

| Vector | Test | Wave |
| --- | --- | --- |
| Replayed attestation envelope (same nonce) | `attestation::pipeline_accepts_valid_release_attestation_then_rejects_replay` | #847 |
| Nonce replay survives a service restart | `nonce::replay_protection_survives_a_restart_within_the_window` | #847 |
| Duplicate deposit tx → one order | `db::test_mint_idempotency_duplicate_order_id_exactly_once`, `db::test_processed_deposits` | #854 |
| On-chain duplicate `orderId` rejected | `WrappedBTH.test.ts › order-id replay guard` (both cases) | #851 |
| Crash at any mint transition → one mint | `chaos_tests::chaos_mint_crash_at_every_transition_boundary` | #854 |
| Concurrent orders each mint once | `chaos_tests::load_test_concurrent_orders_exactly_once` | #854 |

### 2. Double-release (replayed / duplicated burn → release)

| Vector | Test | Wave |
| --- | --- | --- |
| Replayed burn → single release | `engine::test_release_replayed_burn_single_release` | #854 |
| Release claim is exactly-once | `db::test_release_claim_exactly_once` | #854 |
| Crash at any release transition → one landed tx | `chaos_tests::chaos_release_crash_at_every_transition_boundary` | #854 |
| Resume after crash reuses the recorded tx | `engine::test_release_resume_after_crash_reuses_recorded_tx` | #854 |
| Crash after claim before sign is safe | `engine::test_release_crash_after_claim_before_sign_is_safe` | #854 |

### 3. Attestation replay across domains

| Vector | Test | Wave |
| --- | --- | --- |
| All bridge domain tags pairwise-distinct + differ from operator-action | `adversarial_tests::all_attestation_domain_tags_are_pairwise_distinct_and_differ_from_operator_action` | new (#829) |
| A mint sig for chain C cannot authorize chain D | `attestation::test_mint_attestation_does_not_bind_to_other_chain_order`, `adversarial_tests::a_release_attestation_never_verifies_under_a_mint_target_domain` | #847 / new |
| Expired / stale envelope rejected | `attestation::pipeline_rejects_expired_attestation`, `attestation::test_freshness_window` | #847 |

### 4. Cross-domain signature confusion

The keystone check the issue calls out: a validator's Ed25519 node key also
signs **operator actions** (`botho/src/operator_action.rs`,
`DOMAIN_SEPARATOR = "botho-operator-action-v1"`) and potentially wallet
payloads. Those signatures must never authorize a bridge action. Verified by
construction — every bridge domain tag is proven distinct from and non-prefixing
of the operator-action tag — and by forged-signature tests.

| Vector | Test | Wave |
| --- | --- | --- |
| Operator-action-domain signature reused as a bridge attestation | `adversarial_tests::operator_action_domain_signature_reused_as_bridge_attestation_is_rejected` | new (#829) |
| Domainless (wallet-style) signature reused as an attestation | `adversarial_tests::wallet_style_raw_signature_reused_as_bridge_attestation_is_rejected` | new (#829) |
| Payload signature reused in the envelope-signature slot | `adversarial_tests::release_payload_signature_reused_as_the_envelope_signature_is_rejected` | new (#829) |
| Wrong-chain envelope domain fails | `attestation::test_cross_domain_signature_reuse_fails` | #847 |
| Ethereum secp256k1 payload never accepted on the Ed25519 path | `attestation::test_ethereum_kind_rejected_on_ed25519_path` | #847 |

**Domain tags in use** (all proven distinct):
`botho-bridge-attest-{eth,sol,bth}-v1` (envelope),
`botho-bridge-release-v1` (release payload),
`botho-bridge-mint-{eth,sol}-v1` (mint payload) — versus the node's
`botho-operator-action-v1`.

### 5. Below-threshold authorization

| Vector | Test | Wave |
| --- | --- | --- |
| `t`-of-`n` never satisfied below `t` distinct signers | `attestation::test_meets_threshold`, `release::bth::test_below_threshold_rejected` | #847 |
| Threshold 0 never authorizes | `adversarial_tests::a_zero_threshold_never_authorizes_even_with_signatures`, `attestation::from_config_rejects_zero_or_unsatisfiable_threshold` | new / #847 |
| Below-threshold release/mint is fail-safe (no partial action) | `attestation::authorize_release_meets_threshold_and_output_verifies_downstream`, `attestation::authorize_mint_eth_collects_safe_owner_signatures_to_threshold` | #847 |
| On-chain Safe assembly rejects below-threshold | `mint::ethereum::test_safe_signature_assembly_rejects_below_threshold` | #847 |

### 6. Single malicious signer moves funds

| Vector | Test | Wave |
| --- | --- | --- |
| Unknown/byzantine signer not counted | `attestation::pipeline_rejects_unknown_signer_without_counting_it`, `attestation::pipeline_rejects_eth_envelope_from_a_non_owner` | #847 |
| Non-federation signer rejected (release) | `release::bth::test_non_federation_signer_rejected` | #847 |
| One signer cannot reach a `≥2` threshold alone | `adversarial_tests::equivocating_signer_cannot_inflate_the_distinct_signer_count` | new (#829) |

### 7. Equivocation (a signer attests conflicting payloads for one order)

| Vector | Test | Wave |
| --- | --- | --- |
| Same signer, two submissions → counts once (no threshold inflation) | `adversarial_tests::equivocating_signer_cannot_inflate_the_distinct_signer_count`, `attestation::pipeline_same_signer_with_fresh_nonces_counts_once_toward_threshold` | new / #847 |
| Conflicting amount/recipient cannot pass order binding | `adversarial_tests::conflicting_payload_for_the_same_order_cannot_cross_order_binding`, `attestation::test_order_binding_rejects_field_mismatches` | new / #847 |

**Residual / follow-up.** Equivocation is *neutralized* (order binding pins
every field to the on-record order, and the aggregation set deduplicates by
signer identity, so a single equivocating signer can neither move conflicting
funds nor inflate the threshold), but it is **not actively detected**: the
service does not raise an auditable "equivocation" alarm when a signer presents
conflicting bytes for the same order id — it silently keeps the first valid
submission and rejects the rest via the normal replay/binding paths. Active
detection + alerting is filed as a hardening follow-up (see "Follow-ups").

### 8. Reorg double-count (add / orphan / re-add on the source chain)

| Vector | Test | Wave |
| --- | --- | --- |
| Reorg re-add processed exactly once | `watchers::ethereum::test_reorg_readd_processed_exactly_once` | #841 |
| Orphaned burn never confirms | `watchers::ethereum::test_orphaned_burn_never_confirms` | #841 |
| Cursor rewind / replay is a no-op | `watchers::ethereum::test_cursor_rewind_replay_is_noop`, `watchers::bth::test_cursor_rewind_replay_is_deduplicated` | #841 |
| Solana only acts on `Finalized` commitment | `watchers::solana::test_only_finalized_commitment_is_a_reorg_guard` | #841 |
| **Randomized** reorg depth / re-add → exactly-once confirmation | `watchers::adversarial_tests::prop_reorg_finality_fuzz_is_exactly_once` | new (#829) |
| Engine reorg unwind + resubmit keeps one order id | `engine::test_reorg_unwinds_and_resubmits_same_order_id` | #854 |

### 9. Peg break (unbacked mint / missing supply / custody theft)

| Vector | Test | Wave |
| --- | --- | --- |
| Property: `locked == Σ supply` across random mint/burn | `reserve::prop_invariant_holds_across_mint_burn_sequences` | #844 |
| **Property: invariant survives reorg-orphan interleaving** | `adversarial_tests::prop_invariant_survives_reorg_interleaving` | new (#829) |
| Unbacked supply (unauthorized mint) trips alert | `reserve::test_drift_injection_unbacked_supply_trips_alert` | #844 |
| Missing supply trips alert | `reserve::test_drift_injection_missing_supply_trips_alert` | #844 |
| Custody theft (reserve moved w/o burn) flips peg state | `reserve::test_unauthorized_reserve_movement_trips_custody_alert` | #844 |
| Contract supply invariant under random ops | `WrappedBTH.test.ts › supply accounting invariant (randomized)` | #851 |

### 10. Breaker bypass (drift/anomaly should halt, not merely log)

| Vector | Test | Wave |
| --- | --- | --- |
| Drift alert trips the circuit breaker (fail-closed) | `reserve::test_drift_alert_trips_circuit_breaker` | #844 |
| Global/backlog caps trip the breaker | `engine::test_global_cap_trips_breaker`, `engine::test_breaker_auto_trips_on_backlog` | #854 |
| Kill-switch halts submits, lets confirms settle | `engine::test_kill_switch_halts_submits_but_confirms_settle` | #854 |
| On-chain auto-pause at anomalous cumulative volume | `WrappedBTH.test.ts › auto-pause circuit breaker` (3 cases) | #851 |
| **Randomized rate-limit accounting + breaker** | `WrappedBTH.test.ts › rate-limit accounting fuzz (randomized, breaker armed)` | new (#829) |

## Boundary of the model (accepted assumptions)

- **Reorgs deeper than `confirmations_required`.** A burn confirms only at
  `confirmations_required` depth against a canonical block; a reorg *deeper*
  than that could orphan a confirmed burn. This is outside the safety model by
  construction — `confirmations_required` **is** the assumption that such
  reorgs do not occur — so the reorg fuzz bounds its depths strictly below the
  confirmation window. Choosing `confirmations_required` larger than any
  plausible reorg is an operational parameter, not a code defect.
- **Signature transport between federation members** (#828) is out of scope
  here; the tests inject peer envelopes directly. Everything cryptographic
  (signing, verification, replay, binding, thresholding) is exercised.
- **Solana / BTH live supply + reserve-balance transports** (#853) are
  fail-safe stubs: an unverified chain is reported `verified: false` and
  excluded from drift math — never silently counted as healthy
  (`reserve::test_unverified_chain_is_flagged_not_alerted`).

## Follow-ups filed

- **Active equivocation detection & alerting** (#859) — raise an auditable
  alarm when a federation member submits conflicting attestations for the same
  order id (today: neutralized but silent). See threat #7.
