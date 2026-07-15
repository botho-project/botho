# Line-level audit comments — botho-bridge-spec.1

Keyed to `botho-bridge-spec.tex`.

- **§11.1 line 49-50 (wBTH picocredits / 12 decimals)** — verified against `WrappedBTH.sol:90` and the Solana program. The `% anvil-const: name=wbth_decimals value=12` marker is the sole authoritative statement; no drift. (match)

- **§11.2 line 93 (`release/bth.rs::validate_release_attestation`)** — the cited function exists and enforces the distinct-signer dedup and threshold-floor check exactly as claimed. Good, precise code reference. (match)

- **§11.2 line 118 (`bridge/core/src/attestation.rs`)** — the domain-tag table is verbatim-accurate against the module's `ATTEST_DOMAIN_*` / `*_ATTESTATION_DOMAIN_TAG*` constants. The claim "proven distinct from the node's operator-action domain" is backed by the module's mirroring of `botho/src/operator_action.rs`. (match)

- **§11.2 line 159-167 (three-Safe structure + Solana startup custody guard)** — every role assignment matches `WrappedBTH.sol`; the "startup custody guard refuses to operate if the on-chain mint authority equals the local relayer key" claim matches `mint/solana.rs::is_single_key_authority` + the ADR-0002 startup guard. (match)

- **§11.3 line 199-216 (factor-1-only wrapping + factor-1-only release)** — matches `bth_scan.rs` (deposit `factor_one` eligibility) and `release/bth.rs:333-341` (non-factor-1 reserve outputs are never spent nor used as decoys). Strong, verifiable claims. (match)

- **§11.3 line 228-240 (demurrage-settlement on-ramp, target-state)** — correctly marked "(target-state)" and cross-referenced to the register (#831). No consensus settlement op exists in code; the marking is accurate. (registered gap — suppressed)

- **§11.5 lines 306-391 (import cluster tagging)** — the ENTIRE subsection is present-tense normative prose describing a mechanism that does NOT exist in the production path. This is the load-bearing case. It is correctly handled: (a) the two subsection headings carry "(target-state)"; (b) the fig2 caption at line 388-389 states "Target-state: the current implementation releases a factor-1 output (register row IMP-1)"; (c) the §11.6 register carries the exact Live/Target/Tracking row; (d) the §11.5 Divergence note (lines 501-509) quotes `bth_scan.rs` and `bridge_import_sweep.rs` by path. Registered intentional gap, suppressed — not a critical flag. Constants K=17,280 (line 330) and F=1.5× (line 331) match ADR 0007 and the calibration sim. (registered gap — suppressed)

- **§11.5 line 356 ("a 2M-BTH whale needs 541 separate epochs (541 days at K)")** — matches ADR 0007 §Calibration verbatim and the sim's `epochs_to_reach_floor`. (match)

- **§11.5 line 370-371 ("≈9 domestic-mixing spends")** — matches ADR 0007 §Calibration ("≈9 … most of the drop in the first 4"). (match)

- **§11.6 line 480-484 (Solana transports register row)** — the Live description ("code-complete against a raw JSON-RPC client and mock-tested; live-node integration is `#[ignore]`d and unverified end-to-end") is precisely accurate against `mint/solana.rs`, `solana_rpc.rs`, `watchers/solana.rs` (#857) and the `#[ignore]`d `solana_devnet_tests`. The stale BRIEF/ADR-era "stubbed" framing is correctly ABSENT from the section. (match — no spec-wrong)

- **§11.7 line 415-423 (proof-of-reserves) + line 425-432 (fail-closed liveness trade)** — both match `reserve.rs` (exact peg, custody leg, unverified-chain fail-safe, breaker trip) and the two-layer cap design (`engine.rs` backlog cap + on-chain auto-pause). (match)

- **Global (nit, non-blocking)** — the register row for import tagging is labeled "IMP-1" in the fig2 caption but the register table itself is unlabeled (rows keyed by Component name). Minor: a stable row id in the table would make the fig2 cross-reference exact. Not a defect. (nit)
