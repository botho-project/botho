//! Versioned UTXO snapshot V2 tests (#1286 P9, #1646).
use super::{
    snapshot_v2::META_SNAPSHOT_BASE,
    v2_tests::{apply_v2, ledger_at_height_one, output, payout_block, tables, tweak, Rows},
    *,
};
use crate::ledger::{
    lottery_v2::{DerivationReads, StoredContext},
    SnapshotV2Contents, UtxoSnapshot, UtxoSnapshotV2,
};
use bth_transaction_clsag::lottery_v2::Context;
use tempfile::TempDir;

/// Tables a V2 snapshot restores verbatim. `blocks` (genesis + tip only) and
/// `tx_index` (not consensus state) are not carried; `meta` is compared by
/// [`assert_meta_restored`].
const RESTORED: [&str; 6] = [
    "utxos",
    "address_index",
    "key_images",
    "cluster_wealth",
    "bridge_import_clusters",
    super::v2::CONTEXTS_TABLE,
];

fn restored_tables(ledger: &Ledger) -> Vec<(&'static str, Rows)> {
    tables(&ledger.env)
        .into_iter()
        .filter(|(name, _)| RESTORED.contains(name))
        .collect()
}

/// Every source meta row is restored byte-for-byte. The restored ledger may
/// additionally hold the snapshot base and emission keys the source never
/// wrote (written from the snapshot's chain state, whose defaults they equal),
/// so the decoded chain state must match exactly too.
fn assert_meta_restored(src: &Ledger, dst: &Ledger) {
    let meta = |l: &Ledger| {
        tables(&l.env)
            .into_iter()
            .find(|(n, _)| *n == "meta")
            .unwrap()
            .1
    };
    let dst_meta = meta(dst);
    for row in meta(src) {
        assert!(dst_meta.contains(&row), "meta row {:?} not restored", row.0);
    }
    assert_eq!(
        format!("{:?}", dst.get_chain_state().unwrap()),
        format!("{:?}", src.get_chain_state().unwrap())
    );
}

/// Source chain: golden block 1, a V2 payout block 2 (Direct coinbase + tx
/// outputs, one fixture-seeded Lottery payout), and a Direct-only block 3 that
/// reuses a block-2 target key (so the address index has a multi-entry row
/// whose creation order differs from outpoint order). Plus a key image,
/// cluster wealth, an import cluster and a non-zero lottery pool.
struct Source {
    ledger: Ledger,
    payout: UtxoId,
    payout_context: StoredContext,
    tip: Block,
}

fn source_chain(dir: &Path) -> Source {
    let (ledger, b1) = ledger_at_height_one(dir);
    let (b2, records) = payout_block(&b1);
    apply_v2(&ledger, &b2, &records, &mut || Ok(())).unwrap();
    let mut b3 = b2.clone();
    b3.header.height = 3;
    b3.header.prev_block_hash = b2.hash();
    b3.lottery_outputs.clear();
    let mut tx = BothoTransaction::new_stub_with_fee(0);
    let mut shared = output(0x31);
    shared.amount = 9_999;
    tx.outputs = vec![shared, output(0x52)];
    b3.transactions = vec![tx];
    apply_v2(&ledger, &b3, &[], &mut || Ok(())).unwrap();

    let mut txn = ledger.env.write_txn().unwrap();
    ledger
        .key_images_db
        .put(&mut txn, &[0x6b; 32], &2u64.to_le_bytes())
        .unwrap();
    ledger
        .meta_db
        .put(&mut txn, META_LOTTERY_POOL, &123_456_789u128.to_le_bytes())
        .unwrap();
    txn.commit().unwrap();
    ledger.set_cluster_wealth_for_test(77, 5_000_000).unwrap();
    ledger.record_bridge_import_cluster_for_test(88).unwrap();

    let payout = UtxoId::new(b2.hash(), 1);
    Source {
        payout_context: StoredContext::Lottery {
            height: 2,
            ordinal: 0,
            context: records[0].context.clone(),
        },
        payout,
        ledger,
        tip: b3,
    }
}

fn all_contexts(ledger: &Ledger) -> Vec<(UtxoId, StoredContext)> {
    ledger
        .fixture_utxos()
        .unwrap()
        .into_iter()
        .map(|u| (u.id, ledger.get_derivation_context(&u.id).unwrap()))
        .collect()
}

