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
