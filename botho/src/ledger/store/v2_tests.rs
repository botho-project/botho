//! `botho.ledger.2` production schema tests (#1286 P2).
use super::{
    v2::{CONTEXTS_TABLE, LEDGER_SCHEMA_V2, SCHEMA_KEY},
    *,
};
use crate::{
    block::LotteryOutput,
    ledger::{
        lottery_v2::{self, DerivationReads, StoredContext},
        UtxoSnapshot,
    },
};
use bth_transaction_clsag::lottery_v2::{Context, Outpoint, Record};
use tempfile::TempDir;

const ALL_TABLES: [&str; 9] = [
    "blocks",
    "meta",
    "utxos",
    "address_index",
    "key_images",
    "tx_index",
    "cluster_wealth",
    "bridge_import_clusters",
    CONTEXTS_TABLE,
];
type Rows = Vec<(Vec<u8>, Vec<u8>)>;

fn tables(env: &Env) -> Vec<(&'static str, Rows)> {
    let txn = env.read_txn().unwrap();
    ALL_TABLES
        .iter()
        .map(|name| {
            let rows = match env.open_database::<Bytes, Bytes>(&txn, Some(name)).unwrap() {
                Some(db) => db
                    .iter(&txn)
                    .unwrap()
                    .map(|r| {
                        let (k, v) = r.unwrap();
                        (k.to_vec(), v.to_vec())
                    })
                    .collect(),
                None => vec![],
            };
            (*name, rows)
        })
        .collect()
}

fn golden_block() -> Block {
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v1-writer-effects.json"
    ))
    .unwrap();
    bincode::deserialize(&hex::decode(golden["block_hex"].as_str().unwrap()).unwrap()).unwrap()
}

fn output(seed: u8) -> TxOutput {
    TxOutput {
        amount: 1_000 + seed as u64,
        target_key: [seed; 32],
        public_key: [seed.wrapping_add(1); 32],
        e_memo: None,
        cluster_tags: ClusterTagVector::empty(),
        kem_ciphertext: None,
    }
}

/// A canonical, non-zero scalar tweak.
fn tweak() -> [u8; 32] {
    let mut t = [0u8; 32];
    t[0] = 7;
    t
}

/// Block 2 on top of `parent`: one stub transaction with two outputs and one
/// V2 payout (with its record) to `parent`'s coinbase. Storage fixture only:
/// it is applied through the shared writer, never through V1 validation.
fn payout_block(parent: &Block) -> (Block, Vec<Record>) {
    let mut block = parent.clone();
    block.header.height = parent.height() + 1;
    block.header.prev_block_hash = parent.hash();
    let mut tx = BothoTransaction::new_stub_with_fee(0);
    tx.outputs = vec![output(0x31), output(0x32)];
    block.transactions = vec![tx];
    let record = Record {
        ordinal: 0,
        winner: Outpoint {
            hash: parent.hash(),
            index: 0,
        },
        amount: 77,
        target: [0x41; 32],
        public_key: [0x42; 32],
        ciphertext: None,
        context: Context {
            base_index: 0,
            tweak: tweak(),
        },
    };
    block.lottery_outputs = vec![LotteryOutput {
        winner_tx_hash: parent.hash(),
        winner_output_index: 0,
        payout: record.amount,
        target_key: record.target,
        public_key: record.public_key,
        kem_ciphertext: None,
    }];
    (block, vec![record])
}

fn apply_v2(
    ledger: &Ledger,
    block: &Block,
    records: &[Record],
    after: &mut dyn FnMut() -> Result<(), LedgerError>,
) -> Result<(), LedgerError> {
    let state = ledger.get_chain_state().unwrap();
    let bytes = bincode::serialize(block).unwrap();
    let mut txn = ledger.env.write_txn().unwrap();
    ledger.write_tables().block_effects(
        &mut txn,
        block,
        &state,
        0,
        None,
        &bytes,
        writer::Representation::Production {
            contexts: ledger.contexts_db,
            records,
        },
        &mut |_| Ok(()),
        after,
    )?;
    txn.commit().unwrap();
    Ok(())
}

