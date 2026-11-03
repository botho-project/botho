//! #1857 (protocol 8.0.0): the two consensus rules on output cluster tags.
//!
//! - Rule 1: every output of every transfer tx in a block satisfies
//!   `ClusterTagVector::is_valid()` (`validate_ordinary_body`, before C6).
//! - Rule 2: an output entry may name a cluster absent from every ring member
//!   of every input only if it is the bridge-import cluster of the including
//!   block's epoch (`check_cluster_tag_inheritance`).
//!
//! The end-to-end tests are the repro of #1857 (a stock-wallet ring re-signed
//! over an output tagged to another holder's cluster), now rejected by
//! `add_block` with the victim cluster's wealth unchanged. They reuse the
//! #1858 helpers in `mempool_tag_gate_tests.rs`. Kept out of `store.rs` so the
//! tests do not churn that file's SHA-256 pin in
//! `scripts/research/fixtures/ct-contract-inventory.json`.

use super::{
    mempool_tag_gate_tests::{
        add_tx_no_panic, chain, coinbase, mine_with, repeated, ring_clusters, stock_spend,
        with_tags,
    },
    store::{check_cluster_tag_inheritance, first_malformed_output_tags},
    Ledger, LedgerError,
};
use crate::{
    mempool::MempoolError,
    transaction::{Transaction, TxOutput, Utxo, UtxoId, PICOCREDITS_PER_CREDIT},
    wallet::Wallet,
};
use bth_cluster_tax::{
    import_cluster_id, import_cluster_id_for_height, BRIDGE_IMPORT_EPOCH_BLOCKS,
};
use bth_transaction_types::{
    ClusterId, ClusterTagEntry, ClusterTagVector, MAX_CLUSTER_TAGS, MIN_STORED_WEIGHT,
    TAG_WEIGHT_SCALE,
};

// ---------------------------------------------------------------------------
// Pure-function tests (no ledger)
// ---------------------------------------------------------------------------

fn out(amount: u64, tags: ClusterTagVector) -> TxOutput {
    TxOutput {
        amount,
        target_key: [7; 32],
        public_key: [8; 32],
        e_memo: None,
        cluster_tags: tags,
        kem_ciphertext: None,
    }
}

/// A raw vector, bypassing `from_pairs` normalization (so malformed shapes
/// can be expressed).
fn raw(entries: &[(u64, u32)]) -> ClusterTagVector {
    ClusterTagVector {
        entries: entries
            .iter()
            .map(|&(id, weight)| ClusterTagEntry {
                cluster_id: ClusterId(id),
                weight,
            })
            .collect(),
        decay_state: None,
    }
}

fn full(cluster: u64) -> ClusterTagVector {
    raw(&[(cluster, TAG_WEIGHT_SCALE)])
}

/// The block-validation verdict for one tx at height `h`, in the order
/// `validate_ordinary_body` applies it: Rule 1, then C6 with Rule 2.
fn verdict(rings: &[Vec<(ClusterTagVector, u64)>], outputs: &[TxOutput], h: u64) -> &'static str {
    if first_malformed_output_tags(outputs).is_some() {
        return "rule1";
    }
    match check_cluster_tag_inheritance(rings, outputs, import_cluster_id_for_height(h).0) {
        Ok(()) => "ok",
        Err(LedgerError::InvalidBlock(msg)) if msg.contains("absent from every ring") => "rule2",
        Err(LedgerError::InvalidBlock(msg)) if msg.contains("tag inflation") => "c6",
        Err(e) => panic!("unexpected error {e:?}"),
    }
}

