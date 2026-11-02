//! End-to-end checks of the cluster-tag counterexamples in the Lean model
//! (`formal/Botho/Tags/`, #1840).
//!
//! Each test builds a real, signed transaction the way a modified wallet
//! would: the stock wallet picks the ring and signs, then the test re-signs
//! the same ring over outputs whose cluster tags it chose. Consensus never
//! sees the wallet's tag rules (`ClusterTagVector::merge_weighted`), only C6
//! (`check_cluster_tag_inheritance`) and the fee floor, so this is exactly
//! what a block may contain.
//!
//! The tests pin the current behaviour. They do not propose a fix; the rule
//! changes are proposals on the issues they cite (consensus changes are
//! operator-gated).
//!
//! Kept out of `store.rs` so the tests do not churn that file's SHA-256 pin in
//! `scripts/research/fixtures/ct-contract-inventory.json`.

use crate::{
    block::{calculate_block_reward, Block},
    ledger::{Ledger, LedgerError},
    mempool::Mempool,
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

/// #1857. Lean `Botho.Tags.inherit_admits_fresh_mass_above_one` and
/// `Botho.Tags.fresh_cluster_credit_unbounded`, end to end: a block holding a
/// spend whose only output carries 20 entries of weight `u32::MAX` for a
/// cluster absent from the ring (total weight 85,899x the 100% scale) is
/// accepted by `add_block`, and the writer credits that cluster with about
/// 85,899x the output's value. The victim cluster is another miner's
/// coinbase cluster, so the spend raises a third party's recorded wealth
/// (and with it the fee factor and lottery weight read from that wealth).
#[test]
fn fresh_cluster_tags_above_unit_mass_are_accepted_and_credit_a_victim_cluster() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let input = coinbase(&ledger, &blocks[2]);
    let fee = 10 * PICOCREDITS_PER_CREDIT;

    // A victim: some other block's coinbase cluster that the ring does not
    // carry (rings are random, so retry the build until one is outside).
    let (tx, victim) = (0..20)
        .find_map(|_| {
            let stock = stock_spend(&ledger, &wallet, &input, fee);
            let in_ring = ring_clusters(&ledger, &stock);
            let victim = blocks[3..]
                .iter()
                .map(|b| {
                    coinbase(&ledger, b).output.cluster_tags.entries[0]
                        .cluster_id
                        .0
                })
                .find(|c| !in_ring.contains(c))?;
            let tags = repeated(victim, u32::MAX, 20);
            Some((with_tags(&ledger, &wallet, &input, &stock, tags), victim))
        })
        .expect("some coinbase cluster lies outside the ring");
    let out = &tx.outputs[0];
    assert!(
        !out.cluster_tags.is_valid(),
        "the node never calls is_valid"
    );
    let total_weight: u64 = out
        .cluster_tags
        .entries
        .iter()
        .map(|e| e.weight as u64)
        .sum();
    assert!(total_weight > 85_000 * TAG_WEIGHT_SCALE as u64);

    let before = ledger.get_cluster_wealth(victim).unwrap();
    mine_with(&ledger, &wallet, vec![tx.clone()]).expect("C6 and the fee floor accept it");
    let after = ledger.get_cluster_wealth(victim).unwrap();

    let per_entry = out.amount as u128 * u32::MAX as u128 / TAG_WEIGHT_SCALE as u128;
    assert_eq!(after - before, 20 * per_entry);
    assert!(after - before > 85_000 * out.amount as u128);
    // The fee curve reads that wealth for every later spend of a coin tagged
    // to the victim cluster (and for every ring using one as a decoy).
    let curve = bth_cluster_tax::FeeConfig::default();
    let (f_before, f_after) = (curve.cluster_factor(before), curve.cluster_factor(after));
    assert!(f_after > f_before);
    eprintln!(
        "victim cluster {victim:#x}: wealth {before} -> {after} (+{}x the output value {}), \
         cluster factor {f_before} -> {f_after}",
        (after - before) / out.amount as u128,
        out.amount
    );
}

