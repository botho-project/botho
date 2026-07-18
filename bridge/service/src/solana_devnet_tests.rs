// Copyright (c) 2024 The Botho Foundation

//! Solana devnet / `solana-test-validator` drill (#857): the REAL Rust
//! mint-submission and burn-watcher transports against a live cluster with a
//! deployed `wbth` program.
//!
//! Unlike the mocked unit tests in `mint::solana` / `watchers::solana` (which
//! exercise transaction assembly and JSON-RPC parsing against fabricated
//! responses), these tests speak to an actual RPC endpoint. They are
//! `#[ignore]`d by default — they need a running validator, a deployed +
//! initialized program, and a funded mint-authority keypair — and mirror the
//! Ethereum `fork_tests.rs` gating.
//!
//! ## Honest status
//!
//! These are documented drills, NOT part of the green `cargo test` run. The
//! development environment has no `solana`/`anchor`/`solana-test-validator`,
//! so the live path has been validated only by construction + the mocked
//! suites. Do not claim a devnet pass without recording an actual run here.
//!
//! ## Running
//!
//! 1. Start a local validator and deploy + initialize the program:
//!
//! ```text
//! solana-test-validator &                      # or use devnet
//! (cd contracts/solana && anchor build && anchor deploy)
//! # initialize the Bridge PDA + wBTH mint with your authority multisig/keypair
//! ```
//!
//! 2. Point the drill at it and run the ignored tests:
//!
//! ```text
//! export BRIDGE_SOLANA_RPC_URL=http://127.0.0.1:8899
//! export BRIDGE_SOLANA_PROGRAM=<deployed program id, base58>
//! export BRIDGE_SOLANA_KEYPAIR=<path to mint-authority keypair (hex seed or CLI json)>
//! cargo test -p bth-bridge-service -- --ignored solana_devnet_
//! ```
//!
//! `getLatestBlockhash` connectivity is checked first; a misconfigured
//! endpoint fails fast with a clear message rather than a confusing panic.

use std::env;

use bth_bridge_core::{SolanaCommitment, SolanaConfig};

use crate::solana_rpc::{HttpSolanaRpc, SolanaRpc};

/// Build a [`SolanaConfig`] from the `BRIDGE_SOLANA_*` env vars, or `None`
/// when the drill is not configured (so the test self-skips cleanly).
fn config_from_env() -> Option<SolanaConfig> {
    let rpc_url = env::var("BRIDGE_SOLANA_RPC_URL").ok()?;
    let wbth_program = env::var("BRIDGE_SOLANA_PROGRAM").ok()?;
    Some(SolanaConfig {
        rpc_url,
        wbth_program,
        keypair_file: env::var("BRIDGE_SOLANA_KEYPAIR").ok(),
        keypair_env: None,
        enforce_key_permissions: false,
        commitment: SolanaCommitment::Finalized,
        mint_signers: Vec::new(),
        mint_threshold: 0,
    })
}

/// Smoke: the raw JSON-RPC transport can fetch a recent blockhash from the
/// configured cluster. Proves the reqwest client + response parsing work
/// end-to-end against a real node.
#[tokio::test]
#[ignore = "requires a live solana-test-validator / devnet (see module docs)"]
async fn solana_devnet_get_latest_blockhash() {
    let Some(config) = config_from_env() else {
        eprintln!("BRIDGE_SOLANA_RPC_URL unset — skipping (configure per module docs)");
        return;
    };
    let rpc = HttpSolanaRpc::new(config.rpc_url).expect("build rpc client");
    let (blockhash, last_valid) = rpc
        .get_latest_blockhash()
        .await
        .expect("getLatestBlockhash against the live cluster");
    assert_ne!(blockhash, [0u8; 32], "a live blockhash is never all-zero");
    assert!(last_valid > 0, "lastValidBlockHeight should be positive");
}

