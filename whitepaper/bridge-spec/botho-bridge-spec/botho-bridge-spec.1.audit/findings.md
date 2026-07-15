# Audit findings — botho-bridge-spec.1

Per-claim table. `Kind`: factual (internal logic) | implementation-consistency.
`Verified?`: match | contradicts | unresolvable. `Disposition`: spec-wrong | code-wrong | intentional-gap | — (n/a).

## Constants (implementation-consistency)

| # | Claim | Kind | Verified? | Disposition | Evidence |
|---|---|---|---|---|---|
| C1 | wBTH is 12 decimals; 1 base unit = 1 picocredit = 1 unit native BTH (no scaling) | impl-consistency | match | — | `WrappedBTH.sol:90` `DECIMALS = 12` + `:174 decimals()->12`; Solana `programs/wbth/src/lib.rs` "`mint::decimals = 12`"; `reserve.rs:131` "12 decimals … 1:1 with picocredits" |
| C2 | CLSAG ring size = 20 | impl-consistency | match | — | `transaction/clsag/src/lib.rs:571` `pub const DEFAULT_RING_SIZE: usize = 20;`; consumed by `bth_scan.rs` + `release/bth.rs:462` |
| C3 | Threshold floor `t >= t_SCP` (symbolic) | impl-consistency | match | — | `release/bth.rs::validate_release_attestation(threshold_floor)`; `BthReleaser::new` rejects threshold>signers and threshold-0-with-signers (#842); `config.rs:201/257/291`; ADR 0002. No numeric t in code; spec correctly symbolic. |
| C4 | Import epoch `K = 17,280` blocks (1 day) | impl-consistency | match (target-state) | intentional-gap (registered) | ADR 0007 ratified; `bridge_import_sweep.rs:627` `RECOMMENDED_EPOCH_BLOCKS = 17_280`; 17280*5s=86400s ✓. Register IMP row / #938. |
| C5 | Import-factor floor `F = 1.5x` | impl-consistency | match (target-state) | intentional-gap (registered) | ADR 0007 ratified; `bridge_import_sweep.rs:624` `RECOMMENDED_FLOOR = 1500` (FACTOR_SCALE=1000). Register IMP row / #938. |
| C6 | Factor range `[1x,6x]`; log-sigmoid curve, W_mid=100k, saturating 6x | impl-consistency | match | — | ADR 0007 §Decision.2; `bridge_import_sweep.rs` uses `ClusterFactorCurve::default_params()` + 6x saturation |
| C7 | Demurrage: charge = value*rate*(factor-1)/(max_factor-1)*elapsed/blocks_per_year | factual | match | — | `cluster-tax/src/demurrage.rs::demurrage_charge`; used at `bridge_import_sweep.rs:375`. (factor-1) => factor-1 pays 0. Dimensionally sound. |
| C8 | "2M-BTH whale needs 541 epochs (541 days at K)" | factual | match | — | ADR 0007 §Calibration verbatim; sim `epochs_to_reach_floor` (K-independent) |
| C9 | "≈9 domestic-mixing spends" to blend 6x flood to floor | factual | match | — | ADR 0007 §Calibration; sim `decay_by_circulation` + `decay_to_floor_spends` (real `TagVector::mix`) |

## Protocol / mechanism claims (implementation-consistency)

| # | Claim | Kind | Verified? | Disposition | Evidence |
|---|---|---|---|---|---|
| P1 | Peg: Sum(wBTH ETH)+Sum(wBTH SOL) = locked BTH reserve | impl-consistency | match | — | `reserve.rs` docs + `reconcile_once`; property test `prop_invariant_holds_across_mint_burn_sequences` |
| P2 | Exact peg, tolerance default 0 (factor-1 pays zero demurrage forever) | impl-consistency | match | — | `reserve.rs:48` "default 0 — the ADR 0003 exact peg"; `test_reconcile_healthy_exact_peg`, 1-picocredit drift alerts |
| P3 | Domain tags: attest-{eth,sol,bth}-v1; release-v1; mint-{eth,sol}-v1; pairwise-distinct + distinct from operator-action | impl-consistency | match | — | `core/src/attestation.rs:278/282/286/131/230/239` all verbatim; `attestation_domain()`; mirrors `operator_action.rs` |
| P4 | Threshold auth; distinct-signer exact (equivocator counts once) — in `release/bth.rs::validate_release_attestation` | impl-consistency | match | — | `release/bth.rs:185` dedup; `meets_threshold` sort+dedup; `test_below_threshold_rejected` (same signer twice fails t=2) |
| P5 | Equivocating signer flagged by dedicated audit event | impl-consistency | match | — | `AttestationOutcome.equivocation`/`with_equivocation`; `attestation_equivocation` audit |
| P6 | Exactly-once mint: 32-byte orderId; contract records + reverts on duplicate | impl-consistency | match | — | `WrappedBTH.sol:117/198` `processedOrders`; Solana order-marker PDA fails at `init` on duplicate |
| P7 | Exactly-once release: record signed tx before broadcast; re-broadcast reuses recorded tx | impl-consistency | match | — | `engine.rs:1082` `record_release_tx`; `db.rs:298/308` `release_claims` + UNIQUE INDEX on `order_id_hash`; `release/bth.rs::broadcast` idempotent |
| P8 | Schemes: BTH release + Solana mint = Ed25519; Ethereum mint = secp256k1 via Gnosis Safe MINTER_ROLE | impl-consistency | match | — | `attestation.rs` SignatureScheme; `ed25519_payload_digest` errors for Ethereum; `WrappedBTH.sol:86 MINTER_ROLE`; `mint/ethereum.rs` SafeTx path |
| P9 | Three-Safe split (MINTER/ADMIN/PAUSER distinct Safes; deployer no roles; t-of-n in Safe not token) | impl-consistency | match | — | `WrappedBTH.sol:165-167`, ":46 deployer NO roles", ":37-38 threshold in Safe"; Solana three multisigs |
| P10 | Relayer EOA submits Safe.execTransaction w/ threshold sigs over EIP-712 SafeTx wrapping bridgeMint | impl-consistency | match | — | `mint/ethereum.rs:38-78` IGnosisSafe.execTransaction + SafeTx + `safe_tx_hash` |
| P11 | Solana authority = SPL/Squads multisig; startup guard refuses if on-chain authority == local relayer key | impl-consistency | match | — | `mint/solana.rs:218-225` `is_single_key_authority`; `:317-387` startup custody guard |
| P12 | Factor-1-only wrapping; non-factor-1 deposit rejected w/ audit event, never mints | impl-consistency | match | — | `bth_scan.rs:126` `factor_one = explicit_cluster_weight()==0`; `scan_flags_non_factor1_output` |
| P13 | Releases spend ONLY factor-1 reserve outputs; non-factor-1 never spent nor decoy; change zero-demurrage to reserve | impl-consistency | match | — | `release/bth.rs:333-341`; `bth_scan.rs::build_release_tx` change to reserve default subaddress |
| P14 | Unwrap re-shields: fresh one-time stealth, never reused; two releases to same addr => distinct keys | impl-consistency | match | — | `bth_scan.rs:217`; test asserts distinct target_keys (ADR 0004) |
| P15 | Lock reveals amount (leaks amount not source ring); wBTH public | factual | match | — | ADR 0004; transparent-amount model in `bth_scan.rs`; consistent w/ §11.4 |
| P16 | Proof-of-reserves: both drift bounds; drift/shortfall trips fail-closed breaker+alert; unverified chain excluded, never healthy | impl-consistency | match | — | `reserve.rs::reconcile_once`; `unverified_status`; `test_unauthorized_reserve_movement_trips_custody_alert`; `test_drift_alert_trips_circuit_breaker` |
| P17 | Finality: SCP on BTH; depth+canonical on ETH; Finalized on Solana | impl-consistency | match | — | `release/bth.rs::check_confirmation` (0=SCP); `watchers/ethereum.rs`; `watchers/solana.rs` "only Finalized (rooted)" |
| P18 | Auto-pause on-chain breaker + two-layer (tight service cap first, looser on-chain last-resort) | impl-consistency | match | — | `WrappedBTH.sol:220` `_pause()`; Solana `auto_pause_threshold`; `engine.rs:581` backlog cap + reserve breaker + kill switch |
| P19 | Open burn removed; only bridgeBurn emits BridgeBurn | factual | match | — | `WrappedBTH.sol:26-29`, `:234` sole burn path |
| P20 | Envelope single-use/freshness (nonce, expiry, skew, max lifetime); parse-after-verify; dup-key reject; v1 pinned | factual | match | — | `attestation.rs` VERSION=1, MAX_LIFETIME=300, SKEW=30, `verify_and_parse_ed25519`, `reject_duplicate_keys` |

## Import-tagging present-tense claims (the load-bearing register case)

| # | Claim | Kind | Verified? | Disposition | Evidence |
|---|---|---|---|---|---|
| I1 | Unwrap mints into epoch import cluster c_import(m)=H("bridge-import"||m), m=floor(h/K), import_factor(m)=max(F,Curve(sum epoch unwraps)) | impl-consistency | contradicts | intentional-gap | REGISTERED — suppressed by §11.6 target-state row "Import cluster tagging (ADR 0007)": Live=factor-1/empty tags matches `bth_scan.rs`, Target=c_import>=F matches ADR 0007, Tracking=botho#938. Mechanism ONLY in `cluster-tax/.../bridge_import_sweep.rs`; production `bth_scan.rs:218` `debug_assert!(recipient_output.cluster_tags.is_empty())`. NOT a critical flag. |
| I2 | Output carries 100%-weight tag to c_import(m) as a mint output does to its cluster | impl-consistency | contradicts | intentional-gap | REGISTERED — same IMP row / #938. Subsection titled "(target-state)"; fig2 caption "Target-state: the current implementation releases a factor-1 output (register row IMP-1)". Suppressed. |
| I3 | Confidential-amounts-clean: import factor from public boundary amounts, no ZK gadget | factual | match | — | ADR 0007 §Consequences; amounts public at boundary (ADR 0004 / P15). Design property, internally consistent. |

## Demurrage-settlement on-ramp (second registered target-state item)

| # | Claim | Kind | Verified? | Disposition | Evidence |
|---|---|---|---|---|---|
| D1 | Demurrage-settlement op (pay to reclassify to factor-1; fee -> lottery pool) — explicitly target-state | impl-consistency | contradicts | intentional-gap | REGISTERED — §11.6 row "Demurrage-settlement operation" Live="No consensus settlement op"/Target=paid reclassification/Tracking=botho#831 (horizon #833/#925). No consensus op in code (grep). Suppressed. |

## Register-row Live/Target accuracy (audited from the code side)

| Register row | Live accurate? | Target accurate? | Evidence |
|---|---|---|---|
| Ethereum wrap/unwrap = live | YES | YES | mint/ethereum + watchers/ethereum + release/bth + WrappedBTH.sol all live |
| Import cluster tagging = target-state | YES (bth_scan empty tags) | YES (matches ADR 0007) | I1/I2 |
| Demurrage-settlement = target-state | YES (no consensus op) | YES | D1 |
| Solana transports = target-state | YES (code-complete, #[ignore]d live, unverified e2e) | YES | mint/solana + solana_rpc + watchers/solana live-wired #857 |
| BTH/Solana live supply+reserve-balance = target-state | YES (unverified, excluded from drift) | YES | reserve.rs NotImplemented->unverified; #853 |
| External security audit = target-state (mainnet gate) | YES | YES | internal adversarial/chaos/fuzz only |

## Major findings

| # | Finding | Severity | Note |
|---|---|---|---|
| M-1 | `code_ref` scalar glob `bridge/**/*.rs` does not resolve the counterparty contracts (`WrappedBTH.sol`, Solana `lib.rs`) nor `cluster-tax/.../bridge_import_sweep.rs` that the section normatively describes (C1,C4-C9; P9,P11; I1-I3). | major | Read manually per BRIEF note; sweep complete, all matched. Future automated re-audit trusting only resolved `code_ref` would skip them. Track anvil#718 (scalar `code_ref` cannot span a multi-tree implementation). No spec change required; not a blocking flag. |

## Internal-logic / factual audit (no code cross-check)

- Peg / demurrage / epoch-cluster-import-factor equations: dimensionally sound, mutually consistent, consistent with §11.6 register. No same-named-constant drift across sections (each `% anvil-const:` marker stated once authoritatively; import_epoch_blocks + import_factor_floor appear twice with IDENTICAL value+unit).
- Figure captions (fig1/fig2/fig3) do not contradict prose; fig2 annotates the target-state-vs-live factor-1 divergence, reinforcing the register.
- No unsatisfiable predicate, no misused primitive, no dimensionally-unsound formula found.
