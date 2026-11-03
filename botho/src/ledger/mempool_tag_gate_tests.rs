//! #1858 end-to-end: the mempool tag gate on real, signed transactions.
//!
//! Self-contained copies of the #1840 repro helpers
//! (`cluster_tag_invariant_tests.rs`, PR #1860, not yet on main). The stock
//! wallet picks the ring and signs, then the test re-signs the same ring over
//! an output whose cluster tags it chose, exactly as a modified wallet would.
//!
//! These pin the #1858 fix: `Mempool::add_tx` rejects malformed output tags
//! cleanly (no panic), an honest wallet's spend is unaffected, and block
//! acceptance is unchanged (relay policy only; the consensus rule is #1857).
//! Kept out of `store.rs` so the tests do not churn that file's SHA-256 pin in
//! `scripts/research/fixtures/ct-contract-inventory.json`.

use crate::{
    block::{calculate_block_reward, Block},
    ledger::{Ledger, LedgerError},
    mempool::{Mempool, MempoolError},
    transaction::{ClsagRingInput, Transaction, TxOutput, Utxo, UtxoId, PICOCREDITS_PER_CREDIT},
    wallet::{Wallet, WalletRead},
};
use bth_transaction_types::{ClusterId, ClusterTagEntry, ClusterTagVector, TAG_WEIGHT_SCALE};
use bth_util_from_random::OsRng;
use std::collections::BTreeSet;
use tempfile::TempDir;

// Public deterministic test mnemonic, used only for temporary fixtures.
const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

fn open(dir: &std::path::Path) -> Ledger {
    let ledger = Ledger::open(dir).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    ledger
}

fn template(ledger: &Ledger, wallet: &Wallet, txs: Vec<Transaction>) -> Block {
    let state = ledger.get_chain_state().unwrap();
    let tip = ledger.get_tip().unwrap();
    let mut block = Block::new_template_with_txs(
        &tip,
        &wallet.default_address(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        txs,
    );
    block.header.timestamp = block.header.timestamp.max(state.tip_timestamp);
    block.minting_tx.timestamp = block.header.timestamp;
    block
}

fn mine(ledger: &Ledger, wallet: &Wallet) -> Block {
    mine_with(ledger, wallet, vec![]).unwrap()
}

fn mine_with(
    ledger: &Ledger,
    wallet: &Wallet,
    txs: Vec<Transaction>,
) -> Result<Block, LedgerError> {
    let block = ledger.seal_block(template(ledger, wallet, txs))?;
    ledger.add_block(&block)?;
    Ok(block)
}

/// A chain of `n` mined blocks (enough coinbases for a full ring of decoys).
fn chain(n: usize) -> (TempDir, Ledger, Wallet, Vec<Block>) {
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let blocks = (0..n).map(|_| mine(&ledger, &wallet)).collect();
    (dir, ledger, wallet, blocks)
}

fn coinbase(ledger: &Ledger, block: &Block) -> Utxo {
    ledger
        .get_utxo(&UtxoId::new(block.hash(), 0))
        .unwrap()
        .unwrap()
}

/// The stock wallet's signed one-input spend of `input` to one output of
/// `input.amount - fee` (its tags are the wallet's inherited tags).
fn stock_spend(ledger: &Ledger, wallet: &Wallet, input: &Utxo, fee: u64) -> Transaction {
    let height = ledger.get_chain_state().unwrap().height;
    wallet
        .create_private_transaction(
            std::slice::from_ref(input),
            vec![TxOutput::new_hybrid_to_address(
                input.output.amount - fee,
                &wallet.default_address(),
                0,
                None,
                ClusterTagVector::empty(),
            )
            .unwrap()],
            fee,
            height,
            ledger,
        )
        .unwrap()
}

/// What a modified wallet sends: the stock wallet's ring and output, with
/// the output's cluster tags replaced by `tags`, signed again over the new
/// signing hash (which commits to the tags, #1638).
fn with_tags(
    ledger: &Ledger,
    wallet: &Wallet,
    input: &Utxo,
    stock: &Transaction,
    tags: ClusterTagVector,
) -> Transaction {
    let mut outputs = stock.outputs.clone();
    outputs[0].cluster_tags = tags;
    let ring = stock.inputs.clsag()[0].ring.clone();
    let real = ring
        .iter()
        .position(|m| m.target_key == input.output.target_key)
        .unwrap();
    let hash = Transaction::new_clsag(
        Vec::new(),
        outputs.clone(),
        stock.fee,
        stock.created_at_height,
    )
    .signing_hash();
    let key = WalletRead::recover_input(ledger, wallet, input).unwrap();
    let signed =
        ClsagRingInput::new(ring, real, &key, input.output.amount, &hash, &mut OsRng).unwrap();
    let tx = Transaction::new_clsag(vec![signed], outputs, stock.fee, stock.created_at_height);
    tx.verify_ring_signatures()
        .expect("the forged spend is validly signed");
    tx
}

/// The clusters any ring member of `tx` is tagged with: the only clusters C6
/// bounds.
fn ring_clusters(ledger: &Ledger, tx: &Transaction) -> BTreeSet<u64> {
    let mut out = BTreeSet::new();
    for input in tx.inputs.clsag() {
        for member in &input.ring {
            let utxo = ledger
                .get_utxo_by_target_key(&member.target_key)
                .unwrap()
                .unwrap();
            for e in &utxo.output.cluster_tags.entries {
                out.insert(e.cluster_id.0);
            }
        }
    }
    out
}

fn repeated(cluster: u64, weight: u32, count: usize) -> ClusterTagVector {
    ClusterTagVector {
        entries: vec![
            ClusterTagEntry {
                cluster_id: ClusterId(cluster),
                weight,
            };
            count
        ],
        decay_state: None,
    }
}

fn add_tx_no_panic(ledger: &Ledger, tx: Transaction) -> Result<[u8; 32], MempoolError> {
    let mut mempool = Mempool::new();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mempool.add_tx(tx, ledger)))
        .expect("Mempool::add_tx must not panic on malformed output tags (#1858)")
}

