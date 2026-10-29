//! Native node-wallet discovery and spending of V2 lottery payouts (#1286 P7).
use super::{fixture::*, *};
use crate::{mempool::Mempool, wallet::WalletRead};
use bth_crypto_keys::RistrettoPublic;
use bth_crypto_ring_signature::Scalar;
use std::collections::BTreeSet;

fn by_id(owned: &[WalletOutput]) -> std::collections::BTreeMap<[u8; 36], WalletOutput> {
    owned
        .iter()
        .map(|o| (o.utxo.id.to_bytes(), o.clone()))
        .collect()
}

#[test]
fn node_wallet_discovers_repeated_and_nested_v2_payouts() {
    let f = Fixture::build();
    let owned = f.ledger.discover_wallet_outputs(&f.owner).unwrap();
    let found = by_id(&owned);
    let expected: BTreeSet<_> = f.owned_ids().iter().map(UtxoId::to_bytes).collect();
    assert_eq!(found.keys().copied().collect::<BTreeSet<_>>(), expected);

    // Every payout has its own target key and key image: none aliases its
    // source (the #1286 defect), and repeated awards to one source differ.
    let images: BTreeSet<_> = owned.iter().map(|o| o.key_image).collect();
    assert_eq!(images.len(), owned.len());
    let targets: BTreeSet<_> = owned.iter().map(|o| o.utxo.output.target_key).collect();
    assert_eq!(targets.len(), owned.len());
    for (id, amount) in &f.payouts {
        let payout = &found[&id.to_bytes()];
        assert!(payout.lottery_payout);
        assert_eq!(payout.utxo.output.amount, *amount);
        assert_eq!(payout.subaddress, 0);
        assert!(payout.is_unspent());
    }
    for id in [f.hybrid_source, f.classical_source] {
        assert!(!found[&id.to_bytes()].lottery_payout);
    }
    // The nested payouts keep their lineage's base index (the hybrid source's
    // nonzero output index 3).
    for id in [f.repeated[0], f.nested, f.nested_twice] {
        match f.ledger.get_derivation_context(&id).unwrap() {
            StoredContext::Lottery { context, .. } => assert_eq!(context.base_index, 3),
            other => panic!("unexpected context {other:?}"),
        }
    }
    let total: u64 = f
        .ledger
        .unspent_wallet_outputs(&f.owner)
        .unwrap()
        .iter()
        .map(|o| o.utxo.output.amount)
        .sum();
    assert_eq!(total, f.owned_total());

    // A different wallet discovers none of them.
    let other = f.ledger.discover_wallet_outputs(&f.other).unwrap();
    assert!(other.iter().all(|o| !o.lottery_payout));
    assert!(other
        .iter()
        .all(|o| !expected.contains(&o.utxo.id.to_bytes())));

    // Per-block payout discovery (the faucet history path) agrees.
    let b1 = f.ledger.get_block(SEEDED_HEIGHT + 1).unwrap();
    let in_block = f.ledger.wallet_payouts_in_block(&f.owner, &b1).unwrap();
    assert_eq!(
        in_block.iter().map(|o| o.utxo.id).collect::<Vec<_>>(),
        vec![f.repeated[0], f.repeated[1], f.classical_payout]
    );
    let b_pad = f.ledger.get_block(TIP).unwrap();
    assert!(f
        .ledger
        .wallet_payouts_in_block(&f.owner, &b_pad)
        .unwrap()
        .is_empty());
}

#[test]
fn node_wallet_spends_sources_and_payouts_with_distinct_key_images() {
    let f = Fixture::build();
    let discovered = by_id(&f.ledger.discover_wallet_outputs(&f.owner).unwrap());
    // Source and its two repeated payouts, the classical pair, and both
    // nested levels: every one is spent in the same block, which a shared
    // key image would make impossible.
    let mut spends = Vec::new();
    let mut mempool = Mempool::new();
    for id in f.owned_ids() {
        let tx = f.spend(&id);
        let image = tx.inputs.clsag()[0].key_image;
        assert_eq!(image, discovered[&id.to_bytes()].key_image);
        tx.verify_ring_signatures().unwrap();
        f.ledger.verify_transaction(&tx).unwrap();
        f.ledger.verify_ring_members(&tx).unwrap();
        mempool.add_tx(tx.clone(), &f.ledger).unwrap();
        spends.push(tx);
    }
    let images: BTreeSet<_> = spends
        .iter()
        .map(|tx| tx.inputs.clsag()[0].key_image)
        .collect();
    assert_eq!(images.len(), spends.len());

    let block = f.block_with(spends);
    f.ledger.add_block(&block).unwrap();
    let after = f.ledger.discover_wallet_outputs(&f.owner).unwrap();
    for id in f.owned_ids() {
        let output = after.iter().find(|o| o.utxo.id == id).unwrap();
        assert_eq!(output.spent_at, Some(TIP + 1), "{id:?}");
    }
    assert!(f
        .ledger
        .unspent_wallet_outputs(&f.owner)
        .unwrap()
        .is_empty());
    // A spent payout cannot be re-signed.
    let payout = f.utxo(&f.nested);
    let error = f
        .ledger
        .recover_wallet_input(&f.owner, &payout)
        .unwrap_err();
    assert!(error.to_string().contains("already spent"), "{error}");
}

