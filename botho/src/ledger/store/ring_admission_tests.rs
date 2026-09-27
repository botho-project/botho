//! Synthetic signed-ring regressions against real LMDB storage.
use super::*;
use crate::{
    mempool::{Mempool, MempoolError},
    transaction::{ClsagRingInput, Transaction, TxOutput, MIN_RING_SIZE},
};
use tempfile::tempdir;

/// Reproduce the admission boundary with an actual LMDB ledger and a
/// fully signed ring. A later lottery payout reuses a decoy's target and
/// public key but changes its amount; canonical target lookup still finds
/// the first output. The malformed ring is signed, not mutated afterward.
#[test]
fn admission_rejects_signed_lottery_alias_ring_and_accepts_canonical_control() {
    use crate::{
        ledger::{ChainState, LedgerError, UtxoSnapshot},
        transaction::{Utxo, UtxoId},
    };
    use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
    use bth_util_from_random::{FromRandom, OsRng};

    const AMOUNT: u64 = 50_000_000_000_000;
    const FEE: u64 = 1_000_000_000_000;
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let mut rng = OsRng;
    let mut secrets = Vec::new();
    let mut utxos = Vec::new();
    for index in 0..MIN_RING_SIZE {
        let secret = RistrettoPrivate::from_random(&mut rng);
        let output = TxOutput {
            amount: AMOUNT,
            target_key: RistrettoPublic::from(&secret).to_bytes(),
            public_key: RistrettoPublic::from(&RistrettoPrivate::from_random(&mut rng)).to_bytes(),
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
            kem_ciphertext: None,
        };
        utxos.push(Utxo {
            id: UtxoId::new([index as u8 + 1; 32], 0),
            output,
            created_at: 1,
        });
        secrets.push(secret);
    }
    let canonical_ring: Vec<_> = utxos
        .iter()
        .map(|u| RingMember::from_output(&u.output))
        .collect();
    let mut payout = utxos[9].clone();
    payout.id = UtxoId::new([0xff; 32], 1);
    payout.created_at = 90;
    payout.output.amount = 20_000_000;
    let payout_member = RingMember::from_output(&payout.output);
    utxos.push(payout);
    let state = ChainState {
        height: 100,
        difficulty: u64::MAX,
        ..ChainState::default()
    };
    let snapshot = UtxoSnapshot::new(100, [0; 32], state, utxos, vec![], vec![]).unwrap();
    ledger.load_from_snapshot(&snapshot, None).unwrap();
    let canonical = ledger
        .get_utxo_by_target_key(&payout_member.target_key)
        .unwrap()
        .unwrap();
    assert_eq!(canonical.output.amount, AMOUNT);
    assert_eq!(canonical_ring[9].target_key, payout_member.target_key);
    assert_eq!(canonical_ring[9].public_key, payout_member.public_key);
    assert_ne!(canonical_ring[9].commitment, payout_member.commitment);

    let output = TxOutput {
        amount: AMOUNT - FEE,
        target_key: RistrettoPublic::from(&RistrettoPrivate::from_random(&mut rng)).to_bytes(),
        public_key: RistrettoPublic::from(&RistrettoPrivate::from_random(&mut rng)).to_bytes(),
        e_memo: None,
        cluster_tags: ClusterTagVector::empty(),
        kem_ciphertext: None,
    };
    let unsigned = Transaction::new_clsag(vec![], vec![output.clone()], FEE, 100);
    let mut sign = |ring| {
        let input = ClsagRingInput::new(
            ring,
            0,
            &secrets[0],
            AMOUNT,
            &unsigned.signing_hash(),
            &mut rng,
        )
        .unwrap();
        Transaction::new_clsag(vec![input], vec![output.clone()], FEE, 100)
    };
    let mut alias_ring = canonical_ring.clone();
    alias_ring[9] = payout_member;
    let invalid = sign(alias_ring);
    let valid = sign(canonical_ring.clone());
    let mut public_key_mismatch = canonical_ring.clone();
    public_key_mismatch[9].public_key = output.public_key;
    let wrong_public_key = sign(public_key_mismatch);
    let mut missing_target = canonical_ring;
    missing_target[9].target_key = output.target_key;
    let missing = sign(missing_target);
    invalid.is_valid_structure().unwrap();
    invalid.verify_ring_signatures().unwrap();
    ledger.verify_transaction(&invalid).unwrap();
    valid.verify_ring_signatures().unwrap();
    assert!(matches!(
        ledger.verify_ring_members(&invalid),
        Err(LedgerError::InvalidBlock(message)) if message.contains("ring member 9 does not match UTXO")
    ));
    ledger.verify_ring_members(&valid).unwrap();

    // Exercise the actual block application boundary, not only its helper.
    let invalid_block = signed_test_block(&ledger, invalid.clone());
    assert!(matches!(ledger.add_block(&invalid_block),
        Err(LedgerError::InvalidBlock(message)) if message.contains("ring member 9 does not match UTXO")));
    assert_eq!(ledger.get_chain_state().unwrap().height, 100);
    assert_eq!(
        ledger
            .is_key_image_spent(&invalid.inputs.clsag()[0].key_image)
            .unwrap(),
        None
    );

    let mut mempool = Mempool::new();
    let result = mempool.add_tx(invalid, &ledger);
    assert!(matches!(
        result,
        Err(MempoolError::InvalidTransaction(message)) if message.contains("ring member 9 does not match UTXO")
    ));
    assert!(mempool.is_empty());
    for (tx, expected) in [
        (wrong_public_key, "does not match UTXO"),
        (missing, "target_key not in UTXO set"),
    ] {
        tx.verify_ring_signatures().unwrap();
        assert!(
            matches!(ledger.add_block(&signed_test_block(&ledger, tx.clone())),
            Err(LedgerError::InvalidBlock(message)) if message.contains(expected))
        );
        assert!(matches!(mempool.add_tx(tx, &ledger),
            Err(MempoolError::InvalidTransaction(message)) if message.contains(expected)));
        assert!(mempool.is_empty());
    }

    // Fail closed on a single bad index even when every other member resolves.
    // This used to silently remove a member from amount/tag/fee collection.
    let target = canonical.output.target_key;
    let original_index = {
        let rtxn = ledger.env.read_txn().unwrap();
        ledger
            .address_index_db
            .get(&rtxn, &target)
            .unwrap()
            .unwrap()
            .to_vec()
    };
    for index in [
        vec![],
        vec![0; 35],
        UtxoId::new([0xee; 32], 0).to_bytes().to_vec(),
    ] {
        let mut wtxn = ledger.env.write_txn().unwrap();
        ledger
            .address_index_db
            .put(&mut wtxn, &target, &index)
            .unwrap();
        wtxn.commit().unwrap();
        assert!(matches!(
            mempool.add_tx(valid.clone(), &ledger),
            Err(MempoolError::LedgerError(_))
        ));
        assert!(mempool.is_empty());
    }
    let mut wtxn = ledger.env.write_txn().unwrap();
    ledger
        .address_index_db
        .put(&mut wtxn, &target, &original_index)
        .unwrap();
    wtxn.commit().unwrap();

    // A target index pointing at a record with a different target is a mismatch,
    // even if the record's public key and commitment still match the ring.
    let mut corrupt = canonical.clone();
    corrupt.output.target_key = output.target_key;
    let mut wtxn = ledger.env.write_txn().unwrap();
    ledger
        .utxo_db
        .put(
            &mut wtxn,
            &canonical.id.to_bytes(),
            &bincode::serialize(&corrupt).unwrap(),
        )
        .unwrap();
    wtxn.commit().unwrap();
    assert!(matches!(mempool.add_tx(valid.clone(), &ledger),
        Err(MempoolError::InvalidTransaction(message)) if message.contains("does not match UTXO")));
    let mut wtxn = ledger.env.write_txn().unwrap();
    ledger
        .utxo_db
        .put(
            &mut wtxn,
            &canonical.id.to_bytes(),
            &bincode::serialize(&canonical).unwrap(),
        )
        .unwrap();
    wtxn.commit().unwrap();

    // Same real input remains admissible when all ring members correspond
    // to canonical ledger outputs; rejection must not reserve its key image.
    let valid_hash = valid.hash();
    assert_eq!(mempool.add_tx(valid.clone(), &ledger).unwrap(), valid_hash);
    assert_eq!(mempool.len(), 1);
    let key_image = valid.inputs.clsag()[0].key_image;
    ledger
        .add_block(&signed_test_block(&ledger, valid.clone()))
        .unwrap();
    assert_eq!(ledger.get_chain_state().unwrap().height, 101);
    assert_eq!(ledger.is_key_image_spent(&key_image).unwrap(), Some(101));
    assert!(matches!(
        Mempool::new().add_tx(valid, &ledger),
        Err(MempoolError::KeyImageSpent(_))
    ));
}

