//! Versioned UTXO snapshot V2 tests (#1286 P9, #1646).
use super::{
    snapshot_v2::META_SNAPSHOT_BASE,
    v2_tests::{apply_v2, ledger_at_height_one, output, payout_block, tables, tweak, Rows},
    *,
};
use crate::ledger::{
    lottery_v2::{DerivationReads, StoredContext},
    DerivationContext, SnapshotV2Contents, UtxoSnapshot, UtxoSnapshotV2,
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

/// zstd of `len` zero bytes: a few KB that inflate enormously.
fn zeros_bomb(len: usize) -> Vec<u8> {
    zstd::encode_all(&vec![0u8; len][..], 19).unwrap()
}

#[test]
fn decompression_is_bounded_by_declared_counts() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    let snapshot = src.ledger.create_snapshot().unwrap();
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();

    // One declared key image (cap 8 + 40 B) but the section inflates to
    // 64 MiB. Without the bound this decompresses fully and only then fails
    // strict decoding ("bytes remaining"); with it, decoding stops at the cap.
    let mut bomb = snapshot.clone();
    bomb.key_image_data = zeros_bomb(64 << 20);
    assert!(bomb.key_image_data.len() < 16 << 10);
    let bound = |e: &dyn std::fmt::Display| {
        let m = e.to_string();
        assert!(
            m.contains("key image section decompresses past its 48 B bound"),
            "{m}"
        );
    };
    bound(&bomb.verify().unwrap_err()); // the `snapshot info` path
    bound(&bomb.contents().unwrap_err());
    bound(&assert_refused_untouched(&dst, |l| {
        l.load_from_snapshot(&bomb, None)
    }));
    let file = dst_dir.path().join("bomb.snapshot");
    let mut bytes = Vec::new();
    bomb.write_to(&mut bytes).unwrap();
    std::fs::write(&file, bytes).unwrap();
    bound(&assert_refused_untouched(&dst, |l| {
        l.load_snapshot_from_file(&file, None)
    }));

    // The tip block is capped at the stored-block bound.
    let mut tip_bomb = snapshot.clone();
    tip_bomb.tip_block_data = zeros_bomb(17 << 20);
    let err = assert_refused_untouched(&dst, |l| l.load_from_snapshot(&tip_bomb, None));
    assert!(
        err.to_string()
            .contains("tip block section decompresses past"),
        "{err}"
    );

    // Forged counts cannot raise the cap past the global ceiling (checked
    // before anything is decompressed), including on overflow.
    for count in [u64::MAX, u64::MAX / 4096, (2u64 << 30) / 40] {
        let mut forged = snapshot.clone();
        forged.key_image_count = count;
        let err = assert_refused_untouched(&dst, |l| l.load_from_snapshot(&forged, None));
        assert!(
            err.to_string().contains("decompressed snapshot ceiling"),
            "{err}"
        );
        assert!(forged.verify().is_err());
    }

    // The genuine snapshot is within every bound and still loads.
    dst.load_from_snapshot(&snapshot, None).unwrap();
}