#[test]
fn either_spend_order_of_source_and_payout_is_accepted() {
    for payout_first in [false, true] {
        let f = Fixture::build();
        let order = if payout_first {
            [f.repeated[0], f.hybrid_source]
        } else {
            [f.hybrid_source, f.repeated[0]]
        };
        for id in order {
            let tx = f.spend(&id);
            f.ledger.verify_transaction(&tx).unwrap();
            let block = f.block_with(vec![tx]);
            f.ledger.add_block(&block).unwrap();
        }
        let unspent: BTreeSet<_> = f
            .ledger
            .unspent_wallet_outputs(&f.owner)
            .unwrap()
            .iter()
            .map(|o| o.utxo.id.to_bytes())
            .collect();
        assert!(!unspent.contains(&f.hybrid_source.to_bytes()));
        assert!(!unspent.contains(&f.repeated[0].to_bytes()));
        // The other repeated award and the nested descendants stay spendable.
        for id in [f.repeated[1], f.nested, f.nested_twice] {
            assert!(unspent.contains(&id.to_bytes()));
        }
    }
}

#[test]
fn recovered_payout_key_passes_the_xg_check_and_is_not_the_source_key() {
    let f = Fixture::build();
    let source = f.utxo(&f.hybrid_source);
    let source_key = f.ledger.recover_wallet_input(&f.owner, &source).unwrap();
    for id in [f.repeated[0], f.repeated[1], f.nested, f.nested_twice] {
        let payout = f.utxo(&id);
        let key = f.ledger.recover_wallet_input(&f.owner, &payout).unwrap();
        assert_eq!(
            RistrettoPublic::from(&key).to_bytes(),
            payout.output.target_key
        );
        assert_ne!(key.to_bytes(), source_key.to_bytes());
        // The classical scan alone cannot see a V2 payout.
        assert!(f
            .owner
            .scan_output(&payout.output, payout.id.output_index)
            .and_then(|s| f.owner.recover_output_spend_key(
                &payout.output,
                s,
                payout.id.output_index
            ))
            .is_none_or(|k| RistrettoPublic::from(&k).to_bytes() != payout.output.target_key));
    }
    // A claim that differs from the persisted output is refused.
    let mut forged = f.utxo(&f.repeated[0]);
    forged.output.amount += 1;
    assert!(f.ledger.recover_wallet_input(&f.owner, &forged).is_err());
    // Another wallet cannot recover the payout.
    assert!(f
        .ledger
        .recover_wallet_input(&f.other, &f.utxo(&f.repeated[0]))
        .is_err());
}

#[test]
fn legacy_aliased_rows_stay_excluded() {
    let f = Fixture::build();
    // A V1-shaped alias: a second outpoint carrying the hybrid source's exact
    // output (same target key), indexed after the original.
    let original = f.utxo(&f.hybrid_source);
    let alias = Utxo {
        id: UtxoId::new([0xcc; 32], 3),
        output: original.output.clone(),
        created_at: original.created_at,
    };
    let mut txn = f.ledger.env.write_txn().unwrap();
    f.ledger
        .utxo_db
        .put(
            &mut txn,
            &alias.id.to_bytes(),
            &bincode::serialize(&alias).unwrap(),
        )
        .unwrap();
    let context = StoredContext::Direct {
        height: alias.created_at,
        base_index: 3,
    };
    f.ledger
        .contexts_db
        .put(&mut txn, &alias.id.to_bytes(), &context.encode().unwrap())
        .unwrap();
    f.ledger.add_to_address_index(&mut txn, &alias).unwrap();
    txn.commit().unwrap();

    let owned = f.ledger.discover_wallet_outputs(&f.owner).unwrap();
    assert!(owned.iter().all(|o| o.utxo.id != alias.id));
    assert!(owned.iter().any(|o| o.utxo.id == f.hybrid_source));
    assert_eq!(
        f.ledger
            .unspent_wallet_outputs(&f.owner)
            .unwrap()
            .iter()
            .map(|o| o.utxo.output.amount)
            .sum::<u64>(),
        f.owned_total()
    );
    assert!(f.ledger.validate_input(&alias).is_err());
}

