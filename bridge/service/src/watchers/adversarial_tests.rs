// Copyright (c) 2024 The Botho Foundation

//! Reorg + finality fuzz for the Ethereum burn watcher (bridge epic #816,
//! Phase 3, issue #829).
//!
//! `ethereum::tests` covers reorg orphan / re-add / restart as fixed
//! scenarios. This module attacks the same watcher with RANDOMIZED reorg
//! depths, re-add timing, and burn placements, asserting the two
//! exactly-once safety properties that hold no matter how the chain wobbles:
//!
//! 1. **Exactly one order per burn transaction** — no reorg, re-add, or rescan
//!    ever creates a second order (or a second `burn_detected`) for a tx, and
//!    no tx is ever dropped.
//! 2. **Confirm-once against a canonical block** — a burn only advances to
//!    `BurnConfirmed` when its block is `confirmations_required` deep AND still
//!    canonical; a confirmed order's recorded block is always the current
//!    canonical hash, and confirmation never happens twice.
//!
//! Reorg depth is bounded by `confirmations_required` — a reorg deeper than
//! the confirmation requirement is outside the bridge's stated safety model
//! (the requirement is exactly the assumption that such reorgs do not
//! happen), so fuzzing beyond it would test an unsupported regime.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Mutex,
};

use async_trait::async_trait;
use bth_bridge_core::{BridgeOrder, EthereumConfig, OrderStatus};
use proptest::prelude::*;
use tokio::sync::broadcast;

use super::{
    ethereum::{BurnEvent, EthChainClient, EthereumWatcher},
    WatchError,
};
use crate::db::Database;

/// A canonical chain plus the burns ever emitted, with reorg support —
/// modeled on `ethereum::tests::MockChain` but self-contained.
struct MockChain {
    /// height -> canonical block hash.
    hashes: BTreeMap<u64, String>,
    /// Every burn ever emitted; only those whose (height, hash) is still
    /// canonical are visible via `burn_events`.
    events: Vec<BurnEvent>,
    /// Monotonic fork counter so each reorg mints fresh, distinct hashes.
    fork_seq: u64,
}

struct MockEthClient {
    chain: Mutex<MockChain>,
}

impl MockEthClient {
    fn new() -> Self {
        Self {
            chain: Mutex::new(MockChain {
                hashes: BTreeMap::new(),
                events: Vec::new(),
                fork_seq: 0,
            }),
        }
    }

    fn tip(&self) -> u64 {
        *self
            .chain
            .lock()
            .unwrap()
            .hashes
            .keys()
            .next_back()
            .unwrap_or(&0)
    }

    /// Extend the canonical chain by `n` blocks on the current fork.
    fn extend(&self, n: u64) {
        let mut chain = self.chain.lock().unwrap();
        let fork = chain.fork_seq;
        let start = chain.hashes.keys().next_back().map(|h| h + 1).unwrap_or(0);
        for h in start..start + n {
            chain.hashes.insert(h, format!("0x{}_{}", fork, h));
        }
    }

    /// Reorg the top `depth` blocks onto a fresh fork of the same length,
    /// orphaning any burns in that range. Returns the set of heights that
    /// were rewritten.
    fn reorg(&self, depth: u64) {
        let mut chain = self.chain.lock().unwrap();
        let tip = match chain.hashes.keys().next_back() {
            Some(t) => *t,
            None => return,
        };
        chain.fork_seq += 1;
        let fork = chain.fork_seq;
        let from = tip.saturating_sub(depth.saturating_sub(1));
        for h in from..=tip {
            chain.hashes.insert(h, format!("0x{}_{}", fork, h));
        }
    }

    fn block_hash(&self, height: u64) -> Option<String> {
        self.chain.lock().unwrap().hashes.get(&height).cloned()
    }

    /// Emit a single burn for `tx` in the CURRENT canonical block at the tip.
    fn emit_burn(&self, tx: &str, amount: u64, height: u64) {
        let hash = self.block_hash(height).expect("tip block must exist");
        self.chain.lock().unwrap().events.push(BurnEvent {
            tx_hash: tx.to_string(),
            log_index: 0,
            block_number: height,
            block_hash: hash,
            from: "0xburner".to_string(),
            amount,
            bth_address: "bth_dest".to_string(),
        });
    }

    /// Whether `tx` currently has a canonical (visible, non-orphaned) burn.
    fn has_canonical_burn(&self, tx: &str) -> bool {
        let chain = self.chain.lock().unwrap();
        chain
            .events
            .iter()
            .any(|e| e.tx_hash == tx && chain.hashes.get(&e.block_number) == Some(&e.block_hash))
    }
}

#[async_trait]
impl EthChainClient for MockEthClient {
    async fn latest_block(&self) -> Result<u64, WatchError> {
        Ok(self.tip())
    }

    async fn block_hash_at(&self, number: u64) -> Result<Option<String>, WatchError> {
        Ok(self.block_hash(number))
    }