/// Rule 2: the import cluster of the including block's epoch is the one
/// admitted output-only cluster; the previous (and next) epoch's import
/// cluster, absent from the ring, is rejected.
#[test]
fn rule2_admits_only_the_including_epochs_import_cluster() {
    let rings = vec![vec![
        (full(1), 1_000_000u64),
        (ClusterTagVector::empty(), 5),
    ]];
    let h = BRIDGE_IMPORT_EPOCH_BLOCKS + 10; // epoch 1
    let this_epoch = import_cluster_id(1).0;
    let prev_epoch = import_cluster_id(0).0;
    let next_epoch = import_cluster_id(2).0;
    assert_eq!(import_cluster_id_for_height(h).0, this_epoch);

    assert_eq!(
        verdict(&rings, &[out(5_000_000, full(this_epoch))], h),
        "ok"
    );
    assert_eq!(
        verdict(&rings, &[out(5_000_000, full(prev_epoch))], h),
        "rule2"
    );
    assert_eq!(
        verdict(&rings, &[out(5_000_000, full(next_epoch))], h),
        "rule2"
    );
    // The first and last heights of the epoch name the same import cluster.
    for edge in [
        BRIDGE_IMPORT_EPOCH_BLOCKS,
        2 * BRIDGE_IMPORT_EPOCH_BLOCKS - 1,
    ] {
        assert_eq!(verdict(&rings, &[out(5, full(this_epoch))], edge), "ok");
    }
    assert_eq!(
        verdict(
            &rings,
            &[out(5, full(this_epoch))],
            2 * BRIDGE_IMPORT_EPOCH_BLOCKS
        ),
        "rule2"
    );
}

/// Rule 2: a ring cluster at its inherited mass is accepted, and a cluster
/// carried by a decoy whose `value * weight / SCALE` rounds to 0 still counts
/// as on the ring (presence, not mass), so its output entry is bounded by C6
/// (mass 0 + tolerance 1) rather than rejected by Rule 2.
#[test]
fn rule2_ring_presence_is_by_entry_not_mass() {
    let h = 100;
    let rings = vec![vec![
        (full(1), 1_000_000u64),
        // Mass 1 * 1000 / 1e6 = 0.
        (raw(&[(42, MIN_STORED_WEIGHT)]), 1),
    ]];
    assert_eq!(verdict(&rings, &[out(1_000_000, full(1))], h), "ok");
    // Output mass of cluster 42: 1000 * 1000 / 1e6 = 1 <= 0 + tolerance 1.
    assert_eq!(
        verdict(&rings, &[out(1_000, raw(&[(42, MIN_STORED_WEIGHT)]))], h),
        "ok"
    );
    // Above the tolerance it is C6 (inflation), not Rule 2, that rejects.
    assert_eq!(verdict(&rings, &[out(1_000, full(42))], h), "c6");
    // A cluster on no member at all is Rule 2.
    assert_eq!(
        verdict(&rings, &[out(1, raw(&[(43, MIN_STORED_WEIGHT)]))], h),
        "rule2"
    );
    // Any output, not just the first.
    assert_eq!(
        verdict(&rings, &[out(10, full(1)), out(10, full(43))], h),
        "rule2"
    );
    // Clusters may come from any input's ring.
    let two_rings = vec![vec![(full(1), 1_000u64)], vec![(full(2), 1_000u64)]];
    assert_eq!(
        verdict(
            &two_rings,
            &[out(1_000, raw(&[(1, 500_000), (2, 500_000)]))],
            h
        ),
        "ok"
    );
    // Tag drop (#1687) is unchanged: an empty output is always admitted.
    assert_eq!(
        verdict(&rings, &[out(1_000_000, ClusterTagVector::empty())], h),
        "ok"
    );
}

/// Rule 1 boundaries, all through the block-validation helper.
#[test]
fn rule1_boundaries() {
    let ok = |tags: ClusterTagVector| first_malformed_output_tags(&[out(1, tags)]).is_none();
    let sixteen: Vec<(u64, u32)> = (0..MAX_CLUSTER_TAGS as u64).map(|i| (i, 1_000)).collect();
    let seventeen: Vec<(u64, u32)> = (0..MAX_CLUSTER_TAGS as u64 + 1)
        .map(|i| (i, 1_000))
        .collect();
    assert!(ok(raw(&sixteen)));
    assert!(!ok(raw(&seventeen)));
    assert!(ok(raw(&[(1, TAG_WEIGHT_SCALE)])));
    assert!(!ok(raw(&[(1, TAG_WEIGHT_SCALE + 1)])));
    assert!(ok(raw(&[(1, 600_000), (2, 400_000)])));
    assert!(!ok(raw(&[(1, 600_000), (2, 400_001)])));
    assert!(!ok(raw(&[(1, 400_000), (2, 600_000)])), "unsorted");
    assert!(!ok(raw(&[(1, 500_000), (1, 500_000)])), "duplicate id");
    assert!(ok(raw(&[(1, MIN_STORED_WEIGHT)])));
    assert!(
        !ok(raw(&[(1, MIN_STORED_WEIGHT - 1)])),
        "below MIN_STORED_WEIGHT"
    );
    assert!(ok(ClusterTagVector::empty()));
    // The first offending output is reported.
    assert_eq!(
        first_malformed_output_tags(&[out(1, full(1)), out(1, raw(&seventeen))]),
        Some(1)
    );
}