#[test]
fn target_key_lookup_fails_closed_on_malformed_or_dangling_index() {
    let dir = tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let target = [0x42; 32];
    let missing = UtxoId::new([0x11; 32], 0).to_bytes();
    for index in [vec![], vec![0; 35], vec![0; 37], missing.to_vec()] {
        let mut wtxn = ledger.env.write_txn().unwrap();
        ledger
            .address_index_db
            .put(&mut wtxn, &target, &index)
            .unwrap();
        wtxn.commit().unwrap();
        assert!(matches!(
            ledger.get_utxo_by_target_key(&target),
            Err(LedgerError::Database(_))
        ));
    }
}

#[test]
fn target_key_lookup_fails_closed_on_corrupt_indexed_output() {
    let dir = tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let target = [0x42; 32];
    let id = UtxoId::new([0x11; 32], 0).to_bytes();
    let mut wtxn = ledger.env.write_txn().unwrap();
    ledger
        .address_index_db
        .put(&mut wtxn, &target, &id)
        .unwrap();
    ledger.utxo_db.put(&mut wtxn, &id, b"truncated").unwrap();
    wtxn.commit().unwrap();
    assert!(matches!(
        ledger.get_utxo_by_target_key(&target),
        Err(LedgerError::Serialization(_))
    ));
}

