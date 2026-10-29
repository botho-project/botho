// Copyright (c) 2024 Botho Foundation
//
//! Protocol 7.0.0 LotteryV2 activation over a simulated 5-node SCP network
//! (#1286 P3, #1640), at the REAL 720-block lottery maturity.
//!
//! Every node seals each externalized block with the production V2 producer
//! (`BlockBuilder::apply_lottery` -> `Ledger::seal_block`) and accepts it with
//! the production V2 validator (`Ledger::add_block`). The regression asserts:
//! - no payout before height 721, real payouts after;
//! - every payout has its own one-time target (never the source's), inherits
//!   the source's DH key and ciphertext, and commits a Lottery context;
//! - a repeated award (one source winning twice) and source/payout spends in
//!   either order through the production wallet, with distinct key images;
//! - every node holds the identical chain and identical derivation contexts;
//! - a full replay and a compact-block replay into fresh ledgers reproduce the
//!   identical chain and contexts.
//!
//! The forged-payout matrix and nested awards run at the ledger level
//! (`ledger::store::v2_activation_tests`); the V1-peer handshake refusal is
//! `commands::run` identify coverage (this harness has no libp2p).
//!
//! Long: run on the optimized profile,
//! `cargo test --profile ci -p botho --test lottery_v2_activation --
//! --nocapture`.

mod common;

use std::{
    collections::{BTreeMap, BTreeSet},
    thread,
    time::{Duration, Instant},
};

use serial_test::serial;

use botho::{
    block::Block,
    ledger::Ledger,
    mempool::Mempool,
    network::{CompactBlock, ReconstructionResult},
    transaction::{Transaction, TxOutput, Utxo, UtxoId, PICOCREDITS_PER_CREDIT},
    wallet::Wallet,
};

use crate::common::{
    confirm_transaction, mine_block, TestNetwork, TestNetworkConfig, DEFAULT_NUM_NODES,
    TRIVIAL_DIFFICULTY,
};

const MATURITY: u64 = 720;
/// Fee of the per-block self-payment that funds the lottery pool (the
/// emission share alone does not draw at these heights).
const FUNDING_FEE: u64 = 10 * PICOCREDITS_PER_CREDIT;
/// Spend fee cap for source/award spends.
const FEE: u64 = PICOCREDITS_PER_CREDIT;

/// Ordered outpoint key (`UtxoId` is not `Ord`).
type Key = [u8; 36];

fn id(key: &Key) -> UtxoId {
    UtxoId::from_bytes(key).unwrap()
}

fn height(network: &TestNetwork) -> u64 {
    network.get_node(0).chain_state().height
}

fn chain(network: &TestNetwork) -> Vec<Block> {
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();
    let tip = ledger.get_chain_state().unwrap().height;
    (1..=tip).map(|h| ledger.get_block(h).unwrap()).collect()
}

fn utxo(network: &TestNetwork, key: &Key) -> Utxo {
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();
    ledger
        .get_utxo(&id(key))
        .unwrap()
        .expect("persisted output")
}

/// payout outpoint -> its winning source outpoint.
fn awards(blocks: &[Block]) -> BTreeMap<Key, Key> {
    let mut map = BTreeMap::new();
    for block in blocks {
        for (i, o) in block.lottery_outputs.iter().enumerate() {
            map.insert(
                UtxoId::new(block.hash(), i as u32 + 1).to_bytes(),
                o.winner_utxo_id(),
            );
        }
    }
    map
}

/// Index of the wallet that owns a DIRECT (non-payout) output.
fn owner_of(network: &TestNetwork, output: &Utxo) -> usize {
    network
        .wallets
        .iter()
        .position(|w| {
            w.scan_output(&output.output, output.id.output_index)
                .is_some()
        })
        .expect("a network wallet owns the source")
}

