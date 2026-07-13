// Copyright (c) 2024 The Botho Foundation

//! Live BTH-node integration tests (#856): the REAL Rust deposit-scan and
//! release transports against a running node over JSON-RPC.
//!
//! Unlike the mocked unit tests in `watchers::bth`, `release::bth`,
//! `bth_rpc`, and `bth_scan` (which run in every `cargo test` pass), these
//! tests talk to an actual node — so they are `#[ignore]`d and additionally
//! **self-skip** when their environment is not provided. They never claim a
//! live path they could not exercise.
//!
//! ## Running
//!
//! Bring up a local BTH testnet node with JSON-RPC exposed and a funded,
//! **factor-1** reserve wallet (its view/spend private keys written as
//! 32-byte hex files), then:
//!
//! ```text
//! BRIDGE_BTH_RPC_URL=http://127.0.0.1:7101 \
//! BRIDGE_BTH_RESERVE_VIEW_KEY=/path/to/view.hex \
//! BRIDGE_BTH_RESERVE_SPEND_KEY=/path/to/spend.hex \
//!   cargo test -p bth-bridge-service -- --ignored bth_node_
//! ```
//!
//! Mirrors the Ethereum `fork_tests.rs` pattern: same production types
//! (`NodeBthClient`, `BthReleaser`), no hand-rolled RPC.

use crate::{
    bth_rpc::BthNodeRpc,
    watchers::bth::{BthChainClient, NodeBthClient},
};
use bth_bridge_core::BthConfig;

/// The node RPC URL, or `None` to skip (no live node configured).
fn rpc_url() -> Option<String> {
    std::env::var("BRIDGE_BTH_RPC_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Reserve view/spend key file paths, or `None` to skip.
fn reserve_key_files() -> Option<(String, String)> {
    let view = std::env::var("BRIDGE_BTH_RESERVE_VIEW_KEY").ok()?;
    let spend = std::env::var("BRIDGE_BTH_RESERVE_SPEND_KEY").ok()?;
    if view.is_empty() || spend.is_empty() {
        return None;
    }
    Some((view, spend))
}

fn live_config() -> Option<BthConfig> {
    let rpc_url = rpc_url()?;
    let (view_key_file, spend_key_file) = reserve_key_files()?;
    Some(BthConfig {
        rpc_url,
        ws_url: String::new(),
        view_key_file: Some(view_key_file),
        spend_key_file: Some(spend_key_file),
        confirmations_required: 0,
        reserve_address: Some("bth_reserve_addr".to_string()),
        release_signers: Vec::new(),
        release_threshold: 0,
        release_confirmations_required: 0,
    })
}

/// The node answers `getChainInfo` and `chain_getOutputs`, and the deposit
/// scan runs the real view-key match / factor-1 gate against live blocks.
///
/// This is the transport half of Leg A of the testnet runbook: it proves the
/// `NodeBthClient` can fetch a finalized block and scan it without a mock.
#[tokio::test]
#[ignore = "requires a live BTH node (set BRIDGE_BTH_RPC_URL + reserve key files)"]
async fn bth_node_deposit_scan_reads_live_blocks() {
    let Some(config) = live_config() else {
        eprintln!(
            "SKIP: set BRIDGE_BTH_RPC_URL, BRIDGE_BTH_RESERVE_VIEW_KEY, \
             BRIDGE_BTH_RESERVE_SPEND_KEY to run this live-node test"
        );
        return;
    };

    let client = NodeBthClient::new(config).expect("client builds");
    let tip = client
        .tip_height()
        .await
        .expect("node answers getChainInfo");
    assert!(tip > 0, "node reports a non-empty chain");

    // Scan a recent block through the production transport (view-key match +
    // memo decrypt + factor-1 gate). We assert only that it does not error —
    // whether a bridge deposit exists depends on the funded node state, which
    // the operator drives per the runbook.
    let scan_height = tip.saturating_sub(1);
    let block = client
        .block_at(scan_height)
        .await
        .expect("block_at scans without error");
    if let Some(block) = block {
        assert_eq!(block.height, scan_height);
        // Any deposit the scan surfaced is, by construction, owned by the
        // reserve and carries a revealed amount.
        for deposit in &block.deposits {
            assert!(deposit.amount > 0, "revealed deposit amount is non-zero");
        }
        eprintln!(
            "scanned live block {} -> {} reserve deposit(s)",
            scan_height,
            block.deposits.len()
        );
    } else {
        eprintln!("node does not yet have height {scan_height}");
    }
}

/// The RPC client round-trips the core methods the transports depend on.
/// A minimal liveness probe that does not need reserve keys — only the URL.
#[tokio::test]
#[ignore = "requires a live BTH node (set BRIDGE_BTH_RPC_URL)"]
async fn bth_node_rpc_round_trips() {
    let Some(url) = rpc_url() else {
        eprintln!("SKIP: set BRIDGE_BTH_RPC_URL to run this live-node test");
        return;
    };
    let rpc = BthNodeRpc::new(url).expect("client builds");
    let tip = rpc.chain_tip().await.expect("getChainInfo");
    assert!(tip > 0);

    // chain_getOutputs over a small recent window must decode cleanly (this
    // exercises the amountCommitment + clusterTags + eMemo decoding path).
    let start = tip.saturating_sub(5);
    let blocks = rpc.get_outputs(start, tip).await.expect("chain_getOutputs");
    eprintln!(
        "chain_getOutputs [{start}, {tip}] -> {} block(s)",
        blocks.len()
    );

    // are_key_images_spent tolerates an empty query (no spent images).
    let statuses = rpc
        .are_key_images_spent(&[])
        .await
        .expect("chain_areKeyImagesSpent");
    assert!(statuses.is_empty());
}

/// #853: the production [`crate::reserve::NodeReserveBalanceSource`] scans the
/// live reserve window, drops spent/pending outputs, and sums the actual
/// spendable factor-1 reserve balance — the custody leg of the reconciler.
/// Proves `reserve_balance()` runs end-to-end against a real node without a
/// mock (the sum depends on the funded reserve state the operator drives).
#[tokio::test]
#[ignore = "requires a live BTH node (set BRIDGE_BTH_RPC_URL + reserve key files)"]
async fn bth_node_reserve_balance_reads_live_reserve() {
    use crate::reserve::{NodeReserveBalanceSource, ReserveBalanceSource};

    let Some(config) = live_config() else {
        eprintln!(
            "SKIP: set BRIDGE_BTH_RPC_URL, BRIDGE_BTH_RESERVE_VIEW_KEY, \
             BRIDGE_BTH_RESERVE_SPEND_KEY to run this live-node test"
        );
        return;
    };

    let source = NodeReserveBalanceSource::new(config);
    let balance = source
        .reserve_balance()
        .await
        .expect("reserve_balance scans the live reserve window without error");
    eprintln!("live reserve balance: {balance} picocredits (unspent factor-1)");
}