#[test]
fn missing_context_is_an_error_not_a_direct_default() {
    let f = Fixture::build();
    let payout = f.utxo(&f.nested);
    let mut txn = f.ledger.env.write_txn().unwrap();
    f.ledger
        .contexts_db
        .delete(&mut txn, &payout.id.to_bytes())
        .unwrap();
    txn.commit().unwrap();
    assert!(matches!(
        f.ledger.discover_wallet_outputs(&f.owner),
        Err(LedgerError::MissingContext(_))
    ));
    assert!(f.ledger.recover_wallet_input(&f.owner, &payout).is_err());
}

/// The shared recovery reproduces the committed clsag `lottery_v2` vectors:
/// the single derivation, the repeated award, and the three-level lineage
/// (cumulative tweaks over one base key).
#[test]
fn shared_recovery_reproduces_clsag_lottery_v2_vectors() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../transaction/clsag/tests/fixtures/lottery-v2.json"
    ))
    .unwrap();
    let hex32 = |v: &serde_json::Value| -> [u8; 32] {
        hex::decode(v.as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap()
    };
    let base = RistrettoPrivate::from(Scalar::from(7u64));
    let base_target = RistrettoPublic::from(&base).to_bytes();
    assert_eq!(hex32(&vectors["source_target"]), base_target);
    let public_key = RistrettoPublic::from(&RistrettoPrivate::from(Scalar::from(11u64))).to_bytes();
    let recover = |target: [u8; 32], tweak: [u8; 32], base_key: &RistrettoPrivate| {
        let output = TxOutput {
            amount: 900,
            target_key: target,
            public_key,
            e_memo: None,
            cluster_tags: Default::default(),
            kem_ciphertext: None,
        };
        recover_with_base(
            &output,
            &Context {
                base_index: 3,
                tweak,
            },
            |b, index| {
                assert_eq!(index, 3);
                (b.target_key == RistrettoPublic::from(base_key).to_bytes())
                    .then_some((0, *base_key))
            },
        )
    };
    let mut cases = vec![(hex32(&vectors["target"]), hex32(&vectors["context_tweak"]))];
    cases.push((
        hex32(&vectors["repeated"]["target"]),
        hex32(&vectors["repeated"]["tweak"]),
    ));
    for step in vectors["lineage"].as_array().unwrap() {
        assert_eq!(step["base_index"], 3);
        cases.push((hex32(&step["target"]), hex32(&step["tweak"])));
    }
    let mut images = BTreeSet::new();
    for (target, tweak) in cases {
        let (_, key) = recover(target, tweak, &base).unwrap();
        assert_eq!(RistrettoPublic::from(&key).to_bytes(), target);
        let base_scalar: &Scalar = base.as_ref();
        let key_scalar: &Scalar = key.as_ref();
        assert_eq!(
            *key_scalar,
            base_scalar + Scalar::from_canonical_bytes(tweak).unwrap()
        );
        images.insert(*KeyImage::from(&key).as_bytes());
        // The wrong base key never yields a key (the xG = P check).
        let wrong = RistrettoPrivate::from(Scalar::from(8u64));
        assert_eq!(
            recover(target, tweak, &wrong).unwrap_err(),
            clsag_v2::Error::Ownership
        );
    }
    images.insert(*KeyImage::from(&base).as_bytes());
    assert_eq!(images.len(), 6, "every payout key image differs");
}