/// Pinned `(rings, outputs, H) -> verdict` vectors for both rules, the
/// consensus contract the Lean `inheritOk₈` model is to be checked against.
/// Changing any row is a consensus change.
#[test]
fn rules_golden_vectors() {
    let e0 = import_cluster_id(0).0;
    let e1 = import_cluster_id(1).0;
    let k = BRIDGE_IMPORT_EPOCH_BLOCKS;
    let ring_1 = vec![vec![
        (full(1), 1_000u64),
        (ClusterTagVector::empty(), 1_000),
    ]];
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        &str,
        Vec<Vec<(ClusterTagVector, u64)>>,
        Vec<TxOutput>,
        u64,
        &str,
    )> = vec![
        (
            "inherit",
            ring_1.clone(),
            vec![out(1_000, full(1))],
            5,
            "ok",
        ),
        (
            "drop",
            ring_1.clone(),
            vec![out(1_000, ClusterTagVector::empty())],
            5,
            "ok",
        ),
        (
            "inflate",
            ring_1.clone(),
            vec![out(2_000, full(1))],
            5,
            "c6",
        ),
        ("fresh", ring_1.clone(), vec![out(1, full(9))], 5, "rule2"),
        (
            "import e0 at 0",
            ring_1.clone(),
            vec![out(1_000_000, full(e0))],
            0,
            "ok",
        ),
        (
            "import e0 at k-1",
            ring_1.clone(),
            vec![out(1, full(e0))],
            k - 1,
            "ok",
        ),
        (
            "import e0 at k",
            ring_1.clone(),
            vec![out(1, full(e0))],
            k,
            "rule2",
        ),
        (
            "import e1 at k",
            ring_1.clone(),
            vec![out(1, full(e1))],
            k,
            "ok",
        ),
        (
            "import e1 at k-1",
            ring_1.clone(),
            vec![out(1, full(e1))],
            k - 1,
            "rule2",
        ),
        (
            "u32::MAX weight",
            ring_1.clone(),
            vec![out(1, raw(&[(1, u32::MAX)]))],
            5,
            "rule1",
        ),
        (
            "u32::MAX weight on import",
            ring_1.clone(),
            vec![out(1, raw(&[(e0, u32::MAX)]))],
            5,
            "rule1",
        ),
        (
            "20 dup entries",
            ring_1.clone(),
            vec![out(1, repeated(1, 1_000, 20))],
            5,
            "rule1",
        ),
        (
            "mixed ring + import",
            ring_1.clone(),
            vec![out(1_000, raw(&[(e0, 500_000), (1, 500_000)]))],
            5,
            "ok",
        ),
        (
            "mixed ring + fresh",
            ring_1,
            vec![out(1_000, raw(&[(1, 500_000), (9, 500_000)]))],
            5,
            "rule2",
        ),
    ];
    for (name, rings, outputs, h, want) in rows {
        assert_eq!(verdict(&rings, &outputs, h), want, "{name}");
    }
}

/// #1857 §3 residual (pre-existing, not changed by this ruling): when a
/// release's ring happens to contain a decoy already tagged to the same
/// epoch's import cluster, that cluster is a ring cluster, so C6 bounds the
/// recipient's import mass by that decoy's mass and rejects a legitimate
/// release larger than it. Pinned here as current behaviour; tracked
/// separately as #1868 (exempting the import cluster from C6 is a policy
/// change).
#[test]
fn import_cluster_on_a_decoy_bounds_a_release_by_c6() {
    let h = 50;
    let import = import_cluster_id_for_height(h).0;
    let reserve = (ClusterTagVector::empty(), 10_000_000u64);
    let release = [
        out(5_000_000, full(import)),
        out(4_000_000, ClusterTagVector::empty()),
    ];
    // Ring without an import-tagged decoy: Rule 2's exemption applies.
    assert_eq!(
        verdict(&[vec![reserve.clone(), (full(3), 100)]], &release, h),
        "ok"
    );
    // Ring with a small import-tagged decoy: C6 rejects (the residual).
    assert_eq!(
        verdict(&[vec![reserve, (full(import), 100)]], &release, h),
        "c6"
    );
}