#[test]
fn snapshot_round_trip_restores_utxos_contexts_and_tip() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    let file = src_dir.path().join("chain.snapshot");
    let size = src.ledger.write_snapshot_to_file(&file).unwrap();
    assert_eq!(size, std::fs::metadata(&file).unwrap().len());
    let snapshot = src.ledger.create_snapshot().unwrap();
    assert_eq!(snapshot.version, 2);
    assert_eq!(snapshot.storage_schema, b"botho.ledger.2");
    assert_eq!(snapshot.height, 3);
    assert_eq!(snapshot.block_hash, src.tip.hash());
    assert_eq!(
        snapshot.genesis_hash,
        src.ledger.get_block(0).unwrap().hash()
    );
    assert_eq!(snapshot.lottery_pool, 123_456_789);
    // Coinbase 1, block 2 (coinbase, 2 tx outputs, 1 payout), block 3
    // (coinbase, 2 tx outputs).
    assert_eq!((snapshot.utxo_count, snapshot.context_count), (8, 8));

    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    assert_eq!(
        dst.load_snapshot_from_file(&file, Some(&src.tip.hash()))
            .unwrap(),
        8
    );

    let check = |dst: &Ledger| {
        assert_eq!(restored_tables(dst), restored_tables(&src.ledger));
        assert_eq!(all_contexts(dst), all_contexts(&src.ledger));
        assert_meta_restored(&src.ledger, dst);
        assert_eq!(dst.get_tip().unwrap().hash(), src.tip.hash());
        // The V2 payout (created below the tip, whose block is not restored)
        // keeps its exact context and is readable through the strict
        // production view used by the shared V2 rules.
        assert_eq!(
            dst.get_derivation_context(&src.payout).unwrap(),
            src.payout_context
        );
        let txn = dst.env.read_txn().unwrap();
        let view = dst.production_view(&txn);
        let (utxo, context) = view.output_context(&src.payout).unwrap();
        assert_eq!(context, src.payout_context);
        assert_eq!(utxo.output.target_key, [0x41; 32]);
        assert_eq!(view.lottery_pool().unwrap(), 123_456_789);
        assert_eq!(view.genesis_hash().unwrap(), snapshot.genesis_hash);
    };
    check(&dst);
    drop(dst);
    let reopened = Ledger::open(dst_dir.path()).unwrap();
    check(&reopened);
    // Re-exporting the restored ledger reproduces the same commitment.
    assert_eq!(
        reopened.create_snapshot().unwrap().commitment,
        snapshot.commitment
    );
}

#[test]
fn missing_creating_block_is_only_tolerated_below_a_snapshot_base() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    // Without a snapshot base, a payout whose creating block is gone is
    // inconsistent (unchanged P2 behavior).
    let mut txn = src.ledger.env.write_txn().unwrap();
    src.ledger.blocks_db.delete(&mut txn, &2).unwrap();
    txn.commit().unwrap();
    assert!(matches!(
        src.ledger.get_derivation_context(&src.payout),
        Err(LedgerError::InconsistentRecord(_))
    ));
    // A base at the payout height does not cover it (the tip block is kept).
    let mut txn = src.ledger.env.write_txn().unwrap();
    src.ledger
        .meta_db
        .put(&mut txn, META_SNAPSHOT_BASE, &2u64.to_le_bytes())
        .unwrap();
    txn.commit().unwrap();
    assert!(src.ledger.get_derivation_context(&src.payout).is_err());
}

/// Assert `load` is refused and the target ledger is byte-for-byte unchanged.
fn assert_refused_untouched(
    dst: &Ledger,
    load: impl FnOnce(&Ledger) -> Result<usize, LedgerError>,
) -> LedgerError {
    let before = tables(&dst.env);
    let err = load(dst).unwrap_err();
    assert_eq!(tables(&dst.env), before, "refused load wrote: {err}");
    err
}