/// Sign `input` -> `to` with the production wallet over node 0's ledger
/// (`WalletRead for Ledger` recovers payouts through their stored context).
fn spend(
    network: &TestNetwork,
    owner: &Wallet,
    to: &Wallet,
    input: &Utxo,
    fee: u64,
) -> Transaction {
    let node = network.get_node(0);
    let ledger = node.ledger.read().unwrap();
    let height = ledger.get_chain_state().unwrap().height;
    owner
        .create_private_transaction(
            std::slice::from_ref(input),
            vec![TxOutput::new(
                input.output.amount - fee,
                &to.default_address(),
            )],
            fee,
            height,
            &ledger,
        )
        .expect("production wallet signs the input")
}

fn confirm(network: &TestNetwork, tx: &Transaction, miner: usize) {
    assert!(
        confirm_transaction(network, tx, miner),
        "transaction not confirmed"
    );
}

/// Persisted payout row and derivation context, comparable across ledgers.
type Row = (
    [u8; 32],
    u64,
    u64,
    bth_transaction_clsag::lottery_v2::Context,
);

/// Contexts of `keys` on `ledger` (strict reads).
fn contexts(ledger: &Ledger, keys: &BTreeSet<Key>) -> BTreeMap<Key, Row> {
    keys.iter()
        .map(|k| {
            let (u, c) = ledger
                .fixture_derivation_context(&id(k))
                .expect("strict context");
            (*k, (u.output.target_key, u.output.amount, u.created_at, c))
        })
        .collect()
}