/// Honest-path property: `merge_weighted` (both wallets' tag inheritance)
/// over random valid input vectors returns `is_valid()` output whose clusters
/// are a subset of the inputs', so an honest spend satisfies both rules.
#[test]
fn merge_weighted_satisfies_both_rules() {
    // Deterministic splitmix64; `range(lo, hi)` is inclusive of both ends.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }
        fn range(&mut self, lo: u64, hi: u64) -> u64 {
            lo + self.next() % (hi - lo + 1)
        }
    }
    let mut rng = Rng(1857);
    for case in 0..2_000 {
        let n_inputs = rng.range(1, 6);
        let mut inputs: Vec<(ClusterTagVector, u64)> = Vec::new();
        for _ in 0..n_inputs {
            let n = rng.range(0, MAX_CLUSTER_TAGS as u64);
            let mut budget = TAG_WEIGHT_SCALE;
            let mut pairs = Vec::new();
            for _ in 0..n {
                if budget < MIN_STORED_WEIGHT {
                    break;
                }
                let w = rng.range(MIN_STORED_WEIGHT as u64, budget as u64) as u32;
                // Small id space so inputs share clusters; unique within one
                // vector (`is_valid`).
                let id = ClusterId(rng.range(0, 39));
                if pairs.iter().any(|(c, _)| *c == id) {
                    continue;
                }
                budget -= w;
                pairs.push((id, w));
            }
            let tags = ClusterTagVector::from_pairs(&pairs);
            assert!(tags.is_valid(), "case {case}: generator");
            let value = match rng.range(0, 2) {
                0 => rng.range(1, 999),
                1 => rng.range(1, u64::MAX / 8),
                _ => u64::MAX / 8,
            };
            inputs.push((tags, value));
        }
        let decay = [0u32, 50_000, 999_999][rng.range(0, 2) as usize];
        let merged = ClusterTagVector::merge_weighted(&inputs, decay);
        assert!(merged.is_valid(), "case {case}: Rule 1");
        // Rule 2 with the inputs as the ring, and an import cluster that is
        // never an input cluster.
        let outputs = [out(1, merged)];
        let rings: Vec<&ClusterTagVector> = inputs.iter().map(|(t, _)| t).collect();
        super::store::check_output_clusters_on_ring(rings, &outputs, u64::MAX)
            .unwrap_or_else(|e| panic!("case {case}: Rule 2: {e}"));
    }
}

// ---------------------------------------------------------------------------
// End to end through `add_block`
// ---------------------------------------------------------------------------

/// A coinbase cluster that no ring member of `tx` carries: another holder's
/// cluster the attacker wants to inflate.
fn victim(ledger: &Ledger, blocks: &[crate::block::Block], tx: &Transaction) -> u64 {
    let ring = ring_clusters(ledger, tx);
    blocks
        .iter()
        .map(|b| {
            coinbase(ledger, b).output.cluster_tags.entries[0]
                .cluster_id
                .0
        })
        .find(|c| !ring.contains(c))
        .expect("some coinbase cluster is off the ring")
}

fn setup() -> (
    tempfile::TempDir,
    Ledger,
    Wallet,
    Vec<crate::block::Block>,
    Utxo,
    Transaction,
) {
    let (dir, ledger, wallet, blocks) = chain(30);
    let input = coinbase(&ledger, &blocks[2]);
    let stock = stock_spend(&ledger, &wallet, &input, 10 * PICOCREDITS_PER_CREDIT);
    (dir, ledger, wallet, blocks, input, stock)
}

fn expect_rejected(ledger: &Ledger, wallet: &Wallet, tx: Transaction, needle: &str) {
    let tip = ledger.get_chain_state().unwrap().height;
    let hash = tx.hash();
    match mine_with(ledger, wallet, vec![tx]) {
        Err(LedgerError::InvalidBlock(msg)) => assert!(msg.contains(needle), "{msg}"),
        other => panic!("expected InvalidBlock containing {needle:?}, got {other:?}"),
    }
    assert_eq!(ledger.get_chain_state().unwrap().height, tip);
    assert!(ledger.get_utxo(&UtxoId::new(hash, 0)).unwrap().is_none());
}

