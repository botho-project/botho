use super::*;
use crate::{block::LotteryOutput, ledger::Ledger, transaction::Transaction};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_transaction_clsag::lottery_v2::{self, Award, Domain, Outpoint, Source};
use tempfile::TempDir;

fn base() -> Envelope {
    let mut block = Block::genesis();
    let mut tx = Transaction::new_stub_with_fee(0);
    let mut output = block.minting_tx.to_tx_output();
    output.target_key =
        RistrettoPublic::from(&RistrettoPrivate::from(Scalar::from(7u64))).to_bytes();
    output.public_key =
        RistrettoPublic::from(&RistrettoPrivate::from(Scalar::from(11u64))).to_bytes();
    output.kem_ciphertext = Some(vec![23; lottery_v2::KEM_BYTES]);
    output.amount = 100;
    tx.outputs = vec![output.clone(), output.clone(), output];
    block.transactions = vec![tx];
    block.header.tx_root = Block::compute_tx_root(&block.transactions);
    Envelope {
        block,
        records: vec![],
    }
}
fn direct_id(e: &Envelope) -> UtxoId {
    UtxoId::new(e.block.transactions[0].hash(), 2)
}
fn next(previous: &Envelope, source: &Utxo, context: Context, count: usize) -> Envelope {
    let mut block = Block::genesis();
    block.header.height = previous.block.height() + 1;
    block.header.prev_block_hash = previous.block.hash();
    block.minting_tx.block_height = block.header.height;
    let winner = Outpoint {
        hash: source.id.tx_hash,
        index: source.id.output_index,
    };
    let awards = vec![
        Award {
            winner: winner.clone(),
            amount: 5
        };
        count
    ];
    let manifest = lottery_v2::manifest(&awards).unwrap();
    let mut records = vec![];
    for ordinal in 0..count {
        let derived = lottery_v2::derive(
            &Source {
                outpoint: winner.clone(),
                target: source.output.target_key,
                context: context.clone(),
            },
            &Domain {
                genesis: Block::genesis().hash(),
                parent: block.header.prev_block_hash,
                height: block.height(),
                ordinary_root: [0; 32],
                manifest,
                ordinal: ordinal as u32,
                amount: 5,
            },
        )
        .unwrap();
        let record = Record {
            ordinal: ordinal as u32,
            winner: winner.clone(),
            amount: 5,
            target: derived.target,
            public_key: source.output.public_key,
            ciphertext: source.output.kem_ciphertext.clone(),
            context: derived.context,
        };
        block.lottery_outputs.push(LotteryOutput::from_utxo_id(
            source.id.to_bytes(),
            5,
            record.target,
            record.public_key,
            record.ciphertext.clone(),
        ));
        records.push(record);
    }
    block.header.tx_root = lottery_v2::payout_root(&records).unwrap(); // Fixture identity only; not a consensus body root.
    Envelope { block, records }
}
fn lookup(store: &ExperimentalStore, id: UtxoId) -> (Utxo, StoredContext) {
    let txn = store.env.read_txn().unwrap();
    store.read(&txn, &id).unwrap()
}
fn snapshot(store: &ExperimentalStore) -> Vec<(String, Vec<(Vec<u8>, Vec<u8>)>)> {
    let txn = store.env.read_txn().unwrap();
    [
        "blocks",
        "meta",
        "utxos",
        "derivation_contexts",
        "address_index",
        "key_images",
        "tx_index",
        "cluster_wealth",
        "bridge_import_clusters",
    ]
    .into_iter()
    .map(|name| {
        let db: Database<Bytes, Bytes> =
            store.env.open_database(&txn, Some(name)).unwrap().unwrap();
        let rows = db
            .iter(&txn)
            .unwrap()
            .map(|r| {
                let (k, v) = r.unwrap();
                (k.to_vec(), v.to_vec())
            })
            .collect();
        (name.to_string(), rows)
    })
    .collect()
}