/// Acceptance at real 720-block maturity through the production `add_block`
/// path (#1644). GATED ON P3 (#1640): until protocol 7.0.0 activation,
/// `add_block` keeps V1 rules (V1 `tx_root`,
/// `validate_lottery_output_bindings`) and refuses every block carrying lottery
/// payouts, so this cannot run on the P2 base. It compiles against today's APIs
/// and is skipped via `#[ignore]`; un-ignore it once P3 is on the branch (the
/// only P3 seam it relies on is `add_block` accepting the block
/// `lottery_v2::derive` produces).
///
/// The owner is the sole minter and owner of every output, so (1) every draw
/// winner is the owner's, and (2) the wallet balance must equal
/// `total_mined - total_fees_burned - lottery_pool` exactly.
#[test]
#[ignore = "needs #1640 (P3): add_block refuses V2 payout blocks until protocol 7.0.0 activation"]
fn node_wallet_v2_payout_at_real_720_block_maturity() {
    use crate::{block::calculate_block_reward, ledger::lottery_v2};
    const MAX_HEIGHT: u64 = 1_100;
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    let owner = Wallet::from_mnemonic(OWNER_WORDS).unwrap();
    assert_eq!(lottery_v2::canonical_config().draw_config.min_utxo_age, 720);

    // (payout, its source) once a win is discovered; spent ten blocks later.
    let mut win: Option<(UtxoId, UtxoId, u64)> = None;
    let mut spent_pair = false;
    let mut nested_or_repeated = false;
    while ledger.get_chain_state().unwrap().height < MAX_HEIGHT && !spent_pair {
        let state = ledger.get_chain_state().unwrap();
        let height = state.height;
        let unspent = ledger.unspent_wallet_outputs(&owner).unwrap();
        let mature = |o: &&WalletOutput| height.saturating_sub(o.utxo.created_at) >= 10;
        let mut transactions = Vec::new();
        match win {
            Some((payout, source, at)) if height >= at + 10 => {
                // Spend the payout and its source in the same block: distinct
                // key images or the block is a double spend.
                for id in [payout, source] {
                    let input = unspent.iter().find(|o| o.utxo.id == id).unwrap();
                    let output =
                        TxOutput::new(input.utxo.output.amount - FEE, &owner.default_address());
                    transactions.push(
                        owner
                            .create_private_transaction(
                                std::slice::from_ref(&input.utxo),
                                vec![output],
                                FEE,
                                height,
                                &ledger,
                            )
                            .unwrap(),
                    );
                }
                let images: BTreeSet<_> = transactions
                    .iter()
                    .map(|tx| tx.inputs.clsag()[0].key_image)
                    .collect();
                assert_eq!(images.len(), 2);
                spent_pair = true;
            }
            _ if height > 720 => {
                // One fee-paying self-spend per block funds the lottery pool;
                // never spend the tracked pair early.
                let tracked: Vec<UtxoId> = win.iter().flat_map(|(p, s, _)| [*p, *s]).collect();
                if let Some(input) = unspent
                    .iter()
                    .filter(mature)
                    .find(|o| !o.lottery_payout && !tracked.contains(&o.utxo.id))
                {
                    let output =
                        TxOutput::new(input.utxo.output.amount - FEE, &owner.default_address());
                    transactions.push(
                        owner
                            .create_private_transaction(
                                std::slice::from_ref(&input.utxo),
                                vec![output],
                                FEE,
                                height,
                                &ledger,
                            )
                            .unwrap(),
                    );
                }
            }
            _ => {}
        }
        let tip = ledger.get_block(height).unwrap();
        let template = Block::new_template_with_txs(
            &tip,
            &owner.default_address(),
            state.difficulty,
            calculate_block_reward(height + 1, state.total_mined),
            transactions,
        );
        let mut block = {
            let txn = ledger.env.read_txn().unwrap();
            lottery_v2::derive(&ledger.production_view(&txn), template)
                .unwrap()
                .0
        };
        while !block.header.is_valid_pow() {
            block.header.nonce += 1;
        }
        block.minting_tx.nonce = block.header.nonce;
        ledger.add_block(&block).unwrap();

        for payout in ledger.wallet_payouts_in_block(&owner, &block).unwrap() {
            let ordinal = payout.utxo.id.output_index as usize - 1;
            let winner = &block.lottery_outputs[ordinal];
            let source = UtxoId::new(winner.winner_tx_hash, winner.winner_output_index);
            let source_output = ledger.get_utxo(&source).unwrap().unwrap();
            assert_ne!(
                payout.utxo.output.target_key,
                source_output.output.target_key
            );
            let source_image = ledger
                .discover_wallet_outputs(&owner)
                .unwrap()
                .into_iter()
                .find(|o| o.utxo.id == source)
                .unwrap()
                .key_image;
            assert_ne!(payout.key_image, source_image);
            if matches!(
                ledger.get_derivation_context(&source).unwrap(),
                StoredContext::Lottery { .. }
            ) || block
                .lottery_outputs
                .iter()
                .filter(|o| o.winner_utxo_id() == winner.winner_utxo_id())
                .count()
                > 1
            {
                nested_or_repeated = true;
            }
            // Track the first win whose (direct) source is still unspent.
            let source_unspent = ledger
                .unspent_wallet_outputs(&owner)
                .unwrap()
                .iter()
                .any(|o| o.utxo.id == source);
            if win.is_none()
                && source_unspent
                && matches!(
                    ledger.get_derivation_context(&source).unwrap(),
                    StoredContext::Direct { .. }
                )
            {
                win = Some((payout.utxo.id, source, block.height()));
            }
        }
    }
    assert!(
        spent_pair,
        "no V2 win discovered and spent by height {MAX_HEIGHT}"
    );
    let state = ledger.get_chain_state().unwrap();
    let balance: u128 = ledger
        .unspent_wallet_outputs(&owner)
        .unwrap()
        .iter()
        .map(|o| o.utxo.output.amount as u128)
        .sum();
    assert_eq!(
        balance,
        state.total_mined - state.total_fees_burned - ledger.get_lottery_pool().unwrap()
    );
    // Natural draws make repeated/nested awards likely but not certain within
    // the bound; the deterministic fixture tests above pin both.
    if !nested_or_repeated {
        eprintln!("note: no repeated/nested award occurred naturally in this run");
    }
}