    async fn burn_events(&self, from: u64, to: u64) -> Result<Vec<BurnEvent>, WatchError> {
        let chain = self.chain.lock().unwrap();
        Ok(chain
            .events
            .iter()
            .filter(|e| {
                e.block_number >= from
                    && e.block_number <= to
                    && chain.hashes.get(&e.block_number) == Some(&e.block_hash)
            })
            .cloned()
            .collect())
    }
}

fn watcher(confirmations_required: u32) -> (EthereumWatcher, Database) {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    let config = EthereumConfig {
        rpc_url: "http://localhost:8545".to_string(),
        wbth_contract: "0x00000000000000000000000000000000000000ee".to_string(),
        safe_address: None,
        chain_id: 1,
        private_key_file: None,
        private_key_env: None,
        enforce_key_permissions: false,
        confirmations_required,
        gas_price_strategy: Default::default(),
        mint_signers: Vec::new(),
        mint_threshold: 0,
    };
    let (_tx, rx) = broadcast::channel(1);
    (EthereumWatcher::new(config, db.clone(), rx), db)
}

fn burn_orders(db: &Database) -> Vec<BridgeOrder> {
    db.get_orders_by_status("burn").unwrap()
}

/// A single fuzz step against the chain + watcher.
#[derive(Debug, Clone)]
enum Step {
    /// Grow the canonical chain by 1..=4 blocks.
    Grow(u8),
    /// Emit a burn for tx `id` (0..4) at the tip, if that tx has no live burn.
    Burn(u8),
    /// Reorg the top `depth` blocks (bounded to confirmations elsewhere).
    Reorg(u8),
}

fn step_strategy() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => (1u8..=4).prop_map(Step::Grow),
        3 => (0u8..5).prop_map(Step::Burn),
        1 => (1u8..=8).prop_map(Step::Reorg),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn prop_reorg_finality_fuzz_is_exactly_once(
        confirmations in 2u32..=4,
        steps in proptest::collection::vec(step_strategy(), 1..40),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let (watcher, db) = watcher(confirmations);
            let client = MockEthClient::new();
            // Seed a starting chain so the first scan has a tip.
            client.extend(2);

            // Every distinct tx we ever emit — the ground truth for
            // "one order per tx, none dropped".
            let mut distinct_txs: HashMap<String, ()> = HashMap::new();

            for step in steps {
                match step {
                    Step::Grow(n) => client.extend(n as u64),
                    Step::Burn(id) => {
                        let tx = format!("0xburn{}", id);
                        // Emit at most one LIVE burn per tx: re-emit only
                        // after a reorg orphaned the previous location, so a
                        // tx never has two canonical events at once (which
                        // would be two logically distinct burns, not a
                        // reorg re-add). Grow a fresh block first so the burn
                        // lands ABOVE the watcher's forward-only cursor (a
                        // burn buried below the cursor would never be
                        // scanned — a test artifact, not a watcher property).
                        if !client.has_canonical_burn(&tx) {
                            client.extend(1);
                            let height = client.tip();
                            client.emit_burn(&tx, 1_000_000_000 + id as u64, height);
                            distinct_txs.insert(tx, ());
                        }
                    }
                    Step::Reorg(depth) => {
                        // Reorg strictly SHALLOWER than the confirmation
                        // window: a burn confirms only at `confirmations`
                        // depth, so a reorg of depth < confirmations can
                        // never orphan an already-confirmed burn. Reorgs at
                        // or beyond the window are, by definition, outside
                        // the bridge's stated safety assumption.
                        let max_depth = (confirmations as u64).saturating_sub(1).max(1);
                        let depth = (depth as u64).min(max_depth).max(1);
                        client.reorg(depth);
                    }
                }

                watcher.scan_once(&client).await.unwrap();

                // Invariant 1: exactly one order (and one detection) per tx.
                let orders = burn_orders(&db);
                prop_assert_eq!(
                    orders.len(),
                    distinct_txs.len(),
                    "every burn tx must map to exactly one order"
                );
                prop_assert_eq!(
                    db.count_audit_action("burn_detected").unwrap() as usize,
                    distinct_txs.len(),
                    "each tx is detected exactly once"
                );

                // Invariant 2: confirmation is once, and only against a
                // still-canonical block.
                let confirmed: Vec<_> = orders
                    .iter()
                    .filter(|o| o.status == OrderStatus::BurnConfirmed)
                    .collect();
                prop_assert_eq!(
                    db.count_audit_action("burn_confirmed").unwrap() as usize,
                    confirmed.len(),
                    "no order confirms twice"
                );
                for order in &confirmed {
                    let record = db.get_burn_by_order(&order.id).unwrap().unwrap();
                    prop_assert!(!record.orphaned, "a confirmed burn is never orphaned");
                    let canonical = client.block_hash(record.block_number);
                    prop_assert_eq!(
                        canonical.as_deref(),
                        record.block_hash.as_deref(),
                        "a confirmed burn's block must be canonical"
                    );
                }
            }
            Ok(())
        })?;
    }
}
