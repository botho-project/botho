//! Captured before #1352 extraction against parent c4ac0f0d.
//!
//! botho.ledger.2 (#1286 P2): the fixture file is UNCHANGED. A fresh store now
//! carries exactly one extra `meta` row (the `storage_schema` marker) and a
//! `derivation_contexts` table; [`v1_tables`] asserts and strips the marker so
//! every V1 table is still compared byte-for-byte, and the new context rows
//! are pinned separately.
use super::*;
use sha2::{Digest, Sha256};
/// The eight V1 tables with the `botho.ledger.2` marker row asserted and
/// removed from `meta` (it is the only V2 addition to those tables).
fn v1_tables(ledger: &Ledger) -> serde_json::Value {
    let mut value = tables(ledger);
    let meta = value["meta"].as_array_mut().unwrap();
    let marker = serde_json::json!([
        hex::encode(v2::SCHEMA_KEY),
        hex::encode(v2::LEDGER_SCHEMA_V2)
    ]);
    let at = meta
        .iter()
        .position(|row| *row == marker)
        .expect("V2 marker");
    meta.remove(at);
    value
}
fn context_rows(ledger: &Ledger) -> serde_json::Value {
    let txn = ledger.env.read_txn().unwrap();
    serde_json::json!(ledger
        .contexts_db
        .iter(&txn)
        .unwrap()
        .map(|row| {
            let (k, v) = row.unwrap();
            serde_json::json!([hex::encode(k), hex::encode(v)])
        })
        .collect::<Vec<_>>())
}
fn tables(ledger: &Ledger) -> serde_json::Value {
    let txn = ledger.env.read_txn().unwrap();
    let mut result = serde_json::Map::new();
    for name in [
        "blocks",
        "meta",
        "utxos",
        "address_index",
        "key_images",
        "tx_index",
        "cluster_wealth",
        "bridge_import_clusters",
    ] {
        let db: Database<Bytes, Bytes> =
            ledger.env.open_database(&txn, Some(name)).unwrap().unwrap();
        let rows: Vec<_> = db
            .iter(&txn)
            .unwrap()
            .map(|row| {
                let (k, v) = row.unwrap();
                serde_json::json!([hex::encode(k), hex::encode(v)])
            })
            .collect();
        result.insert(name.into(), serde_json::json!(rows));
    }
    serde_json::Value::Object(result)
}
#[test]
fn v1_writer_pre_extraction_bytes_and_helper_effects() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/v1-writer-effects.json");
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    let golden: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let block: Block =
        bincode::deserialize(&hex::decode(golden["block_hex"].as_str().unwrap()).unwrap()).unwrap();
    let emission = EmissionStateUpdate {
        difficulty: 123,
        total_tx: 42,
        epoch_tx: 9,
        epoch_emission: 123456,
        epoch_burns: 789,
        current_reward: 111,
    };
    ledger.add_block_with_emission(&block, emission).unwrap();
    let accepted = v1_tables(&ledger);
    // The only context row: the golden block's coinbase, Direct at height 1.
    let coinbase = UtxoId::new(block.hash(), 0);
    let direct = crate::ledger::lottery_v2::StoredContext::Direct {
        height: 1,
        base_index: 0,
    };
    assert_eq!(
        context_rows(&ledger),
        serde_json::json!([[
            hex::encode(coinbase.to_bytes()),
            hex::encode(direct.encode().unwrap())
        ]])
    );
    let mut output = block.minting_tx.to_tx_output();
    output.amount = 1234567;
    output.cluster_tags = ClusterTagVector::single(bth_transaction_types::ClusterId(
        bth_cluster_tax::import_cluster_id_for_height(1).0,
    ));
    let utxo = Utxo {
        id: UtxoId::new([0x91; 32], 2),
        output,
        created_at: 1,
    };
    let mut txn = ledger.env.write_txn().unwrap();
    ledger.add_to_address_index(&mut txn, &utxo).unwrap();
    ledger
        .update_cluster_wealth_for_output(&mut txn, &utxo.output)
        .unwrap();
    ledger
        .record_bridge_import_clusters_for_output(&mut txn, &utxo.output, 1)
        .unwrap();
    ledger.record_key_image(&mut txn, &[0x72; 32], 1).unwrap();
    ledger.add_tx_to_index(&mut txn, &[0x91; 32], 1, 3).unwrap();
    txn.commit().unwrap();
    let helpers = v1_tables(&ledger);
    let result = serde_json::json!({"source":"c4ac0f0df46d82b862ca51ca05850794fea9556c before writer extraction", "block_hex":hex::encode(bincode::serialize(&block).unwrap()),"accepted":accepted,"helpers":helpers});
    let golden: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(result, golden);
    let hash = hex::encode(Sha256::digest(std::fs::read(&path).unwrap()));
    eprintln!("V1 writer fixture sha256={hash}");
    drop(ledger);
    let reopened = Ledger::open(dir.path()).unwrap();
    assert_eq!(v1_tables(&reopened), golden["helpers"]);
}

#[test]
fn legacy_shared_writer_keeps_callback_after_coinbase_and_rolls_back_error() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let before = tables(&ledger);
    let state = ledger.get_chain_state().unwrap();
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v1-writer-effects.json"
    ))
    .unwrap();
    let mut block: Block =
        bincode::deserialize(&hex::decode(golden["block_hex"].as_str().unwrap()).unwrap()).unwrap();
    block
        .transactions
        .push(BothoTransaction::new_stub_with_fee(0));
    let bytes = bincode::serialize(&block).unwrap();
    let stages = std::cell::Cell::new(0);
    let callbacks = std::cell::Cell::new(0);
    let mut txn = ledger.env.write_txn().unwrap();
    let result = ledger.write_tables().block_effects(
        &mut txn,
        &block,
        &state,
        0,
        None,
        &bytes,
        writer::Representation::Legacy,
        &mut |_| {
            callbacks.set(callbacks.get() + 1);
            assert_eq!(stages.get(), 4);
            Err(LedgerError::InvalidBlock("test callback rejection".into()))
        },
        &mut || {
            stages.set(stages.get() + 1);
            Ok(())
        },
    );
    assert!(matches!(result,Err(LedgerError::InvalidBlock(s)) if s=="test callback rejection"));
    assert_eq!(callbacks.get(), 1);
    assert_eq!(stages.get(), 4);
    drop(txn);
    assert_eq!(tables(&ledger), before);
    drop(ledger);
    assert_eq!(tables(&Ledger::open(dir.path()).unwrap()), before);
}
