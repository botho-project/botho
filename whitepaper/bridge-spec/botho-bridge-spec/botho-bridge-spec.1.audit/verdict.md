# Audit verdict — botho-bridge-spec.1

**Critic**: audit (factual + spec↔implementation consistency)
**Rubric**: anvil-spec-v1 (/44, advance ≥39) — the auditor does NOT score the rubric; that is spec-review.
**code_ref tier**: ACTIVE and resolved. Scalar glob `/Users/rwalters/GitHub/botho/bridge/**/*.rs` (35 files) plus the manually-consulted counterparty contracts and cluster-tax simulation named in the BRIEF note.

## audit_clean: TRUE

**Critical flags (`implementation_contradicts_spec`): none.**
**Major findings: 1** (advisory — code_ref scalar-glob does not reach the counterparty contracts / cluster-tax simulation the section normatively describes; see FRICTION and findings.md M-1). This is not a blocking flag; the auditor read those files manually per the BRIEF instruction and the sweep completed.

The three drafter-flagged ADR↔code focus areas all resolved cleanly:

1. **Import cluster tagging — intentional-gap, REGISTERED (suppressed).** ADR 0007 (Accepted, ratified 2026-07-14, K=17,280, F=1.5×) says an unwrap tags the output 100% to `c_import(m)` at `import_factor(m) ≥ F`. The production release path `bridge/service/src/bth_scan.rs::build_release_tx` emits the recipient output with EMPTY cluster tags (factor-1) and `debug_assert!(recipient_output.cluster_tags.is_empty())` at line 218. The import-factor mechanism (`c_import`, `import_factor`) exists ONLY in `cluster-tax/src/simulation/bridge_import_sweep.rs` — a grep of the entire `*.rs` tree confirms no other file references it. This is a genuine code-vs-spec divergence, BUT it is correctly registered: the `## Implementation status` register carries a `target-state` row (§11 "Import cluster tagging (ADR 0007)") whose **Live** = "Unwrap releases a factor-1 / background output (`bth_scan.rs` recipient output carries empty cluster tags)" matches the code exactly, whose **Target** = "Unwrap tags the output 100% to c_import(m) at import_factor(m)≥F" matches the spec claim exactly, and whose **Tracking** = botho#938. Every present-tense sentence in §11.5 (Import Cluster Tagging) is additionally scoped by the "(target-state)" subsection headings, the fig2 caption ("*Target-state*: the current implementation releases a factor-1 output (register row IMP-1)"), and the explicit §11.5 Divergence note quoting `bth_scan.rs`. Per the register cross-check this is a **registered intentional gap → suppressed, NOT a critical flag, NOT an escalation.** This was the load-bearing case and its disposition is correct.

