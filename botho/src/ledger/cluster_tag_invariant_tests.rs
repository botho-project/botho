//! End-to-end checks of the cluster-tag counterexamples in the Lean model
//! (`formal/Botho/Tags/`, #1840).
//!
//! Each test builds a real, signed transaction the way a modified wallet
//! would: the stock wallet picks the ring and signs, then the test re-signs
//! the same ring over outputs whose cluster tags it chose. The helpers are
//! the #1858 ones in `mempool_tag_gate_tests.rs`.
//!
//! The Lean refutations model protocol 7.x and the pre-#1863 wallet merge.
//! Two of them were bugs, now fixed: #1857 (output tags unbounded by
//! consensus; Rules 1 and 2, protocol 8.0.0, #1870) and #1858 (the mempool's
//! `merge_weighted` panic; `is_valid` relay gate and a saturating merge,
//! #1863). Their tests are kept here as named records of the findings and
//! now assert the rejection (`*_rejected_since_1863` /
//! `*_rejected_since_1870`). The #1687 tag drop is unchanged by 8.0.0 and is
//! still pinned as accepted.
//!
//! Kept out of `store.rs` so the tests do not churn that file's SHA-256 pin in
//! `scripts/research/fixtures/ct-contract-inventory.json`.

use super::mempool_tag_gate_tests::{
    add_tx_no_panic, chain, coinbase, mine, mine_with, repeated, ring_clusters, stock_spend,
    with_tags,
};
use crate::{
    ledger::{Ledger, LedgerError},
    mempool::MempoolError,
    transaction::{Transaction, UtxoId, PICOCREDITS_PER_CREDIT},
    wallet::Wallet,
};
use bth_transaction_types::{ClusterId, ClusterTagVector, TAG_WEIGHT_SCALE};

/// `add_block` rejects the block holding `tx` with a message containing
/// `needle`, and nothing it would have written lands.
fn expect_block_rejected(ledger: &Ledger, wallet: &Wallet, tx: Transaction, needle: &str) {
    let tip = ledger.get_chain_state().unwrap().height;
    let hash = tx.hash();
    match mine_with(ledger, wallet, vec![tx]) {
        Err(LedgerError::InvalidBlock(msg)) => assert!(msg.contains(needle), "{msg}"),
        other => panic!("expected InvalidBlock containing {needle:?}, got {other:?}"),
    }
    assert_eq!(ledger.get_chain_state().unwrap().height, tip);
    assert!(ledger.get_utxo(&UtxoId::new(hash, 0)).unwrap().is_none());
}

