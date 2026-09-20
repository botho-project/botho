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
    ["blocks", "meta", "utxos", "derivation_contexts"]
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
    // Block + three outputs/contexts (coinbase, two awards) + checkpoint = eight
    // writes.
    for stop in 1..=8 {
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