#[test]
#[serial]
fn lottery_v2_activation_over_scp_at_real_maturity() {
    let began = Instant::now();
    let mut network = TestNetwork::build(TestNetworkConfig::for_stress_testing());
    thread::sleep(Duration::from_millis(500));

    // ---- Maturity: no payout can exist before height 721. ------------------
    for h in 1..=MATURITY {
        mine_block(&network, (h as usize) % DEFAULT_NUM_NODES);
    }
    assert_eq!(height(&network), MATURITY);
    assert!(
        chain(&network).iter().all(|b| b.lottery_outputs.is_empty()),
        "payout before maturity"
    );
    eprintln!(
        "LOTTERY_V2_NET phase=maturity height={MATURITY} elapsed_ms={}",
        began.elapsed().as_millis()
    );

    // ---- Fund draws until one source has won twice (repeated award) and a
    // second family exists. Each funding tx spends the coinbase mined 20
    // blocks earlier: ring-mature but never yet a lottery candidate.
    // Awards spent below pay a quarter of their value as fee, so require at
    // least 0.1 credit (fee >= 0.025 credit, far above the consensus floor).
    let usable = |network: &TestNetwork, key: &Key| {
        utxo(network, key).output.amount >= PICOCREDITS_PER_CREDIT / 10
    };
    let (repeat_source, repeat_payouts, other) = loop {
        let tip = height(&network);
        let funded_height = tip - 20;
        let coinbase = {
            let node = network.get_node(0);
            let ledger = node.ledger.read().unwrap();
            let block = ledger.get_block(funded_height).unwrap();
            ledger
                .get_utxo(&UtxoId::new(block.hash(), 0))
                .unwrap()
                .unwrap()
        };
        let owner = owner_of(&network, &coinbase);
        let wallet = network.wallets[owner].clone();
        let tx = spend(&network, &wallet, &wallet, &coinbase, FUNDING_FEE);
        confirm(&network, &tx, (tip as usize + 1) % DEFAULT_NUM_NODES);

        let all = awards(&chain(&network));
        let mut by_source: BTreeMap<Key, Vec<Key>> = BTreeMap::new();
        for (payout, source) in &all {
            if usable(&network, payout) {
                by_source.entry(*source).or_default().push(*payout);
            }
        }
        if let Some((source, payouts)) = by_source.iter().find(|(_, p)| p.len() >= 2) {
            if let Some((s, p)) = by_source
                .iter()
                .find(|(s, _)| *s != source)
                .map(|(s, p)| (*s, p[0]))
            {
                break (*source, payouts.clone(), (s, p));
            }
        }
        assert!(height(&network) < 1_000, "no repeated award within bound");
    };
    let blocks = chain(&network);
    assert!(
        !blocks[MATURITY as usize].lottery_outputs.is_empty(),
        "block {} must pay its first mature candidate",
        MATURITY + 1
    );
    eprintln!(
        "LOTTERY_V2_NET phase=repeated height={} awards={} elapsed_ms={}",
        blocks.len(),
        awards(&blocks).len(),
        began.elapsed().as_millis()
    );

    // ---- Independently keyed payouts. --------------------------------------
    let all = awards(&blocks);
    let mut targets = BTreeSet::new();
    for (payout, source) in &all {
        let (p, s) = (utxo(&network, payout), utxo(&network, source));
        assert_ne!(
            p.output.target_key, s.output.target_key,
            "payout target copied"
        );
        assert_eq!(p.output.public_key, s.output.public_key, "DH key inherited");
        assert_eq!(
            p.output.kem_ciphertext, s.output.kem_ciphertext,
            "ciphertext inherited"
        );
        assert!(
            targets.insert(p.output.target_key),
            "one-time target reused"
        );
        let node = network.get_node(0);
        let (_, context) = node
            .ledger
            .read()
            .unwrap()
            .fixture_derivation_context(&id(payout))
            .unwrap();
        // First-generation awards: a direct source's original index.
        if !all.contains_key(source) {
            assert_eq!(context.base_index, id(source).output_index);
        }
        assert_ne!(context.tweak, [0; 32], "payout context carries a tweak");
    }

    // ---- Spends in either order with distinct key images. ------------------
    for _ in 0..10 {
        mine_block(&network, 0);
    }
    let order = [
        repeat_source,
        repeat_payouts[0],
        repeat_payouts[1],
        other.1,
        other.0,
    ];
    let mut images = BTreeSet::new();
    for (round, key) in order.iter().enumerate() {
        let input = utxo(&network, key);
        let source = if all.contains_key(key) {
            all[key]
        } else {
            *key
        };
        let owner = owner_of(&network, &utxo(&network, &source));
        let to = network.wallets[(owner + 1) % DEFAULT_NUM_NODES].clone();
        let fee = FEE.min(input.output.amount / 4);
        let tx = spend(&network, &network.wallets[owner].clone(), &to, &input, fee);
        let image = tx.inputs.clsag()[0].key_image;
        assert!(
            images.insert(image),
            "key image reused across source/awards"
        );
        confirm(&network, &tx, round % DEFAULT_NUM_NODES);
    }

    // ---- Every node: identical chain and contexts. -------------------------
    network.verify_consistency();
    let blocks = chain(&network);
    let keys: BTreeSet<Key> = awards(&blocks).keys().copied().collect();
    let reference = contexts(&network.get_node(0).ledger.read().unwrap(), &keys);
    for i in 1..DEFAULT_NUM_NODES {
        let node = network.get_node(i);
        let ledger = node.ledger.read().unwrap();
        assert_eq!(
            ledger.get_chain_state().unwrap().tip_hash,
            blocks.last().unwrap().hash(),
            "node {i} tip"
        );
        assert_eq!(contexts(&ledger, &keys), reference, "node {i} contexts");
    }

    // ---- Full replay and compact-block replay into fresh ledgers. ----------
    for compact in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let replica = Ledger::open(dir.path()).unwrap();
        replica.set_difficulty(TRIVIAL_DIFFICULTY).unwrap();
        let mempool = Mempool::new();
        for block in &blocks {
            let block = if compact {
                let prefill: Vec<usize> = (0..block.transactions.len()).collect();
                match CompactBlock::from_block_with_prefilled(block, &prefill).reconstruct(&mempool)
                {
                    ReconstructionResult::Complete(b) => b,
                    other => panic!("compact replay incomplete: {other:?}"),
                }
            } else {
                block.clone()
            };
            replica
                .add_block(&block)
                .expect("replica accepts the chain");
        }
        assert_eq!(
            replica.get_chain_state().unwrap().tip_hash,
            blocks.last().unwrap().hash(),
            "replica tip (compact={compact})"
        );
        assert_eq!(
            contexts(&replica, &keys),
            reference,
            "replica contexts (compact={compact})"
        );
    }

    eprintln!(
        "LOTTERY_V2_NET phase=done height={} awards={} spends={} elapsed_ms={}",
        blocks.len(),
        keys.len(),
        images.len(),
        began.elapsed().as_millis()
    );
    network.stop();
}