#[test]
fn persisted_nonzero_repeated_and_nested_contexts_reopen() {
    let dir = TempDir::new().unwrap();
    let store = ExperimentalStore::open_fixture(dir.path(), true).unwrap();
    let base = base();
    store.persist(&base, || Ok(())).unwrap();
    let (source, c) = lookup(&store, direct_id(&base));
    assert_eq!(c.derivation().base_index, 2);
    let first = next(&base, &source, c.derivation(), 2);
    store.persist(&first, || Ok(())).unwrap();
    let (payout, c) = lookup(&store, UtxoId::new(first.block.hash(), 1));
    assert_eq!(payout.id.output_index, 1);
    assert_eq!(c.derivation().base_index, 2);
    let second = next(&first, &payout, c.derivation(), 1);
    store.persist(&second, || Ok(())).unwrap();
    let id = UtxoId::new(second.block.hash(), 1);
    let (nested, nc) = lookup(&store, id);
    assert_eq!(nc.derivation(), second.records[0].context);
    assert_ne!(nested.output.target_key, payout.output.target_key);
    assert_eq!(second.records[0].winner.hash, first.block.hash());
    assert_eq!(store.all().unwrap().len(), 9);
    let before = snapshot(&store);
    drop(store);
    let reopened = ExperimentalStore::open_fixture(dir.path(), false).unwrap();
    assert_eq!(snapshot(&reopened), before);
    assert_eq!(lookup(&reopened, id).1, nc);
    assert_eq!(reopened.all().unwrap().len(), 9);
}

#[test]
fn abort_every_write_stage_and_conflicts_leave_all_tables_unchanged() {
    let dir = TempDir::new().unwrap();
    let mut store = ExperimentalStore::open_fixture(dir.path(), true).unwrap();
    let base = base();
    store.persist(&base, || Ok(())).unwrap();
    let (source, c) = lookup(&store, direct_id(&base));
    let next = next(&base, &source, c.derivation(), 2);
    let before = snapshot(&store);
    // Count actual shared writes using a separate successful fixture store.
    let probe_dir = TempDir::new().unwrap();
    let probe = ExperimentalStore::open_fixture(probe_dir.path(), true).unwrap();
    probe.persist(&base, || Ok(())).unwrap();
    let mut stages = 0;
    probe
        .persist(&next, || {
            stages += 1;
            Ok(())
        })
        .unwrap();
    assert!(stages > 8); // Includes address/accounting effects beyond the old storage-only path.
    for stop in 1..=stages {
        let mut writes = 0;
        assert!(store
            .persist(&next, || {
                writes += 1;
                if writes == stop {
                    Err(inconsistent("injected abort"))
                } else {
                    Ok(())
                }
            })
            .is_err());
        assert_eq!(writes, stop);
        assert_eq!(snapshot(&store), before);
        drop(store);
        store = ExperimentalStore::open_fixture(dir.path(), false).unwrap();
        assert_eq!(snapshot(&store), before);
    }
    assert!(store.persist(&base, || Ok(())).is_err());
    assert_eq!(snapshot(&store), before);
    let mut conflicting = next.clone();
    conflicting.block.transactions = base.block.transactions.clone();
    assert!(store.persist(&conflicting, || Ok(())).is_err());
    assert_eq!(snapshot(&store), before);
    store.persist(&next, || Ok(())).unwrap();
}