/// Recompress the context section after `edit`, optionally refreshing the
/// context root (but never the commitment).
fn tamper_contexts(
    snapshot: &UtxoSnapshotV2,
    edit: impl FnOnce(&mut SnapshotV2Contents),
) -> UtxoSnapshotV2 {
    let mut contents = snapshot.contents().unwrap();
    edit(&mut contents);
    let rebuilt = UtxoSnapshotV2::new(
        snapshot.genesis_hash,
        snapshot.height,
        snapshot.block_hash,
        snapshot.chain_state.clone(),
        snapshot.lottery_pool,
        &snapshot.tip_block_bytes().unwrap(),
        &contents,
    )
    .unwrap();
    let mut tampered = snapshot.clone();
    tampered.context_data = rebuilt.context_data;
    tampered
}

#[test]
fn tampered_context_is_rejected() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    let snapshot = src.ledger.create_snapshot().unwrap();
    let position = snapshot
        .contents()
        .unwrap()
        .contexts
        .iter()
        .position(|(k, _)| k[..] == src.payout.to_bytes()[..])
        .unwrap();
    let mut other_tweak = tweak();
    other_tweak[0] = 9;
    let forged = StoredContext::Lottery {
        height: 2,
        ordinal: 0,
        context: Context {
            base_index: 0,
            tweak: other_tweak,
        },
    }
    .encode()
    .unwrap();

    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();

    // 1. Context bytes changed, roots left as they were.
    let tampered = tamper_contexts(&snapshot, |c| c.contexts[position].1 = forged.clone());
    let err = assert_refused_untouched(&dst, |l| l.load_from_snapshot(&tampered, None));
    assert!(
        err.to_string().contains("derivation context Merkle root"),
        "{err}"
    );

    // 2. Context root refreshed too: the commitment no longer matches.
    let mut rerooted = tamper_contexts(&snapshot, |c| c.contexts[position].1 = forged.clone());
    let mut contents = snapshot.contents().unwrap();
    contents.contexts[position].1 = forged.clone();
    rerooted.context_merkle_root = UtxoSnapshotV2::new(
        snapshot.genesis_hash,
        snapshot.height,
        snapshot.block_hash,
        snapshot.chain_state.clone(),
        snapshot.lottery_pool,
        &snapshot.tip_block_bytes().unwrap(),
        &contents,
    )
    .unwrap()
    .context_merkle_root;
    let err = assert_refused_untouched(&dst, |l| l.load_from_snapshot(&rerooted, None));
    assert!(err.to_string().contains("commitment mismatch"), "{err}");

    // 3. A dropped context (one UTXO without its context).
    let dropped = tamper_contexts(&snapshot, |_| {});
    let mut dropped_contents = dropped.contents().unwrap();
    dropped_contents.contexts.remove(position);
    let mut dropped = dropped;
    dropped.context_data = zstd::encode_all(
        &bincode::serialize(&dropped_contents.contexts).unwrap()[..],
        3,
    )
    .unwrap();
    assert_refused_untouched(&dst, |l| l.load_from_snapshot(&dropped, None));

    // 4. Any flipped byte in the file is refused as well.
    let mut bytes = Vec::new();
    snapshot.write_to(&mut bytes).unwrap();
    let file = dst_dir.path().join("flipped.snapshot");
    for offset in [8, bytes.len() / 3, bytes.len() / 2, bytes.len() - 1] {
        let mut flipped = bytes.clone();
        flipped[offset] ^= 0x01;
        std::fs::write(&file, &flipped).unwrap();
        assert_refused_untouched(&dst, |l| l.load_snapshot_from_file(&file, None));
    }

    // The untampered snapshot still loads.
    dst.load_from_snapshot(&snapshot, None).unwrap();
    assert_eq!(
        dst.get_derivation_context(&src.payout).unwrap(),
        src.payout_context
    );
}

#[test]
fn truncated_file_is_rejected() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    let file = src_dir.path().join("chain.snapshot");
    src.ledger.write_snapshot_to_file(&file).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    let cut = dst_dir.path().join("cut.snapshot");
    for len in [
        0,
        4,
        8,
        9,
        100,
        bytes.len() / 2,
        bytes.len() - 32,
        bytes.len() - 1,
    ] {
        std::fs::write(&cut, &bytes[..len]).unwrap();
        assert_refused_untouched(&dst, |l| l.load_snapshot_from_file(&cut, None));
    }
    // Trailing bytes are refused too.
    let mut long = bytes.clone();
    long.push(0);
    std::fs::write(&cut, &long).unwrap();
    assert_refused_untouched(&dst, |l| l.load_snapshot_from_file(&cut, None));
    // The intact file loads.
    std::fs::write(&cut, &bytes).unwrap();
    dst.load_snapshot_from_file(&cut, None).unwrap();
}