/// Repro (a), the #1840/#1857 counterexample: 20 entries of weight
/// `u32::MAX` for a victim cluster off the ring. Before 8.0.0 `add_block`
/// accepted it and credited the victim ~85,899x the output value. Now Rule 1
/// rejects the block and the victim's wealth is unchanged.
#[test]
fn fresh_cluster_tags_above_unit_mass_are_rejected() {
    let (_dir, ledger, wallet, blocks, input, stock) = setup();
    let victim = victim(&ledger, &blocks, &stock);
    let before = ledger.get_cluster_wealth(victim).unwrap();
    let tx = with_tags(
        &ledger,
        &wallet,
        &input,
        &stock,
        repeated(victim, u32::MAX, 20),
    );
    expect_rejected(&ledger, &wallet, tx, "malformed cluster tags");
    assert_eq!(ledger.get_cluster_wealth(victim).unwrap(), before);
}

/// Repro (b), the attack within `is_valid`: one entry of weight
/// `TAG_WEIGHT_SCALE` naming the victim cluster off the ring. Rule 2 rejects
/// the block and the victim's wealth is unchanged.
#[test]
fn valid_vector_naming_a_cluster_off_the_ring_is_rejected() {
    let (_dir, ledger, wallet, blocks, input, stock) = setup();
    let victim = victim(&ledger, &blocks, &stock);
    let before = ledger.get_cluster_wealth(victim).unwrap();
    let tags = ClusterTagVector::single(ClusterId(victim));
    assert!(tags.is_valid());
    let tx = with_tags(&ledger, &wallet, &input, &stock, tags.clone());

    // Relay refuses it too (Rule 2 at `tip + 1`).
    match add_tx_no_panic(&ledger, tx.clone()) {
        Err(MempoolError::InvalidTransaction(msg)) => {
            assert!(msg.contains("absent from every ring"), "{msg}")
        }
        other => panic!("expected InvalidTransaction, got {other:?}"),
    }
    expect_rejected(&ledger, &wallet, tx, "absent from every ring");
    assert_eq!(ledger.get_cluster_wealth(victim).unwrap(), before);
}

/// The honest stock-wallet spend (inherited tags, `merge_weighted`) is
/// admitted by relay and accepted by `add_block`.
#[test]
fn honest_inherited_spend_is_accepted() {
    let (_dir, ledger, wallet, _blocks, input, stock) = setup();
    let inherited = Wallet::compute_inherited_tags(std::slice::from_ref(&input), 50_000);
    assert!(!inherited.is_empty());
    let honest = with_tags(&ledger, &wallet, &input, &stock, inherited);
    add_tx_no_panic(&ledger, honest.clone()).expect("relay admits the honest spend");
    mine_with(&ledger, &wallet, vec![honest]).expect("honest spend is a valid block");
}

/// An output tagged to the including block's import cluster (the bridge
/// release shape, ADR 0007) is admitted by relay and accepted by `add_block`,
/// and the ledger records the import cluster. An output tagged to a different
/// epoch's import cluster (a release included outside the epoch its tag
/// names) is rejected by both.
#[test]
fn import_tagged_spend_is_accepted_only_in_its_epoch() {
    let (_dir, ledger, wallet, _blocks, input, stock) = setup();
    let next = ledger.get_chain_state().unwrap().height + 1;
    let import = import_cluster_id_for_height(next).0;
    assert_eq!(import, import_cluster_id(0).0);
    assert!(!ring_clusters(&ledger, &stock).contains(&import));

    let wrong_epoch = with_tags(
        &ledger,
        &wallet,
        &input,
        &stock,
        ClusterTagVector::single(ClusterId(import_cluster_id(1).0)),
    );
    match add_tx_no_panic(&ledger, wrong_epoch.clone()) {
        Err(MempoolError::InvalidTransaction(msg)) => {
            assert!(msg.contains("absent from every ring"), "{msg}")
        }
        other => panic!("expected InvalidTransaction, got {other:?}"),
    }
    expect_rejected(&ledger, &wallet, wrong_epoch, "absent from every ring");

    let tagged = with_tags(
        &ledger,
        &wallet,
        &input,
        &stock,
        ClusterTagVector::single(ClusterId(import)),
    );
    add_tx_no_panic(&ledger, tagged.clone()).expect("relay admits the import-tagged spend");
    mine_with(&ledger, &wallet, vec![tagged]).expect("import-tagged spend is a valid block");
    assert!(ledger.is_bridge_import_cluster(import).unwrap());
}
