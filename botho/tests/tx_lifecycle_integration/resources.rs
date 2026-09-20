//! Explicit, ordinary V1 apply measurements; never run by the default suite.
use super::*;
use nix::libc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::MetadataExt, path::Path, time::Instant};

fn cpu_us() -> (u64, u64) {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes this correctly sized struct on success.
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    let usage = unsafe { usage.assume_init() };
    let us = |v: libc::timeval| {
        u64::try_from(v.tv_sec).unwrap() * 1_000_000 + u64::try_from(v.tv_usec).unwrap()
    };
    (us(usage.ru_utime), us(usage.ru_stime))
}
fn duration(cpu: (u64, u64), wall: Instant) -> Value {
    let end = cpu_us();
    json!({"wall_ns": u64::try_from(wall.elapsed().as_nanos()).unwrap(),
        "user_cpu_us": end.0.checked_sub(cpu.0).unwrap(),
        "system_cpu_us": end.1.checked_sub(cpu.1).unwrap()})
}
fn file_sizes(path: &Path) -> Value {
    let file = |name| {
        let m = fs::metadata(path.join(name)).unwrap();
        // Unix st_blocks counts 512-byte units on both supported targets.
        json!({"logical_bytes": m.len(), "allocated_bytes": m.blocks().checked_mul(512).unwrap()})
    };
    json!({"data.mdb":file("data.mdb"), "lock.mdb":file("lock.mdb")})
}
// A deliberately identified subset, NOT a full database census: the measured
// block row, its coinbase/payment output rows, and both funding UTXO rows.
fn selected_records(ledger: &Ledger, block: &Block, ids: &[UtxoId]) -> Value {
    let mut rows = Vec::<(String, Vec<u8>, Vec<u8>)>::new();
    if ledger.get_chain_state().unwrap().height >= block.height() {
        let stored = ledger.get_block(block.height()).unwrap();
        rows.push((
            "blocks".into(),
            block.height().to_le_bytes().to_vec(),
            bincode::serialize(&stored).unwrap(),
        ));
    }
    for id in ids {
        if let Some(utxo) = ledger.get_utxo(id).unwrap() {
            rows.push((
                "utxos".into(),
                id.to_bytes().to_vec(),
                bincode::serialize(&utxo).unwrap(),
            ));
        }
    }
    rows.sort();
    let encoded_bytes: usize = rows.iter().map(|(_, k, v)| k.len() + v.len()).sum();
    json!({"scope":"selected_blocks_and_utxos_only", "rows": rows.len(),
        "encoded_key_value_bytes":encoded_bytes,
        "rows_sha256":hex::encode(Sha256::digest(bincode::serialize(&rows).unwrap()))})
}