/// Lean `Botho.Tags.tag_drop_escapes_import_floor`, end to end (#1687): an
/// import-tagged coin spent to an untagged output is accepted (C6 admits
/// empty output tags), and at the first demurrage-active height its
/// consensus fee floor is lower than that of the same spend keeping the
/// tag, because the ADR-0007 floor `F` is read from output tags only. The
/// demurrage it pays is also below the capitalized charge at `F`.
#[test]
fn import_tag_drop_is_accepted_and_pays_less_than_the_import_floor() {
    use bth_cluster_tax::{
        capitalized_reset_charge, import_cluster_id_for_height, ClusterFactorCurve,
        BRIDGE_IMPORT_FACTOR_FLOOR, SETTLEMENT_HORIZON_BLOCKS,
    };

    let (_dir, ledger, wallet, blocks) = chain(30);
    let fee = 10 * PICOCREDITS_PER_CREDIT;

    // 1. An import-tagged coin: an output fully tagged to the epoch's import
    //    cluster. (A bridge release writes the same UTXO; C6 also lets any spender
    //    write it, as a cluster absent from the inputs.)
    let source = coinbase(&ledger, &blocks[2]);
    let import = import_cluster_id_for_height(ledger.get_chain_state().unwrap().height + 1).0;
    let stock = stock_spend(&ledger, &wallet, &source, fee);
    let tagged = with_tags(
        &ledger,
        &wallet,
        &source,
        &stock,
        repeated(import, TAG_WEIGHT_SCALE, 1),
    );
    let block = mine_with(&ledger, &wallet, vec![tagged.clone()]).unwrap();
    assert!(ledger.is_bridge_import_cluster(import).unwrap());
    let coin = ledger
        .get_utxo(&UtxoId::new(tagged.hash(), 0))
        .unwrap()
        .expect("the import-tagged coin");
    assert_eq!(coin.created_at, block.height());
    for _ in 0..3 {
        mine(&ledger, &wallet);
    }

    // 2. The same coin, the same ring: kept tag (honest) vs dropped tag.
    let stock = stock_spend(&ledger, &wallet, &coin, fee);
    let honest = with_tags(
        &ledger,
        &wallet,
        &coin,
        &stock,
        repeated(import, TAG_WEIGHT_SCALE, 1),
    );
    let dropped = with_tags(&ledger, &wallet, &coin, &stock, ClusterTagVector::empty());

    let now = ledger.get_chain_state().unwrap().height + 1;
    let policy = crate::monetary::mainnet_policy();
    let active = policy.halving_interval;
    assert_eq!(policy.demurrage_rate_bps(now), 0);
    assert!(policy.demurrage_rate_bps(active) > 0);
    let floor = |tx: &Transaction, h: u64| ledger.consensus_fee_floor(tx, h).unwrap();
    let demurrage = |tx: &Transaction| floor(tx, active) - floor(tx, now);

    let value = dropped.outputs[0].amount;
    let blocks_per_year = (365 * 24 * 60 * 60) / policy.target_block_time_secs.max(1);
    let owed_capitalized = capitalized_reset_charge(
        value,
        BRIDGE_IMPORT_FACTOR_FLOOR,
        ClusterFactorCurve::FACTOR_SCALE,
        SETTLEMENT_HORIZON_BLOCKS,
        policy.demurrage_rate_bps(active),
        blocks_per_year,
    );
    let (d_honest, d_dropped) = (demurrage(&honest), demurrage(&dropped));
    eprintln!(
        "value {value}: demurrage kept-tag {d_honest}, dropped-tag {d_dropped}, \
         capitalized charge at F {owed_capitalized}"
    );
    assert!(d_dropped < d_honest, "dropping the tag lowers the floor");
    assert!(
        d_dropped < owed_capitalized,
        "the dropped spend pays less than the capitalized charge at F"
    );

    // 3. Both are valid today; the tag drop is applied by add_block.
    mine_with(&ledger, &wallet, vec![dropped]).expect("C6 accepts empty output tags");
}

/// #1858. Lean `Botho.Tags.mergeWeighted_overflow_panics`, end to end: the node's
/// mempool calls `ClusterTagVector::merge_weighted` over a transaction's
/// OUTPUT tags (`mempool.rs`, `validate_clsag_inputs`) after the ring
/// signature verifies and before any fee or tag check. An output with 4,295
/// entries of weight `u32::MAX` for one cluster makes the accumulated weight
/// times the retention overflow `u64`, which panics under
/// `overflow-checks = true` (release, #663). The transaction is validly
/// signed and its block is valid (C6 admits it, the cluster being fresh).
#[test]
fn mempool_merge_weighted_panics_on_a_signed_transaction() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let input = coinbase(&ledger, &blocks[2]);
    let fee = 10 * PICOCREDITS_PER_CREDIT;
    let stock = stock_spend(&ledger, &wallet, &input, fee);
    let fresh = 0x00DE_AD00_0000_0001;
    assert!(!ring_clusters(&ledger, &stock).contains(&fresh));
    let tx = with_tags(
        &ledger,
        &wallet,
        &input,
        &stock,
        repeated(fresh, u32::MAX, 4_295),
    );
    assert!(tx.is_valid_structure().is_ok());

    // One entry fewer does not overflow: the mempool rejects or admits it,
    // it does not panic.
    let below = with_tags(
        &ledger,
        &wallet,
        &input,
        &stock,
        repeated(fresh, u32::MAX, 4_294),
    );
    let mut mempool = Mempool::new();
    let below_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mempool.add_tx(below, &ledger).map(|_| ())
    }));
    assert!(below_result.is_ok(), "4,294 entries must not panic");

    let mut mempool = Mempool::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mempool.add_tx(tx.clone(), &ledger).map(|_| ())
    }));
    let panic = result.expect_err("merge_weighted overflows u64 and panics");
    let msg = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    assert!(msg.contains("overflow"), "panic message: {msg}");

    // Consensus has no such check: the same transaction is a valid block.
    mine_with(&ledger, &wallet, vec![tx]).expect("add_block accepts the transaction");
}