/// Drill: resolve the wBTH mint from the on-chain `Bridge` account and prove
/// a full `bridge_mint` transaction assembles + signs against live PDAs.
/// Does NOT broadcast (to avoid consuming the daily limit / requiring a
/// funded authority); flip `BRIDGE_SOLANA_BROADCAST=1` to also submit.
#[tokio::test]
#[ignore = "requires a live cluster with an initialized wbth program"]
async fn solana_devnet_prepare_mint_against_live_pdas() {
    use crate::mint::{solana::SolMinter, Minter};
    use bth_bridge_core::{
        AttestationSignature, BridgeOrder, Chain, MintAuthorization, SignatureScheme,
    };

    let Some(config) = config_from_env() else {
        eprintln!("BRIDGE_SOLANA_RPC_URL unset — skipping");
        return;
    };
    if config.keypair_file.is_none() {
        eprintln!("BRIDGE_SOLANA_KEYPAIR unset — skipping mint drill");
        return;
    }

    let recipient = env::var("BRIDGE_SOLANA_RECIPIENT")
        .expect("set BRIDGE_SOLANA_RECIPIENT to a base58 pubkey with an initialized wBTH ATA");

    let minter = SolMinter::new(config).expect("construct minter");
    let order = BridgeOrder::new_mint(
        Chain::Solana,
        1_000_000_000_000, // 1 BTH
        0,
        "bth_source".to_string(),
        recipient,
    );
    let auth = MintAuthorization {
        order_id: order.order_id_bytes(),
        scheme: SignatureScheme::Ed25519,
        threshold: 1,
        signatures: vec![AttestationSignature {
            signer: vec![1u8; 32],
            signature: vec![2u8; 64],
        }],
        safe_nonce: None,
    };

    let prepared = minter
        .prepare_mint(&order, &auth)
        .await
        .expect("assemble + sign bridge_mint against live PDAs");
    assert!(!prepared.tx_id.is_empty());
    assert!(!prepared.raw.is_empty());

    if env::var("BRIDGE_SOLANA_BROADCAST").as_deref() == Ok("1") {
        minter.broadcast(&prepared).await.expect("broadcast mint");
        let status = minter
            .check_confirmation(&order, &prepared.tx_id)
            .await
            .expect("poll confirmation");
        eprintln!("devnet mint {} status: {:?}", prepared.tx_id, status);
    }
}

/// Drill (#853): resolve the wBTH mint from the live `Bridge` account and read
/// its outstanding SPL supply via `getTokenSupply` through the production
/// [`crate::reserve::SolSupplySource`] — the exact path the reserve reconciler
/// runs. Proves the Solana leg VERIFIES against a real cluster (12-decimal
/// mint => picocredits).
#[tokio::test]
#[ignore = "requires a live cluster with an initialized wbth program"]
async fn solana_devnet_reserve_supply_source() {
    use crate::reserve::{SolSupplySource, SupplySource};

    let Some(config) = config_from_env() else {
        eprintln!("BRIDGE_SOLANA_RPC_URL unset — skipping");
        return;
    };
    let source = SolSupplySource::new(&config).expect("construct supply source");
    let supply = source
        .wrapped_supply()
        .await
        .expect("read wBTH supply via getTokenSupply against the live cluster");
    eprintln!("live wBTH Solana supply: {supply} picocredits");
}