#[test]
fn wrong_genesis_is_rejected() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    let snapshot = src.ledger.create_snapshot().unwrap();
    let dst_dir = TempDir::new().unwrap();
    let mainnet = Ledger::open_for_network(dst_dir.path(), Network::Mainnet).unwrap();
    assert_ne!(mainnet.get_block(0).unwrap().hash(), snapshot.genesis_hash);
    let err = assert_refused_untouched(&mainnet, |l| l.load_from_snapshot(&snapshot, None));
    assert!(err.to_string().contains("genesis"), "{err}");

    // Rewriting the genesis field in the file breaks the commitment instead.
    let testnet_dir = TempDir::new().unwrap();
    let testnet = Ledger::open(testnet_dir.path()).unwrap();
    let mut forged = snapshot.clone();
    forged.genesis_hash = mainnet.get_block(0).unwrap().hash();
    let err = assert_refused_untouched(&testnet, |l| l.load_from_snapshot(&forged, None));
    assert!(err.to_string().contains("commitment mismatch"), "{err}");
}

#[test]
fn wrong_tip_and_non_fresh_targets_are_rejected() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    let snapshot = src.ledger.create_snapshot().unwrap();
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    let err =
        assert_refused_untouched(&dst, |l| l.load_from_snapshot(&snapshot, Some(&[0xab; 32])));
    assert!(err.to_string().contains("Block hash mismatch"), "{err}");
    // Loading over an existing chain (even the same one) is refused.
    let other_dir = TempDir::new().unwrap();
    let (at_one, _) = ledger_at_height_one(other_dir.path());
    let err = assert_refused_untouched(&at_one, |l| l.load_from_snapshot(&snapshot, None));
    assert!(err.to_string().contains("genesis-only"), "{err}");
    let err = assert_refused_untouched(&src.ledger, |l| l.load_from_snapshot(&snapshot, None));
    assert!(err.to_string().contains("genesis-only"), "{err}");
}