#[test]
fn restored_ledger_reports_blocks_below_its_snapshot_base() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    assert_eq!(src.ledger.snapshot_base_height().unwrap(), None);
    assert!(matches!(
        src.ledger.get_block(9),
        Err(LedgerError::BlockNotFound(9))
    ));
    let snapshot = src.ledger.create_snapshot().unwrap();
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    dst.load_from_snapshot(&snapshot, None).unwrap();

    assert_eq!(dst.snapshot_base_height().unwrap(), Some(3));
    let genesis = dst.get_block(0).unwrap();
    assert_eq!(genesis.hash(), snapshot.genesis_hash);
    for h in [1, 2] {
        match dst.get_block(h) {
            Err(LedgerError::BelowSnapshotBase { height, base }) => {
                assert_eq!((height, base), (h, 3))
            }
            other => panic!("height {h}: {other:?}"),
        }
    }
    assert_eq!(dst.get_block(3).unwrap().hash(), src.tip.hash());
    assert!(matches!(
        dst.get_block(4),
        Err(LedgerError::BlockNotFound(4))
    ));
    // Range reads fail closed across the gap instead of returning a short
    // range, and still serve from the base upward.
    assert!(matches!(
        dst.get_blocks(0, 5),
        Err(LedgerError::BelowSnapshotBase { height: 1, base: 3 })
    ));
    assert!(matches!(
        dst.get_blocks(2, 1),
        Err(LedgerError::BelowSnapshotBase { height: 2, base: 3 })
    ));
    let from_base = dst.get_blocks(3, 5).unwrap();
    assert_eq!(from_base.len(), 1);
    assert_eq!(from_base[0].hash(), src.tip.hash());
    // Hash lookback skips the gap rather than failing.
    assert_eq!(
        dst.get_block_by_hash(&genesis.hash(), 10)
            .unwrap()
            .map(|b| b.hash()),
        Some(genesis.hash())
    );
    assert_eq!(
        dst.get_block_by_hash(&[0xcd; 32], 10)
            .unwrap()
            .map(|b| b.hash()),
        None
    );
}

/// Re-commit `snapshot` with edited chain state / contents (a consistent
/// forgery: only the semantic load checks can catch it).
fn recommit(
    snapshot: &UtxoSnapshotV2,
    edit: impl FnOnce(&mut ChainState, &mut SnapshotV2Contents),
) -> UtxoSnapshotV2 {
    let mut state = snapshot.chain_state.clone();
    let mut contents = snapshot.contents().unwrap();
    edit(&mut state, &mut contents);
    UtxoSnapshotV2::new(
        snapshot.genesis_hash,
        snapshot.height,
        snapshot.block_hash,
        state,
        snapshot.lottery_pool,
        &snapshot.tip_block_bytes().unwrap(),
        &contents,
    )
    .unwrap()
}

#[test]
fn tip_timestamp_and_key_image_heights_are_checked() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    let snapshot = src.ledger.create_snapshot().unwrap();
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();

    let skewed = recommit(&snapshot, |s, _| s.tip_timestamp += 1);
    skewed.verify().unwrap();
    let err = assert_refused_untouched(&dst, |l| l.load_from_snapshot(&skewed, None));
    assert!(err.to_string().contains("tip timestamp"), "{err}");

    let future = recommit(&snapshot, |_, c| c.key_images[0].1 = snapshot.height + 1);
    future.verify().unwrap();
    let err = assert_refused_untouched(&dst, |l| l.load_from_snapshot(&future, None));
    assert!(err.to_string().contains("key image spent above"), "{err}");

    let at_tip = recommit(&snapshot, |_, c| c.key_images[0].1 = snapshot.height);
    dst.load_from_snapshot(&at_tip, None).unwrap();
}