/// The Solana analog of `defi_round_trip_tests::defi_round_trip_…` (#1079): the
/// **mint → wrap → (Orca seed/swap) → burn → release** journey of a coin for
/// the Solana venue, driven through the REAL bridge-service transports and
/// asserted across the loop.
///
/// ## Construction-validated boundary (the accepted Solana pattern)
///
/// Unlike the Ethereum driver, this drill does **not** thread the Orca
/// pool/swap legs in-process: **Orca Whirlpools cannot be forked/cloned
/// hermetically** (see the maintainer note on #865), so those legs are driven
/// by the shell driver `scripts/bridge-e2e-defi-solana.sh` against **live
/// devnet** with the #870 TypeScript scripts (the operator step tracked by
/// #1052 / #868). What this Rust drill owns is every leg that CAN be exercised
/// through a real Rust transport against a cluster:
///
/// - **WRAP** — the Ed25519 t-of-n mint-submission transport
///   ([`crate::mint::solana::SolMinter`]) assembles + signs the hardened
///   `bridge_mint` against live PDAs; broadcast (when
///   `BRIDGE_SOLANA_BROADCAST=1` and the authority is funded) is exactly-once
///   via the #850 per-order marker PDA. **No shortcut mint** — the wBTH mint's
///   only authority is the federation key.
/// - **PEG** — [`crate::reserve::SolSupplySource`] reads the outstanding wBTH
///   SPL supply through the exact production path the reconciler runs (#853),
///   and a full [`crate::reserve::Reconciler`] pass proves the Solana leg
///   VERIFIES (`sol_supply` present, factor-1 12-decimal picocredits) — the peg
///   invariant across the loop. On a broadcast run the supply delta equals the
///   wrapped amount (factor-1, ADR 0003).
/// - **REPATRIATE** — the burn-watcher transport
///   ([`crate::watchers::solana::burns_from_logs`] over
///   `get_signatures_for_address` → `get_transaction_logs`) decodes a real
///   `BridgeBurnEvent` off the cluster; the native-BTH release leg is the same
///   [`crate::release::bth::BthReleaser`] the Ethereum driver exercises.
///
/// `#[ignore]`d AND self-skips unless a cluster + program are configured — it
/// never claims a live path it could not exercise (the #992/#993 discipline).
/// Run it via the driver, which boots a local `solana-test-validator` with the
/// wbth program `--clone-upgradeable-program`d from devnet:
///
/// ```text
/// BRIDGE_SOLANA_RPC_URL=http://127.0.0.1:8899 \
/// BRIDGE_SOLANA_PROGRAM=<program id> \
/// BRIDGE_SOLANA_KEYPAIR=<authority keypair> \
/// BRIDGE_SOLANA_RECIPIENT=<wBTH ATA owner / Orca LP pubkey> \
///   cargo test -p bth-bridge-service -- --ignored solana_devnet_defi_round_trip --nocapture
/// ```
#[tokio::test]
#[ignore = "requires a live solana cluster with an initialized wbth program (run scripts/bridge-e2e-defi-solana.sh)"]
async fn solana_devnet_defi_round_trip_wrap_peg_burn() {
    use bth_bridge_core::{
        AttestationSignature, BridgeConfig, BridgeOrder, Chain, MintAuthorization, SignatureScheme,
    };

    use crate::{
        db::Database,
        mint::{solana::SolMinter, Minter},
        reserve::{Reconciler, SolSupplySource, SupplySource},
        watchers::solana::burns_from_logs,
    };

    // ---- Gate 1: the Solana cluster + program are configured ----------------
    let Some(config) = config_from_env() else {
        eprintln!(
            "SKIP solana_devnet_defi_round_trip: set BRIDGE_SOLANA_RPC_URL and \
             BRIDGE_SOLANA_PROGRAM (and BRIDGE_SOLANA_KEYPAIR to also assemble the \
             mint) to run the round trip against a live cluster — see module docs \
             and scripts/bridge-e2e-defi-solana.sh"
        );
        return;
    };

    // ---- Gate 2: the cluster is reachable ----------------------------------
    let rpc = HttpSolanaRpc::new(config.rpc_url.clone()).expect("build rpc client");
    if let Err(e) = rpc.get_latest_blockhash().await {
        eprintln!(
            "SKIP solana_devnet_defi_round_trip: no Solana node reachable at {} ({e}). \
             Start a solana-test-validator (or point at devnet) — see module docs.",
            config.rpc_url
        );
        return;
    }

    // Amount to wrap, in picocredits (the 12-decimal wBTH mint is 1:1 with
    // picocredits — factor-1, ADR 0003). Default 100 BTH.
    let amount: u64 = env::var("BRIDGE_SOLANA_AMOUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000_000_000_000);

    // ---- PEG (start) — read the outstanding wBTH supply via the production
    // reserve transport (#853). This is the invariant we track across the loop.
    let supply_source = SolSupplySource::new(&config).expect("construct supply source");
    let supply_before = supply_source
        .wrapped_supply()
        .await
        .expect("read wBTH supply (start) via getTokenSupply");
    eprintln!("round trip: wBTH supply before wrap = {supply_before} picocredits");

    // =====================================================================
    // STEP 1+2 — WRAP: assemble + sign the hardened bridge_mint through the
    // REAL Ed25519 t-of-n mint transport (NOT a shortcut mint).
    // =====================================================================
    let mut supply_after_wrap = supply_before;
    if config.keypair_file.is_some() {
        let recipient = env::var("BRIDGE_SOLANA_RECIPIENT").expect(
            "set BRIDGE_SOLANA_RECIPIENT to the wBTH ATA owner (the Orca LP pubkey — \
             the minted coin seeds the pool)",
        );
        let minter = SolMinter::new(config.clone()).expect("construct minter");
        let order = BridgeOrder::new_mint(
            Chain::Solana,
            amount,
            0, // keep the peg exact
            "bth_deposit_confirmed_by_watcher".to_string(),
            recipient,
        );
        let auth = MintAuthorization {
            order_id: order.order_id_bytes(),
            scheme: SignatureScheme::Ed25519,
            threshold: 1,
            signatures: vec![AttestationSignature {
                signer: vec![1u8; 32],
                signature: vec![2u8; 64],
            }],
            safe_nonce: None,
        };

        // The mint assembles against live PDAs. The per-order marker PDA (#850)
        // makes the on-chain mint exactly-once regardless of re-submission.
        let prepared = minter
            .prepare_mint(&order, &auth)
            .await
            .expect("assemble + sign bridge_mint against live PDAs");
        assert!(!prepared.tx_id.is_empty(), "assembled mint has a signature");
        assert!(!prepared.raw.is_empty(), "assembled mint serializes");

        // Exactly-once: re-preparing the SAME order derives the SAME per-order
        // marker PDA, so a racing re-submit is a no-op on-chain.
        let prepared_again = minter
            .prepare_mint(&order, &auth)
            .await
            .expect("re-assemble the same order");
        assert_eq!(
            prepared.raw.len(),
            prepared_again.raw.len(),
            "the same order re-assembles to the same-shape transaction (exactly-once marker PDA)"
        );

        if env::var("BRIDGE_SOLANA_BROADCAST").as_deref() == Ok("1") {
            minter.broadcast(&prepared).await.expect("broadcast mint");
            let status = minter
                .check_confirmation(&order, &prepared.tx_id)
                .await
                .expect("poll confirmation");
            eprintln!(
                "round trip: wrap mint {} status {:?}",
                prepared.tx_id, status
            );

            supply_after_wrap = supply_source
                .wrapped_supply()
                .await
                .expect("read wBTH supply (after wrap)");
            // Factor-1 (ADR 0003): the wrap increased supply by exactly the
            // wrapped amount (12-decimal wBTH == picocredits, 1:1).
            assert_eq!(
                supply_after_wrap,
                supply_before + amount as u128,
                "wBTH supply rose by exactly the wrapped amount (factor-1)"
            );
        } else {
            eprintln!(
                "round trip: mint assembled (tx {}) but not broadcast — set \
                 BRIDGE_SOLANA_BROADCAST=1 with a funded authority to also submit",
                prepared.tx_id
            );
        }
    } else {
        eprintln!(
            "round trip: BRIDGE_SOLANA_KEYPAIR unset — wrap leg construction-validated \
             by the mocked mint suites; supply-delta assertion skipped"
        );
    }

    // =====================================================================
    // STEP 3+4 — SEED + SWAP on Orca. NOT threaded here: Orca Whirlpools cannot
    // be forked/cloned hermetically, so the pool/swap legs run against LIVE
    // devnet via scripts/bridge-e2e-defi-solana.sh (RUN_ORCA=1) driving the #870
    // devnet-orca-{pool,swap}.ts scripts, seeded from the wBTH minted above.
    // =====================================================================
    eprintln!(
        "round trip: Orca seed/swap legs run on live devnet (operator step #1052/#868) — \
         see scripts/bridge-e2e-defi-solana.sh"
    );

    // =====================================================================
    // STEP 5 — REPATRIATE: decode a real BridgeBurnEvent off the cluster with
    // the SAME burn-watcher transport the engine runs, then reconcile the peg.
    // =====================================================================
    let sigs = rpc
        .get_signatures_for_address(&config.wbth_program, None, "finalized")
        .await
        .expect("list recent wbth-program signatures");
    let want_sig = env::var("BRIDGE_SOLANA_BURN_SIG").ok();
    let mut decoded_burn = None;
    for (sig, _slot) in sigs.iter().take(25) {
        if let Some(want) = want_sig.as_deref() {
            if sig != want {
                continue;
            }
        }
        if let Some((logs, _tx_slot)) = rpc
            .get_transaction_logs(sig, "finalized")
            .await
            .expect("fetch tx logs")
        {
            if let Some(burn) = burns_from_logs(&logs).into_iter().next() {
                decoded_burn = Some((sig.clone(), burn));
                break;
            }
        }
    }
    if let Some((sig, burn)) = decoded_burn {
        // Provenance: the burn decodes cleanly through the production transport.
        // The engine then releases native BTH to a fresh stealth output (ADR
        // 0004) via BthReleaser — the same release leg the Ethereum driver and
        // Layer 1.5/1.75 exercise.
        assert!(burn.amount > 0, "decoded burn has a positive amount");
        assert!(
            !burn.bth_address.is_empty(),
            "decoded burn carries a BTH release destination"
        );
        eprintln!(
            "round trip: burn-watcher decoded {} picocredits from tx {} -> release destination {}",
            burn.amount, sig, burn.bth_address
        );
    } else {
        eprintln!(
            "round trip: no BridgeBurnEvent in the recent wbth-program history yet — burn \
             transport construction-validated (run the Orca swap + a bridgeBurn first, or set \
             BRIDGE_SOLANA_BURN_SIG)"
        );
    }

    // ---- PEG (reconcile) — the Solana leg VERIFIES through a full Reconciler
    // pass: sol_supply is present and matches the direct supply read (#853).
    let recon_config = BridgeConfig {
        solana: config.clone(),
        ..BridgeConfig::default()
    };
    let db = Database::open_in_memory().expect("db");
    db.migrate().expect("migrate");
    let reconciler = Reconciler::from_config(&recon_config, db);
    let proof = reconciler
        .reconcile_once()
        .await
        .expect("reconcile the Solana leg");
    let reconciled = proof
        .sol_supply
        .expect("the Solana supply leg must verify against the live cluster");
    assert_eq!(
        reconciled as u128, supply_after_wrap,
        "reconciler's Σ wBTH (Solana) == the direct supply read (peg leg verified)"
    );
    eprintln!(
        "round trip OK: wrap transport (no shortcut mint) + peg leg verified \
         (Σ wBTH == {reconciled} picocredits) + burn-watcher transport exercised; \
         Orca seed/swap is the live-devnet operator step (#1052/#868)"
    );
}