2. **Solana "stubbed" wording — NOT PRESENT; section characterization matches code.** The BRIEF/ADR-era framing called Solana transports "stubbed (#856/#857/#858/#853)". The SECTION does not repeat that stale framing. It states Solana is "defined for both chains", that "the validator federation signs Solana authorizations natively", and — in the register — that the Solana transports are "Mint assembly / sign / submit / confirm and burn watcher are code-complete against a raw JSON-RPC client and mock-tested; live-node integration is `#[ignore]`d and unverified end-to-end", marked target-state on the audit/integration axis (botho#853/#856/#857/#858). This matches the code exactly: `mint/solana.rs`, `solana_rpc.rs`, `watchers/solana.rs` are LIVE-WIRED (#857) — recent-blockhash fetch, `bridge_mint` assembly, Ed25519 signing, `sendTransaction` idempotent re-broadcast, `getSignatureStatuses` polling, the startup lone-hot-key custody guard (`is_single_key_authority`), and the `Finalized`-only burn watcher are all implemented. There is no surviving "stubbed" claim to be spec-wrong. **Disposition: not a contradiction — match.**

3. **`release/bth.rs` (#856) — live/target characterization matches the module's actual state.** The section says the release path is live: "`release/bth.rs::validate_release_attestation`" verifies the distinct-signer threshold (§11.2), releases pay a fresh one-time stealth output, spend only factor-1 reserve outputs, change back to the reserve. The module docs confirm "The RPC-dependent stages are now LIVE (#856)": `prepare_release` (attestation gate → factor-1 reserve scan → CLSAG-signed fresh-stealth tx), `broadcast` (idempotent re-broadcast), `check_confirmation` (depth poll; 0 = SCP externalization). Attestation verification, config validation, and the exactly-once engine machinery (`release_claims` record-before-broadcast, unique index on `order_id_hash`) are fully live and tested. Absent RPC/keys the stages fail safe (`NotImplemented`, order stays `BurnConfirmed`) — which the section does NOT claim is live, so no over-statement. **Disposition: match.**

## Asymmetry rule

Not invoked. No contradiction reached the uncertainty branch: the one real code-vs-spec divergence (import tagging) is register-suppressed with an exact Live/Target/Tracking match, and every other checked claim matched the implementation. No claim required defaulting to `code-wrong`, and no `spec-wrong` was asserted (none was needed — the spec is truthful against the code as-built plus its registered target-state gaps).

## Top audit priorities

1. (major, non-blocking) **M-1** — `code_ref` is a scalar glob over `bridge/**/*.rs` only; it does not resolve the counterparty Solidity/Anchor contracts (`contracts/ethereum/contracts/WrappedBTH.sol`, `contracts/solana/programs/wbth/src/lib.rs`) or the cluster-tax import-factor simulation (`cluster-tax/src/simulation/bridge_import_sweep.rs`) that the section normatively describes. The auditor read these manually per the BRIEF note and every claim checked out, so the sweep is complete — but a future automated re-audit that trusts only the resolved `code_ref` would silently skip the wBTH-contract constants (12 decimals, auto-pause, three-Safe roles), the Solana program parity, and the entire import-factor calibration. Recommend the operator track anvil#718 (scalar-only `code_ref` cannot express the multi-tree implementation this spec spans). No spec change is required.

No `code-wrong` escalations. No `unregistered` intentional gaps. No `spec-wrong` findings routing to revise.

## Constant sweep summary (full detail in findings.md)

All five marked constants + the ADR-derived quantities verified against the implementation:

| Constant | Spec | Implementation | Verdict |
|---|---|---|---|
| wBTH decimals | 12 | `WrappedBTH.sol:90 DECIMALS=12`; Solana `mint::decimals=12`; reserve source comments | match |
| Ring size (CLSAG) | 20 | `transaction/clsag/src/lib.rs:571 DEFAULT_RING_SIZE=20` | match |
| Threshold floor | t ≥ t_SCP (symbolic) | `release/bth.rs` `threshold_floor` param; `config.rs` release/mint_threshold; ADR 0002. No numeric t pinned in code — symbolic, correctly stated symbolically. | match |
| Import epoch K | 17,280 blocks (1 day @ 5s) | ADR 0007 (ratified); `bridge_import_sweep.rs:627 RECOMMENDED_EPOCH_BLOCKS=17280`; 17280×5s=86400s ✓. Target-state (register IMP row). | match (target-state) |
| Import floor F | 1.5× | ADR 0007 (ratified); `bridge_import_sweep.rs:624 RECOMMENDED_FLOOR=1500` (FACTOR_SCALE units). Target-state. | match (target-state) |
| Factor range | [1×, 6×] | ClusterFactorCurve saturating 6×, W_MID=100k; ADR 0007 | match |
| Split-game figure | "2M-BTH whale needs 541 epochs (541 days at K)" | ADR 0007 §Calibration verbatim ("541 separate epochs … 541 days at 1-day epochs"); reproduced by the sim | match |
| Decay figure | "≈9 domestic-mixing spends" | ADR 0007 §Calibration ("≈9 domestic-mixing spends (most of the drop in the first 4)") | match |

No same-named-constant drift detected across sections. Domain tags, exactly-once invariants, three-Safe structure, factor-1-only reserve spend, proof-of-reserves, and auto-pause all verified as live (see findings.md).