/// #1858, inverted form of #1840's
/// `mempool_merge_weighted_panics_on_a_signed_transaction`: the 4,295-entry
/// output (Lean `mergeWeighted_overflow_panics`) and the 4,294-entry one just
/// below the old overflow boundary are both validly signed and structurally
/// valid, and `add_tx` now returns `InvalidTransaction` for each, without
/// panicking. The same 4,295-entry transaction is still a valid block:
/// `add_block` accepts it exactly as before (no consensus change; #1857).
#[test]
fn mempool_rejects_overflowing_output_tags_on_a_signed_transaction() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let input = coinbase(&ledger, &blocks[2]);
    let fee = 10 * PICOCREDITS_PER_CREDIT;
    let stock = stock_spend(&ledger, &wallet, &input, fee);
    let fresh = 0x00DE_AD00_0000_0001;
    assert!(!ring_clusters(&ledger, &stock).contains(&fresh));

    let mut at_boundary = None;
    for count in [4_294usize, 4_295] {
        let tx = with_tags(
            &ledger,
            &wallet,
            &input,
            &stock,
            repeated(fresh, u32::MAX, count),
        );
        assert!(tx.is_valid_structure().is_ok(), "{count}");
        assert!(!tx.outputs[0].cluster_tags.is_valid(), "{count}");
        match add_tx_no_panic(&ledger, tx.clone()) {
            Err(MempoolError::InvalidTransaction(msg)) => assert!(
                msg.starts_with("output 0 has malformed cluster tags"),
                "{count}: {msg}"
            ),
            other => panic!("{count} entries: expected InvalidTransaction, got {other:?}"),
        }
        if count == 4_295 {
            at_boundary = Some(tx);
        }
    }

    // Consensus is unchanged: the same transaction is still a valid block.
    let tx = at_boundary.unwrap();
    let hash = tx.hash();
    mine_with(&ledger, &wallet, vec![tx]).expect("add_block still accepts the transaction");
    let utxo = ledger
        .get_utxo(&UtxoId::new(hash, 0))
        .unwrap()
        .expect("the malformed-tag output is in the UTXO set");
    assert_eq!(utxo.output.cluster_tags.len(), 4_295);
}

/// #1858: an honest wallet is unaffected. The stock wallet's inherited tags
/// (`compute_inherited_tags`, `merge_weighted` at the 5% wallet rate) over
/// real coinbases equal the pinned pre-fix arithmetic, are `is_valid()`, and
/// a signed spend carrying them is admitted by the mempool.
#[test]
fn honest_wallet_inherited_tags_unchanged_and_admitted() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let inputs: Vec<Utxo> = blocks[2..5].iter().map(|b| coinbase(&ledger, b)).collect();
    for u in &inputs {
        assert!(u.output.cluster_tags.is_valid());
    }

    // Pre-fix arithmetic, inline: floor(floor(v_i * w_i / V) * 950_000 / 10^6),
    // all in u64 (no overflow for valid tags).
    let total: u64 = inputs.iter().map(|u| u.output.amount).sum();
    let mut expected: Vec<(u64, u64)> = Vec::new();
    for u in &inputs {
        for e in &u.output.cluster_tags.entries {
            let c = (u.output.amount as u128 * e.weight as u128 / total as u128) as u64;
            match expected.iter_mut().find(|(id, _)| *id == e.cluster_id.0) {
                Some(x) => x.1 += c,
                None => expected.push((e.cluster_id.0, c)),
            }
        }
    }
    let expected = ClusterTagVector::from_pairs(
        &expected
            .into_iter()
            .map(|(id, w)| {
                (
                    ClusterId(id),
                    (w * 950_000 / TAG_WEIGHT_SCALE as u64) as u32,
                )
            })
            .collect::<Vec<_>>(),
    );

    let inherited = Wallet::compute_inherited_tags(&inputs, 50_000);
    assert_eq!(inherited, expected);
    assert!(inherited.is_valid());
    assert!(!inherited.is_empty());

    // A signed spend carrying honest inherited tags is admitted
    // (single-input spend so the ring fixture applies).
    let input = &inputs[0];
    let fee = 10 * PICOCREDITS_PER_CREDIT;
    let stock = stock_spend(&ledger, &wallet, input, fee);
    let tags = Wallet::compute_inherited_tags(std::slice::from_ref(input), 50_000);
    assert!(tags.is_valid());
    let tx = with_tags(&ledger, &wallet, input, &stock, tags);
    add_tx_no_panic(&ledger, tx).expect("an honest spend is admitted");
}