/// #1664 review, items 2 and 3 (post-rebase onto P4 #1655):
/// - P4's one-txn payout read `get_block_with_payout_contexts` reports a block
///   the restored ledger never held as `BelowSnapshotBase`, not `BlockNotFound`
///   (which callers treat as "past the tip");
/// - the below-base tolerance lives in `read_context_with` itself, so it
///   applies to EVERY creating-block lookup that reports "no block stored" (the
///   default `blocks_db` path behind `read_context`, P4's public
///   `get_output_derivation_context`, and any custom lookup), and nowhere else:
///   without a base, or at/above it, a missing block stays
///   `InconsistentRecord`.
#[test]
fn payout_reads_on_a_restored_ledger_honour_the_snapshot_base() {
    let src_dir = TempDir::new().unwrap();
    let src = source_chain(src_dir.path());
    let snapshot = src.ledger.create_snapshot().unwrap();
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    dst.load_from_snapshot(&snapshot, None).unwrap();
    assert_eq!(dst.snapshot_base_height().unwrap(), Some(3));

    // Item 2: the payout read distinguishes "never held" from "past the tip".
    for h in [1, 2] {
        match dst.get_block_with_payout_contexts(h) {
            Err(LedgerError::BelowSnapshotBase { height, base }) => {
                assert_eq!((height, base), (h, 3))
            }
            other => panic!("height {h}: {other:?}"),
        }
    }
    let (tip, payouts) = dst.get_block_with_payout_contexts(3).unwrap();
    assert_eq!(tip.hash(), src.tip.hash());
    assert!(payouts.is_empty());
    assert!(matches!(
        dst.get_block_with_payout_contexts(4),
        Err(LedgerError::BlockNotFound(4))
    ));
    assert_eq!(dst.get_block_with_payout_contexts(0).unwrap().0.height(), 0);
    // Without a snapshot base the same missing heights are past the tip.
    assert!(matches!(
        src.ledger.get_block_with_payout_contexts(4),
        Err(LedgerError::BlockNotFound(4))
    ));

    // Item 3, default blocks_db path: the below-base payout (block 2 is not
    // held) is served by both strict readers.
    let expected = DerivationContext::from(&src.payout_context);
    assert_eq!(
        dst.get_output_derivation_context(&src.payout).unwrap().1,
        expected
    );
    assert_eq!(
        dst.get_derivation_context(&src.payout).unwrap(),
        src.payout_context
    );

    // Item 3, the arm is in `read_context_with`: a caller-supplied lookup
    // that reports "no block stored" is judged by the same rule.
    let none = |_: u64| -> Result<Option<[u8; 32]>, LedgerError> { Ok(None) };
    {
        let txn = dst.env.read_txn().unwrap();
        let (utxo, context) =
            super::v2::read_context_with(&dst, &txn, &src.payout, &mut none.clone()).unwrap();
        assert_eq!((utxo.id, context), (src.payout, src.payout_context.clone()));
        // A lookup that does find a block must still match the outpoint.
        let wrong =
            super::v2::read_context_with(&dst, &txn, &src.payout, &mut |_| Ok(Some([0xee; 32])));
        assert!(
            matches!(&wrong, Err(LedgerError::InconsistentRecord(m)) if m.contains("creating block")),
            "{wrong:?}"
        );
    }
    // Without a base (the source), "no block stored" is inconsistent, for
    // the custom lookup and the default path alike.
    {
        let txn = src.ledger.env.read_txn().unwrap();
        let missing =
            super::v2::read_context_with(&src.ledger, &txn, &src.payout, &mut none.clone());
        assert!(
            matches!(&missing, Err(LedgerError::InconsistentRecord(m)) if m == "missing payout creating block"),
            "{missing:?}"
        );
    }
    // At/above the base it is inconsistent too: move the restored base down
    // to the payout height, so block 2 is no longer "strictly below".
    let mut txn = dst.env.write_txn().unwrap();
    dst.meta_db
        .put(&mut txn, META_SNAPSHOT_BASE, &2u64.to_le_bytes())
        .unwrap();
    txn.commit().unwrap();
    assert!(matches!(
        dst.get_output_derivation_context(&src.payout),
        Err(LedgerError::InconsistentRecord(_))
    ));
}

