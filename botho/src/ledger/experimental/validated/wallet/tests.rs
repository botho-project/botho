//! Ordinary positive offline workload. Random output keys and natural lottery
//! draws are retained; this is a bounded functionality profile, not a fixed
//! winner vector, economics calibration, or activated wallet/network route.
use super::*;
use crate::{block::calculate_block_reward, transaction::PICOCREDITS_PER_CREDIT as COIN};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
const RECEIVER_WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const MAX_HEIGHT: u64 = 1700;
const FUNDING_FEE: u64 = 10 * COIN;
const SPEND_FEE: u64 = COIN;

fn height(store: &ValidatedStore) -> u64 {
    let r = store.store.env.read_txn().unwrap();
    store.view(&r).state().unwrap().height
}
fn accepted(store: &ValidatedStore, miner: &Wallet, transactions: Vec<Transaction>) -> Envelope {
    let r = store.store.env.read_txn().unwrap();
    let state = store.view(&r).state().unwrap();
    for tx in &transactions {
        assert!(
            store
                .view(&r)
                .consensus_fee_floor(tx, state.height + 1)
                .unwrap()
                <= tx.fee
        );
    }
    let block = Block::new_template_with_txs(
        &store.store.envelope(&r, state.height).unwrap().block,
        &miner.default_address(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        transactions,
    );
    drop(r);
    let produced = store.produce(block).unwrap();
    store.apply(&produced, None, || Ok(())).unwrap();
    produced
}
fn lookup(store: &ValidatedStore, id: UtxoId) -> (Utxo, StoredContext) {
    let r = store.store.env.read_txn().unwrap();
    store.view(&r).output(&id).unwrap()
}
fn owned(store: &ValidatedStore, wallet: &Wallet, id: UtxoId) -> OwnedOutput {
    store
        .discover_owned(wallet)
        .unwrap()
        .into_iter()
        .find(|o| o.utxo.id == id)
        .unwrap_or_else(|| panic!("wallet did not discover {}", hex::encode(id.to_bytes())))
}
fn payment(
    store: &ValidatedStore,
    wallet: &Wallet,
    recipient: &Wallet,
    id: UtxoId,
    fee: u64,
) -> Transaction {
    let input = owned(store, wallet, id);
    assert_eq!(input.subaddress, 0);
    assert!(input.utxo.output.amount > fee);
    let expected_image = input.key_image;
    let value = input.utxo.output.amount - fee;
    let output = TxOutput::new_hybrid_to_address(
        value,
        &recipient.quantum_safe_address(),
        0,
        None,
        Default::default(),
    )
    .unwrap();
    let tx = store
        .wallet_transaction(wallet, &[input], vec![output], fee)
        .unwrap();
    assert_eq!(tx.inputs.clsag()[0].key_image, expected_image);
    tx
}

#[derive(Clone)]
struct Family {
    source: UtxoId,
    awards: Vec<UtxoId>,
}
fn matrix(
    store: &ValidatedStore,
    edges: &BTreeMap<[u8; 36], Vec<UtxoId>>,
    wallet: &Wallet,
) -> Option<[Family; 3]> {
    let now = height(store);
    let unspent: BTreeSet<_> = store
        .discover_owned(wallet)
        .unwrap()
        .iter()
        .map(|o| o.utxo.id.to_bytes())
        .collect();
    let usable = |id: UtxoId| {
        if !unspent.contains(&id.to_bytes()) {
            return false;
        }
        let (u, _) = lookup(store, id);
        now.saturating_sub(u.created_at) >= 10 && u.output.amount > SPEND_FEE
    };
    // A: nonzero-index hybrid source with two independently funded awards.
    for (source, awards) in edges {
        let source = UtxoId::from_bytes(source).unwrap();
        let (u, c) = lookup(store, source);
        if !matches!(c, StoredContext::Direct { base_index, .. } if base_index > 4)
            || u.output.kem_ciphertext.is_none()
            || !usable(source)
        {
            continue;
        }
        let awards: Vec<_> = awards.iter().copied().filter(|id| usable(*id)).collect();
        if awards.len() < 2 {
            continue;
        }
        let a = Family {
            source,
            awards: awards[..2].to_vec(),
        };
        let mut used: BTreeSet<_> = a.awards.iter().map(|x| x.to_bytes()).collect();
        used.insert(a.source.to_bytes());
        // B: a separate classical source, spent after its award.
        for (source, awards) in edges {
            let source = UtxoId::from_bytes(source).unwrap();
            let (u, c) = lookup(store, source);
            if u.output.kem_ciphertext.is_some()
                || !matches!(c, StoredContext::Direct { .. })
                || used.contains(&source.to_bytes())
                || !usable(source)
            {
                continue;
            }
            let Some(award) = awards
                .iter()
                .copied()
                .find(|id| usable(*id) && !used.contains(&id.to_bytes()))
            else {
                continue;
            };
            let b = Family {
                source,
                awards: vec![award],
            };
            // C: an actual accepted award winning again, on a disjoint family.
            for (source, awards) in edges {
                let source = UtxoId::from_bytes(source).unwrap();
                let (_, c) = lookup(store, source);
                if !matches!(c, StoredContext::Lottery { .. })
                    || !usable(source)
                    || used.contains(&source.to_bytes())
                    || source == b.source
                    || source == award
                {
                    continue;
                }
                let Some(nested) = awards.iter().copied().find(|id| {
                    usable(*id) && !used.contains(&id.to_bytes()) && *id != b.source && *id != award
                }) else {
                    continue;
                };
                return Some([
                    a,
                    b,
                    Family {
                        source,
                        awards: vec![nested],
                    },
                ]);
            }
        }
    }
    None
}

#[test]
#[ignore = "bounded 30-minute native wallet profile; run explicitly"]
fn native_wallet_accepted_repeated_nested_payout_spends() {
    let began = Instant::now();
    let dir = TempDir::new().unwrap();
    let mut store = ValidatedStore::open(dir.path(), true).unwrap();
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let receiver = Wallet::from_mnemonic(RECEIVER_WORDS).unwrap();
    let mut coinbases = Vec::new();
    let mut edges: BTreeMap<[u8; 36], Vec<UtxoId>> = BTreeMap::new();
    let mut fees = 0u128;
    let mut mined = 0u128;
    let mut burned = 0u128;
    let mut distributed = 0u128;
    let mut paid = 0u128;
    let mut transaction_count = 0u64;
    let mut chosen = None;
    assert_eq!(canonical_config().draw_config.min_utxo_age, 720);
    eprintln!("WALLET_V2 phase=generate max_height={MAX_HEIGHT} deadline_seconds=1800 funding_every=10 fee={FUNDING_FEE}");
    for next in 1..=MAX_HEIGHT {
        assert!(
            began.elapsed() < Duration::from_secs(1800),
            "wallet profile deadline; no acceptance claim"
        );
        let mut txs = vec![];
        if next == 40 {
            let inputs = [
                owned(&store, &wallet, coinbases[0]),
                owned(&store, &wallet, coinbases[1]),
            ];
            let amount = inputs
                .iter()
                .try_fold(0u64, |sum, o| sum.checked_add(o.utxo.output.amount))
                .expect("funding amount fits");
            let output_value = 2 * COIN;
            let change = amount
                .checked_sub(32 * output_value)
                .and_then(|v| v.checked_sub(FUNDING_FEE))
                .expect("actual funding covers outputs and fee");
            // Ordinary actual wallet payment: 16 classical and 16 hybrid
            // outputs, with hybrid outputs at nonzero original indices.
            let mut outputs: Vec<_> = (0..32)
                .map(|i| {
                    if i % 2 == 0 {
                        TxOutput::new(output_value, &wallet.default_address())
                    } else {
                        TxOutput::new_hybrid_to_address(
                            output_value,
                            &wallet.quantum_safe_address(),
                            i,
                            None,
                            Default::default(),
                        )
                        .unwrap()
                    }
                })
                .collect();
            outputs.push(TxOutput::new(change, &wallet.default_address()));
            txs.push(
                store
                    .wallet_transaction(&wallet, &inputs, outputs, FUNDING_FEE)
                    .unwrap(),
            );
        } else if next >= 721 && (next % 10 == 1 || next >= 1441) {
            // Recent mature coinbases cannot yet win the lottery; keep the
            // older source/payout families unspent during acquisition.
            let id = coinbases[(next - 21) as usize];
            txs.push(payment(&store, &wallet, &wallet, id, FUNDING_FEE));
        }
        fees += txs.iter().map(|t| t.fee as u128).sum::<u128>();
        transaction_count += txs.len() as u64;
        let e = accepted(&store, &wallet, txs);
        coinbases.push(UtxoId::new(e.block.hash(), 0));
        mined += e.block.minting_tx.reward as u128;
        burned += e.block.lottery_summary.amount_burned as u128;
        distributed += e.block.lottery_summary.pool_distributed as u128;
        for r in &e.records {
            edges
                .entry(UtxoId::new(r.winner.hash, r.winner.index).to_bytes())
                .or_default()
                .push(UtxoId::new(e.block.hash(), r.ordinal + 1));
        }
        if next % 100 == 0 {
            eprintln!(
                "WALLET_V2 phase=generate height={next} families={} elapsed_ms={}",
                edges.len(),
                began.elapsed().as_millis()
            );
        }
        if next >= 1451 && next % 10 == 1 {
            chosen = matrix(&store, &edges, &wallet);
            if chosen.is_some() {
                break;
            }
        }
    }
    let families = chosen.expect("bounded natural workload did not realize mature affordable classical/repeated/nonzero-hybrid/nested families; no spendability acceptance");
    eprintln!(
        "WALLET_V2 phase=discovered height={} elapsed_ms={}",
        height(&store),
        began.elapsed().as_millis()
    );
    // Reopen before discovery/signing: no retained derivation object is authority.
    drop(store);
    store = ValidatedStore::open(dir.path(), false).unwrap();
    let mut order = vec![families[0].source];
    order.extend(&families[0].awards);
    order.extend([
        families[1].awards[0],
        families[1].source,
        families[2].source,
        families[2].awards[0],
    ]);
    let mut remaining: BTreeSet<_> = order.iter().map(|id| id.to_bytes()).collect();
    assert_eq!(remaining.len(), 7);
    let discovered = store.discover_owned(&wallet).unwrap();
    let images: BTreeSet<_> = discovered
        .iter()
        .filter(|o| remaining.contains(&o.utxo.id.to_bytes()))
        .map(|o| o.key_image)
        .collect();
    assert_eq!(
        images.len(),
        7,
        "source and each repeated/nested award have independent final images"
    );
    let nonzero = owned(&store, &wallet, families[0].awards[0]);
    assert!(nonzero.context.derivation().base_index > 4);
    assert_ne!(
        nonzero.context.derivation().base_index,
        nonzero.utxo.id.output_index
    );
    for id in order {
        assert!(
            began.elapsed() < Duration::from_secs(1800),
            "wallet spend profile deadline"
        );
        let tx = payment(&store, &wallet, &receiver, id, SPEND_FEE);
        let tx_id = tx.hash();
        let image = tx.inputs.clsag()[0].key_image;
        paid += tx.outputs.iter().map(|o| o.amount as u128).sum::<u128>();
        fees += tx.fee as u128;
        transaction_count += 1;
        let e = accepted(&store, &wallet, vec![tx]);
        mined += e.block.minting_tx.reward as u128;
        burned += e.block.lottery_summary.amount_burned as u128;
        distributed += e.block.lottery_summary.pool_distributed as u128;
        remaining.remove(&id.to_bytes());
        let scan = store.discover_owned(&wallet).unwrap();
        assert!(!scan.iter().any(|o| o.utxo.id == id));
        for sibling in &remaining {
            assert!(
                scan.iter().any(|o| o.utxo.id.to_bytes() == *sibling),
                "unspent sibling disappeared"
            );
        }
        let received = owned(&store, &receiver, UtxoId::new(tx_id, 0));
        assert_eq!(received.utxo.created_at, height(&store)); // immature discovery is intentional
        let current_height = height(&store);
        let r = store.store.env.read_txn().unwrap();
        let view = store.view(&r);
        assert_eq!(
            view.is_key_image_spent(&image).unwrap(),
            Some(current_height)
        );
        assert!(store
            .store
            .tables
            .tx_index_db
            .get(&r, &tx_id)
            .unwrap()
            .is_some());
        eprintln!(
            "WALLET_V2 phase=accepted_spend height={} source={} tx={} image={}",
            current_height,
            hex::encode(id.to_bytes()),
            hex::encode(tx_id),
            hex::encode(image)
        );
    }
    drop(store);
    store = ValidatedStore::open(dir.path(), false).unwrap();
    let received = store.discover_owned(&receiver).unwrap();
    assert_eq!(received.len(), 7);
    assert_eq!(
        received
            .iter()
            .map(|o| o.utxo.output.amount as u128)
            .sum::<u128>(),
        paid
    );
    let remaining_value: u128 = store
        .discover_owned(&wallet)
        .unwrap()
        .iter()
        .map(|o| o.utxo.output.amount as u128)
        .sum();
    let r = store.store.env.read_txn().unwrap();
    let view = store.view(&r);
    let state = view.state().unwrap();
    assert_eq!(
        remaining_value + paid + burned + view.u128(b"lottery_pool").unwrap(),
        mined
    );
    assert_eq!(state.total_mined, mined);
    assert_eq!(state.total_fees_burned, burned);
    // apply(None) deliberately preserves optional emission-controller counters.
    assert_eq!(state.total_tx, 0);
    assert_eq!(
        store
            .store
            .tables
            .tx_index_db
            .iter(&r)
            .unwrap()
            .map(|row| row.map(|_| ()))
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .len() as u64,
        transaction_count
    );
    assert_eq!(
        fees,
        burned + view.u128(b"lottery_pool").unwrap() + distributed
    );
    eprintln!("WALLET_V2 PASS height={} transactions={} accepted_family_spends=7 receiver_value={} fees={} burned={} distributed={} pool={} elapsed_ms={}",state.height,transaction_count,paid,fees,burned,distributed,view.u128(b"lottery_pool").unwrap(),began.elapsed().as_millis());
}

#[test]
fn authenticated_discovery_separates_maturity_and_checkpoint() {
    let dir = TempDir::new().unwrap();
    let store = ValidatedStore::open(dir.path(), true).unwrap();
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let other = Wallet::from_mnemonic(RECEIVER_WORDS).unwrap();
    let e = accepted(&store, &wallet, vec![]);
    let id = UtxoId::new(e.block.hash(), 0);
    let found = owned(&store, &wallet, id);
    assert!(store.discover_owned(&other).unwrap().is_empty());
    let value = found.utxo.output.amount - SPEND_FEE;
    let make_output = || vec![TxOutput::new(value, &other.default_address())];
    let error = store
        .wallet_transaction(&wallet, &[found], make_output(), SPEND_FEE)
        .unwrap_err();
    assert!(format!("{error:#}").contains("input not mature"));
    for _ in 0..10 {
        accepted(&store, &wallet, vec![]);
    }
    let found = owned(&store, &wallet, id);
    accepted(&store, &wallet, vec![]);
    let error = store
        .wallet_transaction(&wallet, &[found], make_output(), SPEND_FEE)
        .unwrap_err();
    assert!(format!("{error:#}").contains("stale/wrong-chain discovery; rescan required"));
    assert!(owned(&store, &wallet, id).utxo.created_at < height(&store));
}