#[test]
fn default_and_experimental_opens_cannot_cross_schemas() {
    let dir = TempDir::new().unwrap();
    let v1 = Ledger::open(dir.path()).unwrap();
    let genesis = v1.get_block(0).unwrap();
    let original = bincode::serialize(&genesis).unwrap();
    drop(v1);
    let data = std::fs::read(dir.path().join("data.mdb")).unwrap();
    assert!(ExperimentalStore::open_fixture(dir.path(), true).is_err());
    assert!(ExperimentalStore::open_fixture(dir.path(), false).is_err());
    assert_eq!(std::fs::read(dir.path().join("data.mdb")).unwrap(), data);
    let v1 = Ledger::open(dir.path()).unwrap();
    assert_eq!(
        bincode::serialize(&v1.get_block(0).unwrap()).unwrap(),
        original
    );
    drop(v1);
    let other = TempDir::new().unwrap();
    let store = ExperimentalStore::open_fixture(other.path(), true).unwrap();
    let expected = snapshot(&store);
    drop(store);
    let data = std::fs::read(other.path().join("data.mdb")).unwrap();
    assert!(matches!(
        Ledger::open(other.path()),
        Err(LedgerError::UnsupportedSchema(_))
    ));
    assert_eq!(std::fs::read(other.path().join("data.mdb")).unwrap(), data);
    let store = ExperimentalStore::open_fixture(other.path(), false).unwrap();
    assert_eq!(snapshot(&store), expected);
}

#[test]
fn strict_envelope_and_context_decoders() {
    let e = base();
    let bytes = e.encode().unwrap();
    assert_eq!(Envelope::decode(&bytes).unwrap().encode().unwrap(), bytes);
    for end in 0..bytes.len() {
        assert!(Envelope::decode(&bytes[..end]).is_err());
    }
    let mut bad = bytes.clone();
    bad.push(0);
    assert!(Envelope::decode(&bad).is_err());
    bad = bytes;
    bad[0] ^= 1;
    assert!(Envelope::decode(&bad).is_err());
    let c = StoredContext::Lottery {
        height: 2,
        ordinal: 0,
        context: Context {
            base_index: 2,
            tweak: Scalar::ONE.to_bytes(),
        },
    };
    let bytes = c.encode().unwrap();
    for end in 0..bytes.len() {
        assert!(StoredContext::decode(&bytes[..end]).is_err());
    }
    let mut bad = bytes.clone();
    bad[0] = 1;
    assert!(StoredContext::decode(&bad).is_err());
    bad = bytes.clone();
    bad[13..45].fill(255);
    assert!(StoredContext::decode(&bad).is_err());
    bad = bytes;
    bad.push(0);
    assert!(StoredContext::decode(&bad).is_err());
}

#[test]
fn missing_corrupt_and_inconsistent_rows_are_never_skipped() {
    let dir = TempDir::new().unwrap();
    let store = ExperimentalStore::open_fixture(dir.path(), true).unwrap();
    let base = base();
    store.persist(&base, || Ok(())).unwrap();
    let id = direct_id(&base);
    let key = id.to_bytes();
    let (_, c) = lookup(&store, id);
    let good = c.encode().unwrap();
    let mutate = |value: Option<&[u8]>| {
        let mut txn = store.env.write_txn().unwrap();
        match value {
            Some(v) => store.contexts.put(&mut txn, &key, v).unwrap(),
            None => {
                store.contexts.delete(&mut txn, &key).unwrap();
            }
        }
        txn.commit().unwrap();
    };
    mutate(None);
    let txn = store.env.read_txn().unwrap();
    assert!(matches!(
        store.read(&txn, &id),
        Err(LedgerError::MissingContext(_))
    ));
    drop(txn);
    assert!(store.all().is_err());
    mutate(Some(&[255]));
    assert!(store.all().is_err());
    mutate(Some(
        &StoredContext::Direct {
            height: 0,
            base_index: 1,
        }
        .encode()
        .unwrap(),
    ));
    assert!(store.all().is_err());
    mutate(Some(&good));
    assert_eq!(store.all().unwrap().len(), 4);
    let mut wrong_height = base.clone();
    wrong_height.block.header.height = 1;
    let mut txn = store.env.write_txn().unwrap();
    store
        .blocks
        .put(&mut txn, &0, &wrong_height.encode().unwrap())
        .unwrap();
    txn.commit().unwrap();
    assert!(store.all().is_err());
    let mut txn = store.env.write_txn().unwrap();
    store.blocks.delete(&mut txn, &0).unwrap();
    txn.commit().unwrap();
    assert!(store.all().is_err());
}