/// #1646 acceptance, "a V2 payout stays spendable after restoring", against
/// REAL V2 blocks: every block below is sealed by P3's producer
/// (`Ledger::seal_block`, #1640) and applied by the V2 validator
/// (`Ledger::add_block`), on the source and on the restored ledger alike.
///
/// To stay cheap enough to run un-ignored, the pre-state is seeded at a
/// height past the 720-block lottery maturity with mature wallet outputs
/// (P2's fixture seeder, Direct contexts), instead of mining 720 blocks; the
/// draw, the payout derivation and validation are the production ones. The
/// payout's creating block lies strictly below the snapshot base, so the
/// restored ledger does not hold it: discovery, input recovery, ring
/// building, the V2 producer's candidate scan and the validator all go
/// through the below-base arm of `read_context_with`.
#[test]
fn sealed_v2_payout_is_spendable_after_snapshot_restore() {
    use crate::{
        block::calculate_block_reward,
        ledger::{
            wallet_v2_fixture::{OTHER_WORDS, OWNER_WORDS, SOURCE_AMOUNT},
            ChainState,
        },
        transaction::{Transaction, TxOutput, Utxo, MIN_RING_SIZE, PICOCREDITS_PER_CREDIT},
        wallet::Wallet,
    };
    use bth_transaction_types::ClusterTagVector;

    /// Past the 720-block maturity, so the seeded outputs are candidates.
    const SEEDED: u64 = 800;
    const FEE: u64 = PICOCREDITS_PER_CREDIT;
    /// Fee of the self-payment that funds a draw.
    const FUNDING_FEE: u64 = 10 * PICOCREDITS_PER_CREDIT;
    /// Blocks on top of the paying block: past the spend maturity, and the
    /// paying block ends up strictly below the snapshot base.
    const PAD: u64 = 12;

    /// Seal (V2 producer) and apply (V2 validator) the next block.
    fn mine(ledger: &Ledger, minter: &Wallet, txs: Vec<Transaction>) -> Block {
        let state = ledger.get_chain_state().unwrap();
        let mut block = Block::new_template_with_txs(
            &Block::genesis(),
            &minter.default_address(),
            state.difficulty,
            calculate_block_reward(state.height + 1, state.total_mined),
            txs,
        );
        block.header.height = state.height + 1;
        block.header.prev_block_hash = state.tip_hash;
        block.header.timestamp = block.header.timestamp.max(state.tip_timestamp);
        block.minting_tx.block_height = block.header.height;
        block.minting_tx.prev_block_hash = state.tip_hash;
        block.minting_tx.timestamp = block.header.timestamp;
        let mut block = ledger.seal_block(block).expect("seal");
        while !block.header.is_valid_pow() {
            block.header.nonce += 1;
        }
        block.minting_tx.nonce = block.header.nonce;
        ledger
            .add_block(&block)
            .expect("V2 validator accepts the V2 producer");
        block
    }

    fn spend(ledger: &Ledger, owner: &Wallet, to: &Wallet, input: &Utxo, fee: u64) -> Transaction {
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
                ledger,
            )
            .expect("production wallet signs the input")
    }

    let owner = Wallet::from_mnemonic(OWNER_WORDS).unwrap();
    let other = Wallet::from_mnemonic(OTHER_WORDS).unwrap();

    // ---- Source: mature owner outputs (one hybrid at a nonzero index). ----
    let src_dir = TempDir::new().unwrap();
    let src = Ledger::open(src_dir.path()).unwrap();
    let mut utxos = vec![Utxo {
        id: UtxoId::new([0xa1; 32], 3),
        output: TxOutput::new_hybrid_to_address(
            SOURCE_AMOUNT,
            &owner.default_address(),
            3,
            None,
            ClusterTagVector::empty(),
        )
        .unwrap(),
        created_at: 1,
    }];
    for i in 0..MIN_RING_SIZE + 8 {
        utxos.push(Utxo {
            id: UtxoId::new([i as u8 + 0x10; 32], 0),
            output: TxOutput::new(SOURCE_AMOUNT, &owner.default_address()),
            created_at: 1,
        });
    }
    let funding_sources: Vec<Utxo> = utxos[1..].to_vec();
    let state = ChainState {
        height: SEEDED,
        difficulty: u64::MAX,
        ..ChainState::default()
    };
    let seed = UtxoSnapshot::new(SEEDED, [0; 32], state, utxos, vec![], vec![]).unwrap();
    src.seed_fixture_snapshot_unvalidated(&seed).unwrap();

    // ---- A real sealed V2 payout block (funded by a fee). ------------------
    let mut funding = funding_sources.iter();
    let paying = loop {
        let source = funding.next().expect("no V2 draw within the funding bound");
        let fund = spend(&src, &owner, &other, source, FUNDING_FEE);
        let block = mine(&src, &other, vec![fund]);
        if !block.lottery_outputs.is_empty() {
            break block;
        }
    };
    let payout = UtxoId::new(paying.hash(), 1);
    let winner = UtxoId::new(
        paying.lottery_outputs[0].winner_tx_hash,
        paying.lottery_outputs[0].winner_output_index,
    );
    let StoredContext::Lottery { height, .. } = src.get_derivation_context(&payout).unwrap() else {
        panic!("a sealed payout carries a Lottery context");
    };
    assert_eq!(height, paying.height());
    assert_ne!(
        src.get_utxo(&payout).unwrap().unwrap().output.target_key,
        src.get_utxo(&winner).unwrap().unwrap().output.target_key,
        "the payout is independently keyed"
    );
    // Every candidate is the owner's, so every payout from here on is too
    // (the pad blocks may draw from the emission share).
    let mut payouts_total = paying.lottery_outputs.len();
    for _ in 0..PAD {
        payouts_total += mine(&src, &other, vec![]).lottery_outputs.len();
    }
    let tip = src.get_chain_state().unwrap();
    assert_eq!(tip.height, paying.height() + PAD);

    // ---- Snapshot -> file -> restore into a fresh genesis-only ledger. -----
    let file = src_dir.path().join("sealed.snapshot");
    src.write_snapshot_to_file(&file).unwrap();
    let dst_dir = TempDir::new().unwrap();
    let dst = Ledger::open(dst_dir.path()).unwrap();
    dst.load_snapshot_from_file(&file, Some(&tip.tip_hash))
        .unwrap();
    assert_eq!(dst.snapshot_base_height().unwrap(), Some(tip.height));
    assert!(matches!(
        dst.get_block(paying.height()),
        Err(LedgerError::BelowSnapshotBase { .. })
    ));
    assert_eq!(
        dst.get_derivation_context(&payout).unwrap(),
        src.get_derivation_context(&payout).unwrap()
    );

    // The production wallet discovers the same outputs, payouts included,
    // with the same key images, on both ledgers.
    let view = |ledger: &Ledger| {
        let mut owned: Vec<_> = ledger
            .discover_wallet_outputs(&owner)
            .unwrap()
            .into_iter()
            .map(|o| {
                (
                    o.utxo.id.to_bytes(),
                    o.key_image,
                    o.lottery_payout,
                    o.spent_at,
                )
            })
            .collect();
        owned.sort();
        owned
    };
    let restored_view = view(&dst);
    assert_eq!(restored_view, view(&src));
    let paid: Vec<_> = restored_view.iter().filter(|o| o.2).collect();
    assert_eq!(paid.len(), payouts_total);
    assert!(paid.iter().any(|o| o.0 == payout.to_bytes()));
    assert!(paid.iter().all(|o| o.3.is_none()));

    // ---- Spend the payout on the restored ledger (sealed + validated). ----
    let payout_utxo = dst.get_utxo(&payout).unwrap().unwrap();
    let tx = spend(&dst, &owner, &other, &payout_utxo, FEE);
    dst.verify_transaction(&tx).unwrap();
    dst.verify_ring_members(&tx).unwrap();
    // The same signed spend is valid against the source: the restore changed
    // nothing the spend depends on.
    src.verify_transaction(&tx).unwrap();
    let image = tx.inputs.clsag()[0].key_image;
    let spent_in = mine(&dst, &other, vec![tx.clone()]);
    let after = dst.discover_wallet_outputs(&owner).unwrap();
    let spent = after.iter().find(|o| o.utxo.id == payout).unwrap();
    assert_eq!(spent.key_image, image);
    assert_eq!(spent.spent_at, Some(spent_in.height()));
    // Replay is rejected on the restored ledger.
    assert!(dst.verify_transaction(&tx).is_err());
    let error = dst.recover_wallet_input(&owner, &payout_utxo).unwrap_err();
    assert!(error.to_string().contains("already spent"), "{error}");
    // And the restored chain keeps producing V2 blocks.
    mine(&dst, &other, vec![]);
}