/// #1857, fixed by #1870 (protocol 8.0.0). Lean
/// `Botho.Tags.inherit_admits_fresh_mass_above_one` and
/// `Botho.Tags.fresh_cluster_credit_unbounded` model 7.x C6: a spend whose
/// only output carries 20 entries of weight `u32::MAX` for a cluster absent
/// from the ring (85,899x the 100% scale) passed C6, and the writer credited
/// another holder's cluster with ~85,899x the output's value. Before 8.0.0
/// this test asserted that `add_block` accepted it. Now:
///
/// - Rule 1 (`is_valid` on every output) rejects that block;
/// - Rule 2 (no output-only cluster but the epoch import cluster) rejects the
///   same attack kept within `is_valid` (one entry of weight 100% naming the
///   victim), at relay and in a block;
///
/// and the victim cluster's recorded wealth is unchanged.
#[test]
fn fresh_cluster_tags_above_unit_mass_rejected_since_1870() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let input = coinbase(&ledger, &blocks[2]);
    let fee = 10 * PICOCREDITS_PER_CREDIT;

    // A victim: some other block's coinbase cluster that the ring does not
    // carry (rings are random, so retry the build until one is outside).
    let (stock, victim) = (0..20)
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
            Some((stock, victim))
        })
        .expect("some coinbase cluster lies outside the ring");
    let before = ledger.get_cluster_wealth(victim).unwrap();

    // The 7.x counterexample: above unit mass, on a fresh cluster. Rule 1.
    let tx = with_tags(
        &ledger,
        &wallet,
        &input,
        &stock,
        repeated(victim, u32::MAX, 20),
    );
    let out = &tx.outputs[0];
    assert!(!out.cluster_tags.is_valid());
    let total_weight: u64 = out
        .cluster_tags
        .entries
        .iter()
        .map(|e| e.weight as u64)
        .sum();
    assert!(total_weight > 85_000 * TAG_WEIGHT_SCALE as u64);
    expect_block_rejected(&ledger, &wallet, tx, "malformed cluster tags");
    assert_eq!(ledger.get_cluster_wealth(victim).unwrap(), before);

    // The same credit within `is_valid`: one 100% entry naming the victim.
    // Rule 2.
    let tags = ClusterTagVector::single(ClusterId(victim));
    assert!(tags.is_valid());
    let tx = with_tags(&ledger, &wallet, &input, &stock, tags);
    match add_tx_no_panic(&ledger, tx.clone()) {
        Err(MempoolError::InvalidTransaction(msg)) => {
            assert!(msg.contains("absent from every ring"), "{msg}")
        }
        other => panic!("expected relay rejection (Rule 2), got {other:?}"),
    }
    expect_block_rejected(&ledger, &wallet, tx, "absent from every ring");
    assert_eq!(ledger.get_cluster_wealth(victim).unwrap(), before);
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
    //    cluster. (A bridge release writes the same UTXO; Rule 2 (#1870, 8.0.0)
    //    still lets any spender write it, as the including epoch's import cluster.)
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
    // Demurrage = floor − base fee. Since #1685 the import toll is priced in
    // epoch 0 too, so `floor(now)` is no longer the base fee alone.
    let base = |tx: &Transaction| {
        let wealth = ledger
            .effective_cluster_wealth_from_outputs(&tx.outputs)
            .unwrap();
        bth_cluster_tax::FeeConfig::default().minimum_fee_dynamic_with_outputs(
            bth_cluster_tax::TransactionType::Hidden,
            tx.estimate_size(),
            wealth,
            tx.outputs.len(),
            tx.outputs.iter().filter(|o| o.has_memo()).count(),
            1, // CONSENSUS_FEE_BASE
        )
    };
    let demurrage = |tx: &Transaction| floor(tx, active) - base(tx);

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

/// #1858, fixed by #1863. Lean `Botho.Tags.mergeWeighted_overflow_panics`
/// models the pre-#1863 `merge_weighted`: an output with 4,295 entries of
/// weight `u32::MAX` for one cluster made `w * retention` overflow `u64`, and
/// the node's mempool, which merges a transaction's OUTPUT tags, panicked
/// (`overflow-checks = true`, #663) on a validly signed transaction. Before
/// #1863 this test asserted the panic. Now `Mempool::add_tx` rejects the
/// transaction at its `is_valid` gate, before `merge_weighted` runs, without
/// panicking (4,294 entries too); `merge_weighted` itself saturates (the DRT
/// leg `merge_weighted` compares it with `Botho.Tags.mergeWeightedSafe`); and
/// since 8.0.0 (#1870 Rule 1) the block holding it is invalid.
#[test]
fn mempool_merge_weighted_overflow_rejected_since_1863() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let input = coinbase(&ledger, &blocks[2]);
    let fee = 10 * PICOCREDITS_PER_CREDIT;
    let stock = stock_spend(&ledger, &wallet, &input, fee);
    let fresh = 0x00DE_AD00_0000_0001;
    assert!(!ring_clusters(&ledger, &stock).contains(&fresh));

    for count in [4_294usize, 4_295] {
        let tags = repeated(fresh, u32::MAX, count);
        // The fixed merge saturates instead of panicking.
        let merged = ClusterTagVector::merge_weighted(&[(tags.clone(), 1)], 0);
        assert_eq!(merged.entries.len(), 1, "{count}");
        assert_eq!(merged.entries[0].weight, u32::MAX, "{count}");

        let tx = with_tags(&ledger, &wallet, &input, &stock, tags);
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
            expect_block_rejected(&ledger, &wallet, tx, "malformed cluster tags");
        }
    }
}