#[test]
fn shared_ring_member_validation_propagates_target_lookup_read_errors() {
    let dir = tempdir().unwrap();
    let ledger = Ledger::open_single_reader(dir.path()).unwrap();
    let _held = ledger.read_txn_for_test().unwrap();
    assert!(matches!(
        ledger.verify_ring_members(&tx_with_key_image([0x33; 32])),
        Err(LedgerError::Database(_))
    ));
}

fn tx_with_key_image(key_image: [u8; 32]) -> BothoTransaction {
    use crate::transaction::ClsagRingInput;
    let input = ClsagRingInput {
        ring: vec![RingMember {
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            commitment: [0u8; 32],
        }],
        key_image,
        commitment_key_image: [0u8; 32],
        clsag_signature: Vec::new(),
        pseudo_output_amount: 0,
    };
    BothoTransaction::new(vec![input], vec![], 0, 0)
}

/// Build a real trivial-PoW block against the synthetic snapshot. All fixture
/// outputs are younger than the default lottery eligibility age, so the pool
/// carries forward and only the normal fee burn is included in the summary.
fn signed_test_block(ledger: &Ledger, tx: Transaction) -> Block {
    use bth_account_keys::AccountKey;
    use rand::{rngs::StdRng, SeedableRng};
    let state = ledger.get_chain_state().unwrap();
    let minter = AccountKey::random(&mut StdRng::seed_from_u64(1444));
    let mut block = Block::new_template_with_txs(
        &Block::genesis(),
        &minter.default_subaddress(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        vec![tx],
    );
    block.header.height = state.height + 1;
    block.header.prev_block_hash = state.tip_hash;
    block.minting_tx.block_height = block.header.height;
    block.minting_tx.prev_block_hash = state.tip_hash;
    block.lottery_summary.amount_burned =
        LotteryFeeConfig::default().split_fees(block.total_fees()).1;
    while !block.header.is_valid_pow() {
        block.header.nonce += 1;
    }
    block.minting_tx.nonce = block.header.nonce;
    block
}