#[test]
fn v1_snapshot_files_are_refused_on_v2_ledgers() {
    let dir = TempDir::new().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let v1 = UtxoSnapshot::new(
        5,
        [9; 32],
        ChainState {
            height: 5,
            ..ChainState::default()
        },
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let file = dir.path().join("v1.snapshot");
    let mut bytes = Vec::new();
    v1.write_to(&mut bytes).unwrap();
    std::fs::write(&file, bytes).unwrap();
    let err = assert_refused_untouched(&ledger, |l| l.load_snapshot_from_file(&file, None));
    assert!(
        matches!(&err, LedgerError::UnsupportedSchema(m) if m.contains("V1")),
        "{err}"
    );
    // A V2 struct claiming another version is refused the same way.
    let src_dir = TempDir::new().unwrap();
    let mut other = source_chain(src_dir.path())
        .ledger
        .create_snapshot()
        .unwrap();
    other.version = 1;
    let err = assert_refused_untouched(&ledger, |l| l.load_from_snapshot(&other, None));
    assert!(matches!(err, LedgerError::UnsupportedSchema(_)), "{err}");
}

#[test]
fn genesis_only_snapshot_round_trips() {
    let src_dir = TempDir::new().unwrap();
    let src = Ledger::open(src_dir.path()).unwrap();
    let snapshot = src.create_snapshot().unwrap();
    assert_eq!((snapshot.height, snapshot.utxo_count), (0, 0));
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    assert_eq!(dst.load_from_snapshot(&snapshot, None).unwrap(), 0);
    assert_eq!(dst.get_chain_state().unwrap().tip_hash, snapshot.block_hash);
}

/// Size and timing of V2 snapshot create / write / read+verify / load. Runs
/// 100k UTXOs (half carrying a 1,088 B ML-KEM-768-sized ciphertext, as
/// protocol-6 outputs do) with Direct contexts, plus 1k Lottery-context
/// payouts. On demand: `--ignored --nocapture`.
#[test]
#[ignore = "measurement (100k UTXOs); run with --ignored --nocapture"]
fn measure_snapshot_v2_size_and_timing() {
    use rand::{rngs::StdRng, RngExt, SeedableRng};
    use std::time::Instant;
    const UTXOS: usize = 100_000;
    const PAYOUTS: usize = 1_000;
    let src_dir = TempDir::new().unwrap();
    let (ledger, b1) = ledger_at_height_one(src_dir.path());
    // Seed fixture rows at height 1 on top of the real tip, then re-tag a few
    // as tip payouts (outpoint = (tip hash, 1 + ordinal)).
    let mut rng = StdRng::seed_from_u64(1646);
    let mut utxos = Vec::with_capacity(UTXOS);
    for i in 0..UTXOS {
        let mut hash = [0u8; 32];
        rng.fill(&mut hash);
        let mut out = output((i % 251) as u8);
        rng.fill(&mut out.target_key);
        rng.fill(&mut out.public_key);
        if i % 2 == 0 {
            let mut ct = vec![0u8; 1088];
            rng.fill(&mut ct[..]);
            out.kem_ciphertext = Some(ct);
        }
        utxos.push(Utxo {
            id: UtxoId::new(hash, (i % 3) as u32),
            output: out,
            created_at: 1,
        });
    }
    let state = ledger.get_chain_state().unwrap();
    let seed = UtxoSnapshot::new(1, b1.hash(), state, utxos, vec![], vec![]).unwrap();
    ledger.seed_fixture_snapshot_unvalidated(&seed).unwrap();
    {
        let mut txn = ledger.env.write_txn().unwrap();
        for ordinal in 0..PAYOUTS as u32 {
            let id = UtxoId::new(b1.hash(), ordinal + 1);
            let mut out = output(7);
            rng.fill(&mut out.target_key);
            let utxo = Utxo {
                id,
                output: out,
                created_at: 1,
            };
            ledger
                .utxo_db
                .put(
                    &mut txn,
                    &id.to_bytes(),
                    &bincode::serialize(&utxo).unwrap(),
                )
                .unwrap();
            ledger.add_to_address_index(&mut txn, &utxo).unwrap();
            let context = StoredContext::Lottery {
                height: 1,
                ordinal,
                context: Context {
                    base_index: 0,
                    tweak: tweak(),
                },
            };
            ledger
                .contexts_db
                .put(&mut txn, &id.to_bytes(), &context.encode().unwrap())
                .unwrap();
        }
        txn.commit().unwrap();
    }
    // The seeder replaced the golden coinbase row with the fixture rows.
    let rows = UTXOS + PAYOUTS;

    let t = Instant::now();
    let snapshot = ledger.create_snapshot().unwrap();
    let create = t.elapsed();
    let file = src_dir.path().join("measure.snapshot");
    let t = Instant::now();
    let size = ledger.write_snapshot_to_file(&file).unwrap();
    let write = t.elapsed();
    let t = Instant::now();
    let read =
        UtxoSnapshotV2::read_from(std::io::BufReader::new(std::fs::File::open(&file).unwrap()))
            .unwrap();
    read.verify().unwrap();
    let verify = t.elapsed();
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    let t = Instant::now();
    assert_eq!(dst.load_snapshot_from_file(&file, None).unwrap(), rows);
    let load = t.elapsed();
    assert_eq!(snapshot.context_count as usize, rows);
    let uncompressed = {
        let c = snapshot.contents().unwrap();
        bincode::serialize(&c.utxos).unwrap().len()
            + bincode::serialize(&c.contexts).unwrap().len()
            + bincode::serialize(&c.address_index).unwrap().len()
    };
    eprintln!(
        "snapshot V2: utxos={} (half hybrid 1088 B ct) payout contexts={PAYOUTS} \
         file={} B ({:.1} B/utxo) context section={} B compressed (raw {} B) \
         address index={} B utxo section={} B; uncompressed utxo+ctx+index={} B; \
         create={:?} write(create+file)={:?} read+verify={:?} load(read+verify+write)={:?}",
        rows,
        size,
        size as f64 / rows as f64,
        snapshot.context_data.len(),
        bincode::serialize(&snapshot.contents().unwrap().contexts)
            .unwrap()
            .len(),
        snapshot.address_index_data.len(),
        snapshot.utxo_data.len(),
        uncompressed,
        create,
        write,
        verify,
        load,
    );
}