#[test]
fn unknown_marker_and_missing_tables_fail_without_initialization() {
    let dir = TempDir::new().unwrap();
    let store = ExperimentalStore::open_fixture(dir.path(), true).unwrap();
    let mut txn = store.env.write_txn().unwrap();
    store.meta.put(&mut txn, SCHEMA_KEY, b"future").unwrap();
    txn.commit().unwrap();
    drop(store);
    let data = std::fs::read(dir.path().join("data.mdb")).unwrap();
    assert!(matches!(
        Ledger::open(dir.path()),
        Err(LedgerError::UnsupportedSchema(_))
    ));
    assert!(ExperimentalStore::open_fixture(dir.path(), false).is_err());
    assert_eq!(std::fs::read(dir.path().join("data.mdb")).unwrap(), data);
    let missing = TempDir::new().unwrap();
    assert!(ExperimentalStore::open_fixture(missing.path(), false).is_err());
    assert!(!missing.path().join("data.mdb").exists());
}

#[test]
fn single_payout_reads_check_immediate_source_context_without_recursion() {
    let dir = TempDir::new().unwrap();
    let store = ExperimentalStore::open_fixture(dir.path(), true).unwrap();
    let base = base();
    store.persist(&base, || Ok(())).unwrap();
    let source_id = direct_id(&base);
    let (source, c) = lookup(&store, source_id);
    let payout = next(&base, &source, c.derivation(), 1);
    store.persist(&payout, || Ok(())).unwrap();
    let payout_id = UtxoId::new(payout.block.hash(), 1);
    let source_bytes = bincode::serialize(&source).unwrap();
    let context_bytes = c.encode().unwrap();
    let read_error = || {
        let txn = store.env.read_txn().unwrap();
        assert!(store.read(&txn, &payout_id).is_err());
    };
    for mode in 0..5 {
        let mut txn = store.env.write_txn().unwrap();
        store
            .utxos
            .put(&mut txn, &source_id.to_bytes(), &source_bytes)
            .unwrap();
        store
            .contexts
            .put(&mut txn, &source_id.to_bytes(), &context_bytes)
            .unwrap();
        match mode {
            0 => {
                store
                    .contexts
                    .delete(&mut txn, &source_id.to_bytes())
                    .unwrap();
            }
            1 => {
                let mut changed = source.clone();
                changed.id.output_index = 1;
                store
                    .utxos
                    .put(
                        &mut txn,
                        &source_id.to_bytes(),
                        &bincode::serialize(&changed).unwrap(),
                    )
                    .unwrap();
            }
            2 => {
                store
                    .contexts
                    .put(
                        &mut txn,
                        &source_id.to_bytes(),
                        &StoredContext::Direct {
                            height: 1,
                            base_index: 2,
                        }
                        .encode()
                        .unwrap(),
                    )
                    .unwrap();
            }
            3 => {
                let mut changed = source.clone();
                changed.created_at = 1;
                store
                    .utxos
                    .put(
                        &mut txn,
                        &source_id.to_bytes(),
                        &bincode::serialize(&changed).unwrap(),
                    )
                    .unwrap();
                store
                    .contexts
                    .put(
                        &mut txn,
                        &source_id.to_bytes(),
                        &StoredContext::Direct {
                            height: 1,
                            base_index: 2,
                        }
                        .encode()
                        .unwrap(),
                    )
                    .unwrap();
            }
            4 => {
                store
                    .contexts
                    .put(
                        &mut txn,
                        &source_id.to_bytes(),
                        &StoredContext::Direct {
                            height: 0,
                            base_index: 1,
                        }
                        .encode()
                        .unwrap(),
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        txn.commit().unwrap();
        read_error();
    }
}

#[test]
fn mismatched_envelope_or_context_reference_is_rejected() {
    let dir = TempDir::new().unwrap();
    let store = ExperimentalStore::open_fixture(dir.path(), true).unwrap();
    let base = base();
    store.persist(&base, || Ok(())).unwrap();
    let (source, c) = lookup(&store, direct_id(&base));
    let payout = next(&base, &source, c.derivation(), 1);
    let before = snapshot(&store);
    let mut bad = payout.clone();
    bad.records[0].amount += 1;
    assert!(store.persist(&bad, || Ok(())).is_err());
    assert_eq!(snapshot(&store), before);
    store.persist(&payout, || Ok(())).unwrap();
    let id = UtxoId::new(payout.block.hash(), 1);
    let (stored, context) = lookup(&store, id);
    let good = context.encode().unwrap();
    let mut bad = good.clone();
    bad[13] ^= 1; // canonical but inconsistent cumulative tweak
    let mut txn = store.env.write_txn().unwrap();
    store.contexts.put(&mut txn, &id.to_bytes(), &bad).unwrap();
    txn.commit().unwrap();
    let txn = store.env.read_txn().unwrap();
    assert!(store.read(&txn, &id).is_err());
    drop(txn);
    let mut txn = store.env.write_txn().unwrap();
    store.contexts.put(&mut txn, &id.to_bytes(), &good).unwrap();
    let mut changed = stored;
    changed.output.amount += 1;
    store
        .utxos
        .put(
            &mut txn,
            &id.to_bytes(),
            &bincode::serialize(&changed).unwrap(),
        )
        .unwrap();
    txn.commit().unwrap();
    assert!(store.all().is_err());
}

#[test]
fn marked_but_incomplete_schema_does_not_get_repaired_on_reopen() {
    let dir = TempDir::new().unwrap();
    // SAFETY: this test owns the temporary directory and closes this handle first.
    let env = unsafe {
        heed::EnvOpenOptions::new()
            .max_dbs(8)
            .map_size(1024 * 1024 * 1024)
            .open(dir.path())
    }
    .unwrap();
    let mut txn = env.write_txn().unwrap();
    let meta: Database<Bytes, Bytes> = env.create_database(&mut txn, Some("meta")).unwrap();
    meta.put(&mut txn, SCHEMA_KEY, SCHEMA).unwrap();
    txn.commit().unwrap();
    drop(env);
    let before = std::fs::read(dir.path().join("data.mdb")).unwrap();
    assert!(matches!(
        ExperimentalStore::open_fixture(dir.path(), false),
        Err(LedgerError::InconsistentRecord(_))
    ));
    assert_eq!(std::fs::read(dir.path().join("data.mdb")).unwrap(), before);
}

fn full_effects_fixture(
    store: &ExperimentalStore,
) -> (Envelope, Envelope, u128, EmissionStateUpdate) {
    use crate::transaction::ClsagRingInput;
    let mut base = base();
    let origin =
        bth_transaction_types::ClusterId(bth_cluster_tax::import_cluster_id_for_height(1).0);
    base.block.transactions[0].outputs[2].cluster_tags =
        bth_transaction_types::ClusterTagVector::single(origin);
    base.block.header.tx_root = Block::compute_tx_root(&base.block.transactions);
    store.persist(&base, || Ok(())).unwrap();
    let (source, c) = lookup(store, direct_id(&base));
    let mut e = next(&base, &source, c.derivation(), 2);
    e.block.minting_tx.reward = 1000;
    let mut tx = Transaction::new_stub_with_fee(50);
    // Raw storage-only input identity: never signed, verified, submitted, or
    // described as a valid consensus transaction. Exercises index writes only.
    tx.inputs = crate::transaction::TxInputs::new(vec![ClsagRingInput {
        ring: vec![],
        key_image: [0x62; 32],
        commitment_key_image: [0; 32],
        clsag_signature: vec![],
        pseudo_output_amount: 0,
    }]);
    let mut output = source.output.clone();
    output.amount = 50;
    tx.outputs = vec![output];
    e.block.transactions = vec![tx];
    e.block.lottery_summary.total_fees = 50;
    e.block.lottery_summary.amount_burned = 10;
    e.block.lottery_summary.pool_distributed = 10;
    let pool = u128::from(e.block.minting_tx.lottery_emission_share()) + 40 - 10;
    let emission = EmissionStateUpdate {
        difficulty: 77,
        total_tx: 123,
        epoch_tx: 8,
        epoch_emission: 999,
        epoch_burns: 10,
        current_reward: 1000,
    };
    (base, e, pool, emission)
}
fn meta_u128(store: &ExperimentalStore, key: &[u8]) -> u128 {
    let txn = store.env.read_txn().unwrap();
    u128::from_le_bytes(
        store
            .meta
            .get(&txn, key)
            .unwrap()
            .unwrap()
            .try_into()
            .unwrap(),
    )
}
#[test]
fn all_nine_tables_and_emission_are_atomic_at_every_actual_shared_write() {
    let probe_dir = TempDir::new().unwrap();
    let probe = ExperimentalStore::open_fixture(probe_dir.path(), true).unwrap();
    let (_, e, pool, emission) = full_effects_fixture(&probe);
    let mut count = 0;
    probe
        .persist_fixture_effects(&e, pool, Some(emission), || {
            count += 1;
            Ok(())
        })
        .unwrap();
    let accepted = snapshot(&probe);
    assert_eq!(accepted.len(), 9);
    assert!(accepted.iter().all(|(_, rows)| !rows.is_empty()));
    assert_eq!(meta_u128(&probe, b"total_mined"), 1000);
    assert_eq!(meta_u128(&probe, b"fees_burned"), 10);
    assert_eq!(meta_u128(&probe, b"lottery_pool"), pool);
    assert_eq!(
        pool + u128::from(e.block.lottery_summary.pool_distributed),
        u128::from(e.block.minting_tx.lottery_emission_share()) + 50 - 10
    );
    let txn = probe.env.read_txn().unwrap();
    for (key, value) in [
        (b"difficulty".as_slice(), 77u64),
        (b"total_tx", 123),
        (b"epoch_tx", 8),
        (b"epoch_emission", 999),
        (b"epoch_burns", 10),
        (b"current_reward", 1000),
    ] {
        assert_eq!(
            probe.meta.get(&txn, key).unwrap().unwrap(),
            value.to_le_bytes()
        );
    }
    assert_eq!(
        probe
            .tables
            .key_images_db
            .get(&txn, &[0x62; 32])
            .unwrap()
            .unwrap(),
        1u64.to_le_bytes()
    );
    let tx = e.block.transactions[0].hash();
    let mut location = 1u64.to_le_bytes().to_vec();
    location.extend(0u32.to_le_bytes());
    assert_eq!(
        probe.tables.tx_index_db.get(&txn, &tx).unwrap().unwrap(),
        location
    );
    let import = bth_cluster_tax::import_cluster_id_for_height(1)
        .0
        .to_le_bytes();
    assert_eq!(
        probe
            .tables
            .bridge_import_clusters_db
            .get(&txn, &import)
            .unwrap(),
        Some(&[][..])
    );
    // Tagged principal100 + ordinary50 + two inherited-tag payouts5 each.
    assert_eq!(
        probe
            .tables
            .cluster_wealth_db
            .get(&txn, &import)
            .unwrap()
            .unwrap(),
        160u128.to_le_bytes()
    );
    for ordinal in 0..2 {
        let id = UtxoId::new(e.block.hash(), ordinal + 1);
        assert_eq!(
            probe
                .tables
                .address_index_db
                .get(&txn, &e.records[ordinal as usize].target)
                .unwrap()
                .unwrap(),
            id.to_bytes()
        );
    }
    drop(txn);
    drop(probe);
    let reopened = ExperimentalStore::open_fixture(probe_dir.path(), false).unwrap();
    assert_eq!(snapshot(&reopened), accepted);
    drop(reopened);
    assert!(count > 25);
    eprintln!("full experimental shared-write stages: {count}");
    let dir = TempDir::new().unwrap();
    let mut store = ExperimentalStore::open_fixture(dir.path(), true).unwrap();
    let (_, e, pool, emission) = full_effects_fixture(&store);
    let before = snapshot(&store);
    for stop in 1..=count {
        let mut calls = 0;
        let result = store.persist_fixture_effects(&e, pool, Some(emission), || {
            calls += 1;
            if calls == stop {
                Err(inconsistent("injected shared write abort"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(calls, stop);
        assert_eq!(snapshot(&store), before);
        drop(store);
        store = ExperimentalStore::open_fixture(dir.path(), false).unwrap();
        assert_eq!(snapshot(&store), before);
    }
    store
        .persist_fixture_effects(&e, pool, Some(emission), || Ok(()))
        .unwrap();
    assert_eq!(snapshot(&store), accepted);
    // A later storage fixture reusing the key image must fail and roll back.
    let mut duplicate = e.clone();
    duplicate.block.header.height += 1;
    duplicate.block.header.prev_block_hash = e.block.hash();
    let before = snapshot(&store);
    assert!(
        matches!(store.persist_fixture_effects(&duplicate, pool, None, || Ok(())),
        Err(LedgerError::InvalidBlock(message)) if message == "Key image already spent (double-spend)")
    );
    assert_eq!(snapshot(&store), before);
}

#[test]
fn previous_storage_only_schema_is_not_upgraded() {
    let dir = TempDir::new().unwrap();
    let store = ExperimentalStore::open_fixture(dir.path(), true).unwrap();
    let mut txn = store.env.write_txn().unwrap();
    store
        .meta
        .put(
            &mut txn,
            SCHEMA_KEY,
            b"botho.experimental.lottery-v2.storage.1",
        )
        .unwrap();
    txn.commit().unwrap();
    drop(store);
    let before = std::fs::read(dir.path().join("data.mdb")).unwrap();
    assert!(matches!(
        ExperimentalStore::open_fixture(dir.path(), false),
        Err(LedgerError::UnsupportedSchema(_))
    ));
    assert_eq!(std::fs::read(dir.path().join("data.mdb")).unwrap(), before);
}

#[test]
fn optional_emission_none_preserves_supplied_counters_after_next_commit() {
    let dir = TempDir::new().unwrap();
    let store = ExperimentalStore::open_fixture(dir.path(), true).unwrap();
    let (_, e, pool, emission) = full_effects_fixture(&store);
    store
        .persist_fixture_effects(&e, pool, Some(emission), || Ok(()))
        .unwrap();
    let keys = [
        b"difficulty".as_slice(),
        b"total_tx",
        b"epoch_tx",
        b"epoch_emission",
        b"epoch_burns",
        b"current_reward",
    ];
    let values = |s: &ExperimentalStore| {
        let txn = s.env.read_txn().unwrap();
        keys.iter()
            .map(|key| s.meta.get(&txn, key).unwrap().unwrap().to_vec())
            .collect::<Vec<_>>()
    };
    let before = values(&store);
    let mut next = e.clone();
    next.block.header.height += 1;
    next.block.header.prev_block_hash = e.block.hash();
    next.block.minting_tx.block_height = next.block.height();
    next.block.transactions.clear();
    next.block.lottery_outputs.clear();
    next.records.clear();
    next.block.lottery_summary = Default::default();
    store
        .persist_fixture_effects(&next, pool, None, || Ok(()))
        .unwrap();
    assert_eq!(values(&store), before);
    assert_eq!(meta_u128(&store, b"total_mined"), 2000);
    assert_eq!(meta_u128(&store, b"fees_burned"), 10);
    assert_eq!(meta_u128(&store, b"lottery_pool"), pool);
    drop(store);
    let reopened = ExperimentalStore::open_fixture(dir.path(), false).unwrap();
    assert_eq!(values(&reopened), before);
}