/// Ledger with the golden V1 block applied through the live add_block path.
fn ledger_at_height_one(dir: &Path) -> (Ledger, Block) {
    let ledger = Ledger::open(dir).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    let block = golden_block();
    ledger.add_block(&block).unwrap();
    (ledger, block)
}

#[test]
fn fresh_init_writes_marker_and_empty_contexts_table() {
    let dir = TempDir::new().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let txn = ledger.env.read_txn().unwrap();
    assert_eq!(
        ledger.meta_db.get(&txn, SCHEMA_KEY).unwrap(),
        Some(LEDGER_SCHEMA_V2)
    );
    assert_eq!(LEDGER_SCHEMA_V2, b"botho.ledger.2");
    assert!(ledger.contexts_db.is_empty(&txn).unwrap());
    // Genesis creates no outputs, so it has no contexts.
    assert_eq!(ledger.utxo_db.len(&txn).unwrap(), 0);
    drop(txn);
    let before = tables(&ledger.env);
    drop(ledger);
    let reopened = Ledger::open(dir.path()).unwrap();
    assert_eq!(tables(&reopened.env), before);
}

#[test]
fn contexts_round_trip_through_reopen() {
    let dir = TempDir::new().unwrap();
    let (ledger, b1) = ledger_at_height_one(dir.path());
    let (b2, records) = payout_block(&b1);
    apply_v2(&ledger, &b2, &records, &mut || Ok(())).unwrap();
    let tx_hash = b2.transactions[0].hash();
    let expected = vec![
        (
            UtxoId::new(b1.hash(), 0),
            StoredContext::Direct {
                height: 1,
                base_index: 0,
            },
        ),
        (
            UtxoId::new(b2.hash(), 0),
            StoredContext::Direct {
                height: 2,
                base_index: 0,
            },
        ),
        (
            UtxoId::new(tx_hash, 0),
            StoredContext::Direct {
                height: 2,
                base_index: 0,
            },
        ),
        (
            UtxoId::new(tx_hash, 1),
            StoredContext::Direct {
                height: 2,
                base_index: 1,
            },
        ),
        (
            UtxoId::new(b2.hash(), 1),
            StoredContext::Lottery {
                height: 2,
                ordinal: 0,
                context: records[0].context.clone(),
            },
        ),
    ];
    let check = |ledger: &Ledger| {
        let txn = ledger.env.read_txn().unwrap();
        assert_eq!(
            ledger.utxo_db.len(&txn).unwrap(),
            ledger.contexts_db.len(&txn).unwrap()
        );
        assert_eq!(ledger.contexts_db.len(&txn).unwrap(), expected.len() as u64);
        drop(txn);
        for (id, context) in &expected {
            assert_eq!(&ledger.get_derivation_context(id).unwrap(), context);
        }
        assert!(matches!(
            ledger.get_derivation_context(&UtxoId::new([0xee; 32], 0)),
            Err(LedgerError::MissingContext(_))
        ));
        let txn = ledger.env.read_txn().unwrap();
        let view = ledger.production_view(&txn);
        for (id, context) in &expected {
            let (utxo, via_view) = view.output_context(id).unwrap();
            assert_eq!(&via_view, context);
            assert_eq!(utxo.id, *id);
        }
        // The payout carries the V2 record's envelope, not the winner's key.
        let (payout, _) = view.output_context(&UtxoId::new(b2.hash(), 1)).unwrap();
        assert_eq!(payout.output.target_key, records[0].target);
        assert!(matches!(
            view.output_context(&UtxoId::new([0xee; 32], 0)),
            Err(LedgerError::MissingContext(_))
        ));
    };
    check(&ledger);
    let before = tables(&ledger.env);
    drop(ledger);
    let reopened = Ledger::open(dir.path()).unwrap();
    assert_eq!(tables(&reopened.env), before);
    check(&reopened);
}