#[test]
#[ignore = "explicit six-process resource collector only"]
fn accepted_v1_apply_resource_sample() {
    let count: usize = std::env::var("BOTHO_V1_RESOURCE_PAYMENTS")
        .unwrap()
        .parse()
        .unwrap();
    assert!(count <= 2);
    let setup_wall = Instant::now();
    let setup_cpu = cpu_us();
    let (dir, ledger) = create_test_ledger();
    let miner = create_wallet(1);
    let decoy = create_wallet(0);
    let recipients = [create_wallet(2), create_wallet(3)];
    for i in 0..20 {
        let wallet = if i < 2 { &miner } else { &decoy };
        let block = mine_block(&ledger, &wallet.public_address(), vec![]);
        ledger.add_block(&block).unwrap();
    }
    let funding = scan_wallet_utxos(&ledger, &miner);
    assert_eq!(funding.len(), 2);
    let payment = 10 * PICOCREDITS_PER_CREDIT;
    let transactions: Vec<_> = (0..count)
        .map(|i| {
            let (utxo, subaddress) = &funding[i];
            let tx = create_signed_transaction(
                &miner,
                utxo,
                *subaddress,
                &recipients[i].public_address(),
                payment,
                MIN_TX_FEE,
                20,
                &ledger,
            );
            assert_eq!(tx.outputs.len(), 2);
            assert_eq!(
                tx.outputs.iter().map(|o| o.amount as u128).sum::<u128>() + tx.fee as u128,
                utxo.output.amount as u128
            );
            tx.verify_ring_signatures().unwrap();
            tx
        })
        .collect();
    let block = mine_block(&ledger, &miner.public_address(), transactions);
    assert_eq!(block.height(), 21);
    assert!(block.lottery_outputs.is_empty()); // current fixture is below maturity
    let block_bytes = bincode::serialize(&block).unwrap();
    let transaction_bytes: Vec<usize> = block
        .transactions
        .iter()
        .map(|tx| bincode::serialized_size(tx).unwrap() as usize)
        .collect();
    let mut ids: Vec<_> = funding.iter().map(|(u, _)| u.id.clone()).collect();
    ids.push(UtxoId::new(block.hash(), 0));
    for tx in &block.transactions {
        for index in 0..tx.outputs.len() {
            ids.push(UtxoId::new(tx.hash(), index as u32));
        }
        for image in tx.key_images() {
            assert_eq!(ledger.is_key_image_spent(&image).unwrap(), None);
        }
    }
    ids.sort_by_key(|id| id.to_bytes());
    ids.dedup_by_key(|id| id.to_bytes());
    let path = dir.path().join("ledger");
    let before_files = file_sizes(&path);
    let before_records = selected_records(&ledger, &block, &ids);
    let before_state = ledger.get_chain_state().unwrap();
    let before_pool = ledger.get_lottery_pool().unwrap();
    let setup = duration(setup_cpu, setup_wall);

    let wall = Instant::now();
    let cpu = cpu_us();
    let applied = ledger.add_block(&block);
    let apply = duration(cpu, wall);
    applied.expect("measured block must pass unchanged production validation");

    let after_files = file_sizes(&path);
    let after_records = selected_records(&ledger, &block, &ids);
    let after_state = ledger.get_chain_state().unwrap();
    assert_eq!(after_state.height, 21);
    assert_eq!(after_state.tip_hash, block.hash());
    assert_eq!(
        after_state.total_mined - before_state.total_mined,
        block.minting_tx.reward as u128
    );
    assert_eq!(
        after_state.total_fees_burned - before_state.total_fees_burned,
        block.lottery_summary.amount_burned as u128
    );
    let gross: u128 = block.transactions.iter().map(|t| t.fee as u128).sum();
    assert_eq!(
        ledger.get_lottery_pool().unwrap() + block.lottery_summary.amount_burned as u128,
        before_pool + gross + block.minting_tx.lottery_emission_share() as u128
    );
    let check = |ledger: &Ledger| {
        assert_eq!(
            bincode::serialize(&ledger.get_block(21).unwrap()).unwrap(),
            block_bytes
        );
        for (i, tx) in block.transactions.iter().enumerate() {
            let location = ledger
                .get_transaction_location(&tx.hash())
                .unwrap()
                .unwrap();
            assert_eq!((location.block_height, location.tx_index), (21, i as u32));
            for image in tx.key_images() {
                assert_eq!(ledger.is_key_image_spent(&image).unwrap(), Some(21));
            }
            for (j, output) in tx.outputs.iter().enumerate() {
                let stored = ledger
                    .get_utxo(&UtxoId::new(tx.hash(), j as u32))
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    bincode::serialize(&stored.output).unwrap(),
                    bincode::serialize(output).unwrap()
                );
            }
            assert_eq!(get_wallet_balance(ledger, &recipients[i]), payment);
        }
        assert_eq!(selected_records(ledger, &block, &ids), after_records);
    };
    check(&ledger);
    drop(ledger);
    let reopened = Ledger::open(&path).unwrap();
    check(&reopened);
    let sample = json!({"schema":1, "kind":"ordinary_v1_accepted_block_apply", "status":"accepted_reopened",
        "pid":std::process::id(), "payments":count, "transactions":count,
        "recipient_outputs":count, "change_outputs":count, "coinbase_outputs":1, "lottery_outputs":0,
        "ring_size":20, "inputs_per_transaction":1,
        "units":{"wall":"nanoseconds", "cpu":"microseconds_RUSAGE_SELF", "objects":"bincode_object_bytes_not_wire", "storage":"file_bytes_and_selected_record_bytes_separate"},
        "setup":setup, "apply":apply,
        "objects":{"block_bytes":block_bytes.len(), "transaction_bytes":transaction_bytes,
          "block_sha256":hex::encode(Sha256::digest(&block_bytes))},
        "storage":{"before_files":before_files,"after_files":after_files,"before_selected_records":before_records,"after_selected_records":after_records,
          "selected_block_height":21,"selected_utxo_ids":ids.iter().map(|id|hex::encode(id.to_bytes())).collect::<Vec<_>>()}});
    println!(
        "V1_RESOURCE_SAMPLE {}",
        serde_json::to_string(&sample).unwrap()
    );
}
