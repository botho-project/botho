use super::*;
use crate as node;
#[path = "../../../../tests/common/positive_transfer.rs"]
mod positive_transfer;
use crate::{
    block::calculate_block_reward,
    ledger::Ledger,
    transaction::{Transaction, MIN_RING_SIZE, PICOCREDITS_PER_CREDIT},
};
use botho_wallet::WalletKeys;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn wallet() -> WalletKeys {
    WalletKeys::from_mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art").unwrap()
}
fn template(store: &ValidatedStore, wallet: &WalletKeys, transactions: Vec<Transaction>) -> Block {
    let txn = store.store.env.read_txn().unwrap();
    let state = store.view(&txn).state().unwrap();
    Block::new_template_with_txs(
        &store.store.envelope(&txn, state.height).unwrap().block,
        &wallet.public_address(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        transactions,
    )
}
fn raw_snapshot(store: &ExperimentalStore) -> Vec<Vec<(Vec<u8>, Vec<u8>)>> {
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
    .iter()
    .map(|n| {
        let table = store
            .env
            .open_database::<Bytes, Bytes>(&txn, Some(n))
            .unwrap()
            .unwrap();
        table
            .iter(&txn)
            .unwrap()
            .map(|r| {
                let (k, v) = r.unwrap();
                (k.to_vec(), v.to_vec())
            })
            .collect()
    })
    .collect()
}
fn snapshot(store: &ValidatedStore) -> Vec<Vec<(Vec<u8>, Vec<u8>)>> {
    raw_snapshot(&store.store)
}

#[test]
fn fresh_schema_reopen_and_ordinary_v1_refusal() {
    let dir = TempDir::new().unwrap();
    let store = ValidatedStore::open(dir.path(), true).unwrap();
    let before = snapshot(&store);
    drop(store);
    assert!(Ledger::open(dir.path()).is_err());
    assert!(ExperimentalStore::open_fixture(dir.path(), false).is_err());
    let store = ValidatedStore::open(dir.path(), false).unwrap();
    assert_eq!(snapshot(&store), before);
    drop(store);
    let fixture = TempDir::new().unwrap();
    drop(ExperimentalStore::open_fixture(fixture.path(), true).unwrap());
    assert!(ValidatedStore::open(fixture.path(), false).is_err());
    assert!(ValidatedStore::open(fixture.path(), true).is_err());
}
#[test]
fn mint_transition_same_state_stale_and_all_write_rollback() {
    let dir = TempDir::new().unwrap();
    let store = ValidatedStore::open(dir.path(), true).unwrap();
    let w = wallet();
    let block = store.produce(template(&store, &w, vec![])).unwrap();
    let mut count = 0;
    store
        .apply(&block, None, || {
            count += 1;
            Ok(())
        })
        .unwrap();
    let accepted = snapshot(&store);
    assert!(store.apply(&block, None, || Ok(())).is_err());
    assert_eq!(snapshot(&store), accepted);
    drop(store);
    for fail in 1..=count {
        let dir = TempDir::new().unwrap();
        let store = ValidatedStore::open(dir.path(), true).unwrap();
        let before = snapshot(&store);
        let mut at = 0;
        assert!(store
            .apply(&block, None, || {
                at += 1;
                if at == fail {
                    Err(inconsistent("injected write abort"))
                } else {
                    Ok(())
                }
            })
            .is_err());
        assert_eq!(snapshot(&store), before);
        drop(store);
        let reopened = ValidatedStore::open(dir.path(), false).unwrap();
        assert_eq!(snapshot(&reopened), before);
    }
}
#[test]
fn strict_state_and_candidate_errors_do_not_change_draw() {
    let dir = TempDir::new().unwrap();
    let store = ValidatedStore::open(dir.path(), true).unwrap();
    let w = wallet();
    let block = store.produce(template(&store, &w, vec![])).unwrap();
    store.apply(&block, None, || Ok(())).unwrap();
    let next = template(&store, &w, vec![]);
    let id = UtxoId::new(block.block.hash(), 0);
    let mut txn = store.store.env.write_txn().unwrap();
    store
        .store
        .contexts
        .delete(&mut txn, &id.to_bytes())
        .unwrap();
    txn.commit().unwrap();
    assert!(matches!(
        store.produce(next),
        Err(LedgerError::MissingContext(_))
    ));
    let mut txn = store.store.env.write_txn().unwrap();
    store
        .store
        .meta
        .put(&mut txn, b"total_mined", &[1])
        .unwrap();
    txn.commit().unwrap();
    drop(store);
    assert!(ValidatedStore::open(dir.path(), false).is_err());
}
#[test]
fn empty_summary_and_rule_root_are_always_checked() {
    let dir = TempDir::new().unwrap();
    let store = ValidatedStore::open(dir.path(), true).unwrap();
    let w = wallet();
    let e = store.produce(template(&store, &w, vec![])).unwrap();
    let before = snapshot(&store);
    for field in 0..5 {
        let mut bad = e.clone();
        match field {
            0 => bad.block.lottery_summary.total_fees = 1,
            1 => bad.block.lottery_summary.pool_distributed = 1,
            2 => bad.block.lottery_summary.amount_burned = 1,
            3 => bad.block.lottery_summary.lottery_seed[0] = 1,
            _ => {}
        }
        bad.block.header.tx_root = if field == 4 {
            Block::compute_tx_root(&bad.block.transactions)
        } else {
            root(&bad.block, &bad.records).unwrap()
        };
        assert!(store.apply(&bad, None, || Ok(())).is_err());
        assert_eq!(snapshot(&store), before);
    }
}
#[test]
fn canonical_maturity_fee_funded_payout_accepts_and_reopens() {
    let total = Instant::now();
    let dir = TempDir::new().unwrap();
    let store = ValidatedStore::open(dir.path(), true).unwrap();
    let w = wallet();
    let mut producing = Duration::ZERO;
    let mut validating = Duration::ZERO;
    let mut persisting = Duration::ZERO;
    let maturity = canonical_config().draw_config.min_utxo_age;
    assert_eq!(maturity, 720);
    let mut sources = vec![];
    let mut accepted_rewards = 0u128;
    for _ in 0..=maturity {
        assert!(
            total.elapsed() < Duration::from_secs(600),
            "canonical maturity execution deadline"
        );
        let began = Instant::now();
        let e = store.produce(template(&store, &w, vec![])).unwrap();
        producing += began.elapsed();
        assert!(e.records.is_empty());
        assert_eq!(e.block.minting_tx.lottery_emission_share(), 0);
        let (v, p) = store.apply(&e, None, || Ok(())).unwrap();
        validating += v;
        persisting += p;
        accepted_rewards += e.block.minting_tx.reward as u128;
        if e.block.height().is_multiple_of(120) {
            eprintln!(
                "V2_MATURITY height={} elapsed_ms={}",
                e.block.height(),
                total.elapsed().as_millis()
            );
        }
        if sources.len() < MIN_RING_SIZE {
            let txn = store.store.env.read_txn().unwrap();
            sources.push(
                store
                    .store
                    .read(&txn, &UtxoId::new(e.block.hash(), 0))
                    .unwrap()
                    .0,
            );
        }
    }
    let matured = total.elapsed();
    let fee = PICOCREDITS_PER_CREDIT;
    let signing = Instant::now();
    let tx = positive_transfer::with_decoys(
        &w,
        &sources[0],
        0,
        &w.public_address(),
        PICOCREDITS_PER_CREDIT,
        fee,
        maturity + 1,
        &sources[1..]
            .iter()
            .map(|u| u.output.clone())
            .collect::<Vec<_>>(),
    );
    let sign_time = signing.elapsed();
    let txn = store.store.env.read_txn().unwrap();
    assert!(
        store
            .view(&txn)
            .consensus_fee_floor(&tx, maturity + 2)
            .unwrap()
            <= fee
    );
    drop(txn);
    let began = Instant::now();
    let e = store
        .produce(template(&store, &w, vec![tx.clone()]))
        .unwrap();
    let funded_produce = began.elapsed();
    assert!(!e.records.is_empty());
    assert!(e.block.lottery_summary.pool_distributed > 0);
    let emission = EmissionStateUpdate {
        difficulty: u64::MAX,
        total_tx: 1,
        epoch_tx: 1,
        epoch_emission: 0,
        epoch_burns: e.block.lottery_summary.amount_burned,
        current_reward: e.block.minting_tx.reward,
    };
    // Abort each actual write in this fully validated, fee-funded transition.
    // The first iteration whose abort index exceeds the actual write count
    // commits it, so no alternative writer or synthetic effect count is used.
    let mut store = store;
    let before_funded = snapshot(&store);
    let mut accepted_timing = None;
    let mut aborted_stages = 0;
    for fail in 1..=128 {
        let mut at = 0;
        match store.apply(&e, Some(emission), || {
            at += 1;
            if at == fail {
                Err(inconsistent("funded write abort"))
            } else {
                Ok(())
            }
        }) {
            Ok(timing) => {
                accepted_timing = Some(timing);
                break;
            }
            Err(LedgerError::InconsistentRecord(message)) if message == "funded write abort" => {
                assert_eq!(at, fail);
                assert_eq!(snapshot(&store), before_funded);
                drop(store);
                store = ValidatedStore::open(dir.path(), false).unwrap();
                assert_eq!(snapshot(&store), before_funded);
                aborted_stages += 1;
            }
            Err(error) => panic!("unexpected funded validation failure: {error}"),
        }
    }
    let (funded_validate, funded_persist) = accepted_timing.expect("finite actual write count");
    assert!(aborted_stages > 20);
    let accepted = snapshot(&store);
    drop(store);
    let store = ValidatedStore::open(dir.path(), false).unwrap();
    assert_eq!(snapshot(&store), accepted);
    let txn = store.store.env.read_txn().unwrap();
    let view = store.view(&txn);
    let state = view.state().unwrap();
    assert_eq!(state.height, maturity + 2);
    assert_eq!(state.total_tx, 1);
    assert_eq!(
        state.total_fees_burned,
        e.block.lottery_summary.amount_burned as u128
    );
    assert_eq!(
        fee as u128,
        state.total_fees_burned
            + view.u128(b"lottery_pool").unwrap()
            + e.block.lottery_summary.pool_distributed as u128
    );
    assert_eq!(
        state.total_mined,
        accepted_rewards + e.block.minting_tx.reward as u128
    );
    assert_eq!(
        view.is_key_image_spent(&tx.inputs.clsag()[0].key_image)
            .unwrap(),
        Some(state.height)
    );
    let location = store
        .store
        .tables
        .tx_index_db
        .get(&txn, &tx.hash())
        .unwrap()
        .unwrap();
    assert_eq!(&location[..8], &state.height.to_le_bytes());
    assert_eq!(&location[8..], &0u32.to_le_bytes());
    let (_, c) = store.store.read(&txn, &UtxoId::new(tx.hash(), 1)).unwrap();
    assert_eq!(c.derivation().base_index, 1);
    for (i, r) in e.records.iter().enumerate() {
        let id = UtxoId::new(e.block.hash(), i as u32 + 1);
        let (p, c) = store.store.read(&txn, &id).unwrap();
        assert_eq!(p.output.target_key, r.target);
        assert_eq!(c.derivation(), r.context);
        assert_ne!(
            r.target,
            view.source(&r.winner).unwrap().0.output.target_key
        );
        let index = store
            .store
            .tables
            .address_index_db
            .get(&txn, &r.target)
            .unwrap()
            .unwrap();
        assert_eq!(index, id.to_bytes());
        assert_eq!(
            view.get_utxo_by_target_key(&r.target).unwrap().unwrap().id,
            id
        );
    }
    // Recompute the exact wealth materialization from all durable outputs,
    // including the new payouts, rather than only checking row existence.
    let mut expected_wealth = std::collections::BTreeMap::<u64, u128>::new();
    for row in store.store.utxos.iter(&txn).unwrap() {
        let (_, bytes) = row.unwrap();
        let u: Utxo = decode(bytes).unwrap();
        for tag in &u.output.cluster_tags.entries {
            let contribution = u.output.amount as u128 * tag.weight as u128
                / bth_transaction_types::TAG_WEIGHT_SCALE as u128;
            if contribution > 0 {
                let sum = expected_wealth.entry(tag.cluster_id.0).or_default();
                *sum = sum.saturating_add(contribution);
            }
        }
    }
    for (cluster, expected) in expected_wealth {
        assert_eq!(view.get_cluster_wealth(cluster).unwrap(), expected);
    }
    // All source contexts and creating commitments survived the accepted path.
    assert_eq!(root(&e.block, &e.records).unwrap(), e.block.header.tx_root);
    eprintln!("V2_ACCEPTED aborted_stages={} maturity_blocks={} maturity_ms={} produce_ms={} validate_ms={} persist_ms={} sign_ms={} funded_produce_ms={} funded_validate_ms={} funded_persist_ms={} total_ms={} payouts={} fee={} burn={} pool={} distributed={}",aborted_stages,maturity+1,matured.as_millis(),producing.as_millis(),validating.as_millis(),persisting.as_millis(),sign_time.as_millis(),funded_produce.as_millis(),funded_validate.as_millis(),funded_persist.as_millis(),total.elapsed().as_millis(),e.records.len(),fee,state.total_fees_burned,view.u128(b"lottery_pool").unwrap(),e.block.lottery_summary.pool_distributed);
}

#[test]
fn strict_metadata_identities_fail_reopen_without_writes() {
    for key in [
        GENESIS,
        RULES_KEY,
        CHECKPOINT,
        b"tip_hash",
        b"difficulty",
        b"lottery_pool",
    ] {
        let dir = TempDir::new().unwrap();
        let store = ValidatedStore::open(dir.path(), true).unwrap();
        let mut txn = store.store.env.write_txn().unwrap();
        store
            .store
            .meta
            .put(&mut txn, key, b"wrong width/identity")
            .unwrap();
        txn.commit().unwrap();
        let before = snapshot(&store);
        drop(store);
        assert!(ValidatedStore::open(dir.path(), false).is_err());
        // Inspect only through the private raw test opener to prove rejection
        // made no writes; this never constructs a validated handle.
        let raw =
            ExperimentalStore::open_local(dir.path(), false, VALIDATED_SCHEMA, |_, _, _| Ok(()))
                .unwrap();
        assert_eq!(raw_snapshot(&raw), before);
    }
}
#[test]
fn strict_creating_commitment_and_positive_wealth_are_required() {
    let dir = TempDir::new().unwrap();
    let store = ValidatedStore::open(dir.path(), true).unwrap();
    let w = wallet();
    let e = store.produce(template(&store, &w, vec![])).unwrap();
    store.apply(&e, None, || Ok(())).unwrap();
    let id = UtxoId::new(e.block.hash(), 0);
    let txn = store.store.env.read_txn().unwrap();
    let (u, _) = store.view(&txn).output(&id).unwrap();
    drop(txn);
    let tag = &u.output.cluster_tags.entries[0];
    let mut txn = store.store.env.write_txn().unwrap();
    let key = tag.cluster_id.0.to_le_bytes();
    let wealth = store
        .store
        .tables
        .cluster_wealth_db
        .get(&txn, &key)
        .unwrap()
        .unwrap()
        .to_vec();
    store
        .store
        .tables
        .cluster_wealth_db
        .delete(&mut txn, &key)
        .unwrap();
    txn.commit().unwrap();
    let txn = store.store.env.read_txn().unwrap();
    assert!(
        matches!(store.view(&txn).output(&id),Err(LedgerError::InconsistentRecord(s)) if s=="missing positive cluster wealth")
    );
    drop(txn);
    let next = template(&store, &w, vec![]);
    assert!(store.produce(next).is_err());
    let mut txn = store.store.env.write_txn().unwrap();
    store
        .store
        .tables
        .cluster_wealth_db
        .put(&mut txn, &key, &wealth)
        .unwrap();
    let mut corrupted = e.clone();
    corrupted.block.lottery_summary.total_fees = 1;
    store
        .store
        .blocks
        .put(&mut txn, &1, &corrupted.encode().unwrap())
        .unwrap();
    txn.commit().unwrap();
    let txn = store.store.env.read_txn().unwrap();
    assert!(
        matches!(store.view(&txn).output(&id),Err(LedgerError::InconsistentRecord(s)) if s=="creating body commitment")
    );
    assert!(store.view(&txn).state().is_err());
}