#[test]
fn abort_at_every_write_leaves_all_tables_consistent() {
    // Count the writes of a successful application first.
    let dir = TempDir::new().unwrap();
    let (ledger, b1) = ledger_at_height_one(dir.path());
    let (b2, records) = payout_block(&b1);
    let mut writes = 0;
    apply_v2(&ledger, &b2, &records, &mut || {
        writes += 1;
        Ok(())
    })
    .unwrap();
    // block, 3 outputs x (utxo, context, address index), tx index, payout
    // (utxo, context, address index) and 5 metadata writes at least.
    assert!(writes >= 1 + 3 * 3 + 1 + 3 + 5, "writes={writes}");
    drop(ledger);
    for fail in 1..=writes {
        let dir = TempDir::new().unwrap();
        let (ledger, b1) = ledger_at_height_one(dir.path());
        let before = tables(&ledger.env);
        let mut at = 0;
        let result = apply_v2(&ledger, &b2_for(&b1), &records, &mut || {
            at += 1;
            if at == fail {
                Err(LedgerError::InconsistentRecord(
                    "injected write abort".into(),
                ))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err(), "fail={fail}");
        assert_eq!(tables(&ledger.env), before, "fail={fail}");
        drop(ledger);
        let reopened = Ledger::open(dir.path()).unwrap();
        assert_eq!(tables(&reopened.env), before, "fail={fail}");
        let txn = reopened.env.read_txn().unwrap();
        assert_eq!(
            reopened.utxo_db.len(&txn).unwrap(),
            reopened.contexts_db.len(&txn).unwrap()
        );
    }
    fn b2_for(b1: &Block) -> Block {
        payout_block(b1).0
    }
}

#[test]
fn production_writer_refuses_overwriting_an_existing_outpoint_context() {
    let dir = TempDir::new().unwrap();
    let (ledger, b1) = ledger_at_height_one(dir.path());
    let before = tables(&ledger.env);
    // Re-applying the same block would overwrite the coinbase row/context.
    assert!(matches!(
        apply_v2(&ledger, &b1, &[], &mut || Ok(())),
        Err(LedgerError::InconsistentRecord(_))
    ));
    assert_eq!(tables(&ledger.env), before);
}

#[test]
fn live_add_block_refuses_v1_lottery_payouts_before_any_write() {
    let dir = TempDir::new().unwrap();
    let (ledger, b1) = ledger_at_height_one(dir.path());
    let before = tables(&ledger.env);
    let (b2, _) = payout_block(&b1);
    let err = ledger.add_block(&b2).unwrap_err();
    assert!(
        matches!(&err, LedgerError::UnsupportedSchema(m) if m.contains("V1 lottery payout")),
        "{err}"
    );
    assert_eq!(tables(&ledger.env), before);
}

#[test]
fn strict_context_reads_reject_missing_corrupt_and_disagreeing_rows() {
    let dir = TempDir::new().unwrap();
    let (ledger, b1) = ledger_at_height_one(dir.path());
    let id = UtxoId::new(b1.hash(), 0);
    let key = id.to_bytes();
    let good = ledger
        .get_derivation_context(&id)
        .unwrap()
        .encode()
        .unwrap();
    let put = |value: &[u8]| {
        let mut txn = ledger.env.write_txn().unwrap();
        ledger.contexts_db.put(&mut txn, &key, value).unwrap();
        txn.commit().unwrap();
    };
    // Truncated / trailing / non-zero Direct tweak.
    put(&good[..good.len() - 1]);
    assert!(matches!(
        ledger.get_derivation_context(&id),
        Err(LedgerError::StorageEncoding(_))
    ));
    let mut bad = good.clone();
    bad.push(0);
    put(&bad);
    assert!(ledger.get_derivation_context(&id).is_err());
    // Wrong creation height.
    put(&StoredContext::Direct {
        height: 9,
        base_index: 0,
    }
    .encode()
    .unwrap());
    assert!(matches!(
        ledger.get_derivation_context(&id),
        Err(LedgerError::InconsistentRecord(_))
    ));
    // Direct base index must be the output index.
    put(&StoredContext::Direct {
        height: 1,
        base_index: 3,
    }
    .encode()
    .unwrap());
    assert!(matches!(
        ledger.get_derivation_context(&id),
        Err(LedgerError::InconsistentRecord(_))
    ));
    // A coinbase is not a payout (ordinal 0 is output index 1).
    put(&StoredContext::Lottery {
        height: 1,
        ordinal: 0,
        context: Context {
            base_index: 0,
            tweak: tweak(),
        },
    }
    .encode()
    .unwrap());
    assert!(matches!(
        ledger.get_derivation_context(&id),
        Err(LedgerError::InconsistentRecord(_))
    ));
    put(&good);
    ledger.get_derivation_context(&id).unwrap();
    // Missing context: explicit error, never a default.
    let mut txn = ledger.env.write_txn().unwrap();
    ledger.contexts_db.delete(&mut txn, &key).unwrap();
    txn.commit().unwrap();
    assert!(matches!(
        ledger.get_derivation_context(&id),
        Err(LedgerError::MissingContext(_))
    ));
    let txn = ledger.env.read_txn().unwrap();
    assert!(matches!(
        ledger.production_view(&txn).output_context(&id),
        Err(LedgerError::MissingContext(_))
    ));
    drop(txn);
    // Context without its output.
    put(&good);
    let mut txn = ledger.env.write_txn().unwrap();
    ledger.utxo_db.delete(&mut txn, &key).unwrap();
    txn.commit().unwrap();
    assert!(matches!(
        ledger.get_derivation_context(&id),
        Err(LedgerError::InconsistentRecord(_))
    ));
}

/// Build an unmarked store holding V1-shaped rows, exactly as a pre-P2
/// binary leaves it (no `storage_schema`, no `derivation_contexts`).
fn write_v1_store(path: &Path, table: &str) {
    std::fs::create_dir_all(path).unwrap();
    let env = unsafe {
        EnvOpenOptions::new()
            .max_dbs(9)
            .map_size(16 * 1024 * 1024)
            .open(path)
    }
    .unwrap();
    let mut txn = env.write_txn().unwrap();
    for name in &ALL_TABLES[..8] {
        let db: Database<Bytes, Bytes> = env.create_database(&mut txn, Some(name)).unwrap();
        if *name == table {
            db.put(&mut txn, b"k", b"v").unwrap();
        }
    }
    txn.commit().unwrap();
}

#[test]
fn unmarked_nonempty_v1_store_is_refused_untouched_with_wipe_guidance() {
    for table in &ALL_TABLES[..8] {
        let dir = TempDir::new().unwrap();
        write_v1_store(dir.path(), table);
        let data = std::fs::read(dir.path().join("data.mdb")).unwrap();
        let err = match Ledger::open(dir.path()) {
            Err(e) => e,
            Ok(_) => panic!("V1 store with a {table} row opened"),
        };
        let message = err.to_string();
        assert!(
            matches!(err, LedgerError::UnsupportedSchema(_)),
            "{message}"
        );
        assert!(message.contains("unmarked V1 ledger store"), "{message}");
        assert!(message.contains("wipe/reset"), "{message}");
        assert!(
            message.contains(&dir.path().display().to_string()),
            "{message}"
        );
        assert_eq!(std::fs::read(dir.path().join("data.mdb")).unwrap(), data);
        // Still refused on a second attempt (nothing was half-initialized).
        assert!(Ledger::open(dir.path()).is_err());
    }
}

#[test]
fn unmarked_store_with_only_empty_tables_initializes_as_v2() {
    let dir = TempDir::new().unwrap();
    write_v1_store(dir.path(), "none");
    let ledger = Ledger::open(dir.path()).unwrap();
    let txn = ledger.env.read_txn().unwrap();
    assert_eq!(
        ledger.meta_db.get(&txn, SCHEMA_KEY).unwrap(),
        Some(LEDGER_SCHEMA_V2)
    );
}

#[test]
fn foreign_markers_and_missing_contexts_table_are_refused() {
    for marker in [
        &b"botho.ledger.1"[..],
        b"botho.ledger.3",
        b"botho.experimental.lottery-v2.storage.2",
        b"",
    ] {
        let dir = TempDir::new().unwrap();
        {
            let ledger = Ledger::open(dir.path()).unwrap();
            let mut txn = ledger.env.write_txn().unwrap();
            ledger.meta_db.put(&mut txn, SCHEMA_KEY, marker).unwrap();
            txn.commit().unwrap();
        }
        let data = std::fs::read(dir.path().join("data.mdb")).unwrap();
        assert!(matches!(
            Ledger::open(dir.path()),
            Err(LedgerError::UnsupportedSchema(_))
        ));
        assert_eq!(std::fs::read(dir.path().join("data.mdb")).unwrap(), data);
    }
    // Marked V2 but the contexts table is missing: refused, not recreated.
    let dir = TempDir::new().unwrap();
    write_v1_store(dir.path(), "none");
    {
        let env = unsafe {
            EnvOpenOptions::new()
                .max_dbs(9)
                .map_size(16 * 1024 * 1024)
                .open(dir.path())
        }
        .unwrap();
        let mut txn = env.write_txn().unwrap();
        let meta: Database<Bytes, Bytes> = env.create_database(&mut txn, Some("meta")).unwrap();
        meta.put(&mut txn, SCHEMA_KEY, LEDGER_SCHEMA_V2).unwrap();
        txn.commit().unwrap();
    }
    let data = std::fs::read(dir.path().join("data.mdb")).unwrap();
    assert!(matches!(
        Ledger::open(dir.path()),
        Err(LedgerError::InconsistentRecord(_))
    ));
    assert_eq!(std::fs::read(dir.path().join("data.mdb")).unwrap(), data);
}

#[test]
fn snapshots_fail_closed_without_writes() {
    let dir = TempDir::new().unwrap();
    let (ledger, _) = ledger_at_height_one(dir.path());
    let before = tables(&ledger.env);
    assert!(matches!(
        ledger.create_snapshot(),
        Err(LedgerError::UnsupportedSchema(_))
    ));
    let file = dir.path().join("out.snapshot");
    assert!(matches!(
        ledger.write_snapshot_to_file(&file),
        Err(LedgerError::UnsupportedSchema(_))
    ));
    assert!(!file.exists());
    let snapshot = UtxoSnapshot::new(
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
    assert!(matches!(
        ledger.load_from_snapshot(&snapshot, None),
        Err(LedgerError::UnsupportedSchema(_))
    ));
    std::fs::write(&file, b"not a snapshot").unwrap();
    let err = ledger.load_snapshot_from_file(&file, None).unwrap_err();
    assert!(
        matches!(&err, LedgerError::UnsupportedSchema(m) if m.contains("botho.ledger.2")),
        "{err}"
    );
    assert_eq!(tables(&ledger.env), before);
}

#[test]
fn fixture_seeding_writes_direct_contexts_for_every_row() {
    let dir = TempDir::new().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let utxos: Vec<_> = (0..4u8)
        .map(|i| Utxo {
            id: UtxoId::new([i + 1; 32], i as u32),
            output: output(i),
            created_at: 3,
        })
        .collect();
    let snapshot = UtxoSnapshot::new(
        10,
        [0; 32],
        ChainState {
            height: 10,
            ..ChainState::default()
        },
        utxos.clone(),
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        ledger.seed_fixture_snapshot_unvalidated(&snapshot).unwrap(),
        4
    );
    for u in &utxos {
        assert_eq!(
            ledger.get_derivation_context(&u.id).unwrap(),
            StoredContext::Direct {
                height: 3,
                base_index: u.id.output_index,
            }
        );
    }
    assert_eq!(ledger.fixture_utxos().unwrap().len(), 4);
}

#[test]
fn production_view_runs_shared_v2_derive_and_validate() {
    use rand::{rngs::StdRng, SeedableRng};
    let dir = TempDir::new().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    let account = bth_account_keys::AccountKey::random(&mut StdRng::seed_from_u64(1639));
    let state = ledger.get_chain_state().unwrap();
    let genesis = ledger.get_block(0).unwrap();
    let template = Block::new_template_with_txs(
        &genesis,
        &account.default_subaddress(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        vec![],
    );
    let txn = ledger.env.read_txn().unwrap();
    let view = ledger.production_view(&txn);
    assert_eq!(view.genesis_hash().unwrap(), genesis.hash());
    assert_eq!(view.lottery_pool().unwrap(), 0);
    let (block, records) = lottery_v2::derive(&view, template).unwrap();
    assert!(records.is_empty());
    assert_eq!(
        block.header.tx_root,
        lottery_v2::root(&block, &records).unwrap()
    );
    lottery_v2::validate(&view, &block, &records, &state).unwrap();
    // The shared validator recomputes the V2 body root: a V1 root is refused.
    let mut v1_root = block.clone();
    v1_root.header.tx_root = Block::compute_tx_root(&v1_root.transactions);
    assert!(lottery_v2::validate(&view, &v1_root, &records, &state).is_err());
}

/// LMDB growth per derivation-context row (cf. #1536 map sizing). Uses
/// uniformly random 36-byte outpoint keys (tx hashes are hash outputs) and the
/// real 45-byte Direct encoding; reported via `--nocapture`.
#[test]
fn measure_lmdb_growth_per_context_row() {
    use rand::{rngs::StdRng, RngExt, SeedableRng};
    let dir = TempDir::new().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let mut rng = StdRng::seed_from_u64(1536);
    const ROWS: usize = 100_000;
    let direct = StoredContext::Direct {
        height: 123_456,
        base_index: 1,
    }
    .encode()
    .unwrap();
    let lottery = StoredContext::Lottery {
        height: 123_456,
        ordinal: 0,
        context: Context {
            base_index: 1,
            tweak: tweak(),
        },
    }
    .encode()
    .unwrap();
    assert_eq!((direct.len(), lottery.len()), (45, 49));
    let size_before = std::fs::metadata(dir.path().join("data.mdb"))
        .unwrap()
        .len();
    // Insert in block-sized batches, like the writer does.
    for chunk in 0..(ROWS / 1_000) {
        let mut txn = ledger.env.write_txn().unwrap();
        for _ in 0..1_000 {
            let mut key = [0u8; 36];
            rng.fill(&mut key[..32]);
            key[32..].copy_from_slice(&rng.random_range(0u32..4).to_le_bytes());
            ledger.contexts_db.put(&mut txn, &key, &direct).unwrap();
        }
        txn.commit().unwrap();
        let _ = chunk;
    }
    let txn = ledger.env.read_txn().unwrap();
    let stat = ledger.contexts_db.stat(&txn).unwrap();
    let pages = stat.branch_pages + stat.leaf_pages + stat.overflow_pages;
    let bytes = pages as f64 * stat.page_size as f64;
    let per_row = bytes / stat.entries as f64;
    let size_after = std::fs::metadata(dir.path().join("data.mdb"))
        .unwrap()
        .len();
    eprintln!(
        "derivation_contexts growth: rows={} value={}B key=36B page_size={} depth={} \
         branch={} leaf={} overflow={} => {:.1} B/row in table pages; data.mdb {} -> {} \
         (+{:.1} B/row incl. freelist/COW slack); 1 GiB map ~= {:.1}M rows",
        stat.entries,
        direct.len(),
        stat.page_size,
        stat.depth,
        stat.branch_pages,
        stat.leaf_pages,
        stat.overflow_pages,
        per_row,
        size_before,
        size_after,
        (size_after - size_before) as f64 / ROWS as f64,
        (1u64 << 30) as f64 / per_row / 1e6,
    );
    assert_eq!(stat.entries, ROWS);
    // key + value + 8 B LMDB node header + 2 B slot = 91 B at 100 % fill;
    // random inserts settle near 2/3 fill.
    assert!(per_row > 91.0 && per_row < 200.0, "{per_row}");
}
