//! Protocol 10.0.0 (#1684 / #1772, ADR 0014): the structural transaction rules
//! are consensus rules at block application.
//!
//! Every transaction here is fully signed against real LMDB ring members, so
//! the structural rule under test is the only thing wrong with a rejected one.
//! Each boundary is exercised on both sides: through `add_block` (the block
//! validator, `validate_ordinary_body` C9) and through
//! `Ledger::check_transfer_for_block` (the per-transaction
//! `verify_transfer_for_block` the block builder and the SCP slot check use).
//! The SCP value-validation side is in `consensus::validation` tests.
use super::{ring_admission_tests::signed_test_block, *};
use crate::{
    ledger::{LedgerError, TransferBlockCheck, UtxoSnapshot},
    transaction::{
        ClsagRingInput, Transaction, TxOutput, Utxo, UtxoId, DUST_THRESHOLD, MIN_RING_SIZE,
    },
};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_util_from_random::{FromRandom, OsRng};

const AMOUNT: u64 = 50_000_000_000_000;
const FEE: u64 = 1_000_000_000_000;
const MAX_OUTPUTS: usize = bth_transaction_types::MAX_OUTPUTS as usize;
const STRUCTURAL: &str = "structural consensus rule violated";

/// A ledger at height 100 holding `MIN_RING_SIZE` spendable outputs of
/// `AMOUNT`, and the one-time secret of each.
struct Fixture {
    _dir: tempfile::TempDir,
    ledger: Ledger,
    secrets: Vec<RistrettoPrivate>,
    ring: Vec<RingMember>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let mut rng = OsRng;
    let mut secrets = Vec::new();
    let mut utxos = Vec::new();
    for index in 0..MIN_RING_SIZE {
        let secret = RistrettoPrivate::from_random(&mut rng);
        utxos.push(Utxo {
            id: UtxoId::new([index as u8 + 1; 32], 0),
            output: TxOutput {
                amount: AMOUNT,
                target_key: RistrettoPublic::from(&secret).to_bytes(),
                public_key: RistrettoPublic::from(&RistrettoPrivate::from_random(&mut rng))
                    .to_bytes(),
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
                kem_ciphertext: None,
            },
            created_at: 1,
        });
        secrets.push(secret);
    }
    let ring = utxos
        .iter()
        .map(|u| RingMember::from_output(&u.output))
        .collect();
    let state = ChainState {
        height: 100,
        difficulty: u64::MAX,
        ..ChainState::default()
    };
    let snapshot = UtxoSnapshot::new(100, [0; 32], state, utxos, vec![], vec![]).unwrap();
    ledger.seed_fixture_snapshot_unvalidated(&snapshot).unwrap();
    Fixture {
        _dir: dir,
        ledger,
        secrets,
        ring,
    }
}

/// A shape-valid output of `amount` (ML-KEM ciphertext present, no tags).
fn output(amount: u64) -> TxOutput {
    let mut rng = OsRng;
    TxOutput {
        amount,
        target_key: RistrettoPublic::from(&RistrettoPrivate::from_random(&mut rng)).to_bytes(),
        public_key: RistrettoPublic::from(&RistrettoPrivate::from_random(&mut rng)).to_bytes(),
        e_memo: None,
        cluster_tags: ClusterTagVector::empty(),
        kem_ciphertext: Some(vec![0x5C; crate::consensus::ML_KEM_768_CIPHERTEXT_BYTES]),
    }
}

/// `count` outputs summing to `AMOUNT - FEE`: `count - 1` outputs of `each`
/// plus one carrying the remainder, so the transaction balances.
fn outputs(count: usize, each: u64) -> Vec<TxOutput> {
    let rest = AMOUNT - FEE - each * (count as u64 - 1);
    let mut outs: Vec<_> = (1..count).map(|_| output(each)).collect();
    outs.push(output(rest));
    outs
}

/// Sign a transaction spending ring member 0 with the first `ring_len` ring
/// members as its ring. `ClsagRingInput::new` refuses a ring below
/// `MIN_RING_SIZE`, so this signs with `Clsag::sign` directly and serializes
/// the signature the same way (`c_zero || responses`): a genuine signature
/// over a 19-member ring, not a 20-member one truncated afterwards.
fn signed(fx: &Fixture, ring_len: usize, outs: Vec<TxOutput>) -> Transaction {
    use bth_crypto_ring_signature::{generators, Clsag, Scalar};
    let mut rng = OsRng;
    let ring: Vec<RingMember> = fx.ring[..ring_len].to_vec();
    let message = Transaction::new_clsag(vec![], outs.clone(), FEE, 100).signing_hash();
    let input = if ring_len >= MIN_RING_SIZE {
        ClsagRingInput::new(ring, 0, &fx.secrets[0], AMOUNT, &message, &mut rng).unwrap()
    } else {
        let reduced: Vec<_> = ring
            .iter()
            .map(|m| m.to_reduced_tx_out().unwrap())
            .collect();
        let clsag = Clsag::sign(
            &message,
            &reduced,
            0,
            &fx.secrets[0],
            AMOUNT,
            &Scalar::ZERO,
            &Scalar::ZERO,
            &generators(0),
            &mut rng,
        )
        .unwrap();
        let mut signature: Vec<u8> = AsRef::<[u8]>::as_ref(&clsag.c_zero).to_vec();
        for response in &clsag.responses {
            signature.extend_from_slice(AsRef::<[u8]>::as_ref(response));
        }
        ClsagRingInput {
            ring,
            key_image: *clsag.key_image.as_bytes(),
            commitment_key_image: *clsag.commitment_key_image.as_bytes(),
            clsag_signature: signature,
            pseudo_output_amount: AMOUNT,
        }
    };
    let tx = Transaction::new_clsag(vec![input], outs, FEE, 100);
    // Everything except the structural rule holds: the signatures verify and
    // the transaction balances.
    tx.verify_ring_signatures().unwrap();
    tx
}

/// `add_block` and the per-transaction block check both reject `tx` for the
/// structural rule whose message contains `reason`, and nothing is applied.
fn assert_rejected(fx: &Fixture, tx: Transaction, reason: &str) {
    let state = fx.ledger.get_chain_state().unwrap();
    match fx
        .ledger
        .check_transfer_for_block(&tx, state.height, state.tip_hash)
        .unwrap()
    {
        TransferBlockCheck::Invalid(msg) => {
            assert!(msg.contains(STRUCTURAL) && msg.contains(reason), "{msg}")
        }
        _ => panic!("per-transaction block check must reject: {reason}"),
    }
    let key_image = tx.inputs.clsag()[0].key_image;
    let block = signed_test_block(&fx.ledger, tx);
    match fx.ledger.add_block(&block) {
        Err(LedgerError::InvalidBlock(msg)) => assert!(
            msg.starts_with("Transaction 0: ") && msg.contains(STRUCTURAL) && msg.contains(reason),
            "{msg}"
        ),
        other => panic!("add_block must reject ({reason}): {other:?}"),
    }
    assert_eq!(fx.ledger.get_chain_state().unwrap().height, 100);
    assert_eq!(fx.ledger.is_key_image_spent(&key_image).unwrap(), None);
}

/// `add_block` and the per-transaction block check both accept `tx`.
fn assert_accepted(fx: &Fixture, tx: Transaction) {
    let state = fx.ledger.get_chain_state().unwrap();
    assert!(matches!(
        fx.ledger
            .check_transfer_for_block(&tx, state.height, state.tip_hash)
            .unwrap(),
        TransferBlockCheck::Valid
    ));
    let key_image = tx.inputs.clsag()[0].key_image;
    fx.ledger
        .add_block(&signed_test_block(&fx.ledger, tx))
        .unwrap();
    assert_eq!(fx.ledger.get_chain_state().unwrap().height, 101);
    assert_eq!(fx.ledger.is_key_image_spent(&key_image).unwrap(), Some(101));
}

#[test]
fn block_apply_rejects_ring_of_19_and_accepts_ring_of_20() {
    assert_eq!(MIN_RING_SIZE, 20);
    let fx = fixture();
    let tx = signed(&fx, MIN_RING_SIZE - 1, outputs(1, 0));
    assert_eq!(tx.inputs.clsag()[0].ring.len(), 19);
    assert_rejected(&fx, tx, "Ring size below minimum");

    let fx = fixture();
    assert_accepted(&fx, signed(&fx, MIN_RING_SIZE, outputs(1, 0)));
}

#[test]
fn block_apply_rejects_17_outputs_and_accepts_16() {
    assert_eq!(MAX_OUTPUTS, 16);
    let fx = fixture();
    let tx = signed(&fx, MIN_RING_SIZE, outputs(MAX_OUTPUTS + 1, DUST_THRESHOLD));
    assert_rejected(&fx, tx, "17 outputs, exceeding the maximum of 16");

    let fx = fixture();
    assert_accepted(
        &fx,
        signed(&fx, MIN_RING_SIZE, outputs(MAX_OUTPUTS, DUST_THRESHOLD)),
    );
}

#[test]
fn block_apply_rejects_output_one_below_dust_and_accepts_dust_threshold() {
    let fx = fixture();
    let tx = signed(&fx, MIN_RING_SIZE, outputs(2, DUST_THRESHOLD - 1));
    assert_eq!(tx.outputs[0].amount, DUST_THRESHOLD - 1);
    assert_rejected(&fx, tx, "below dust threshold");

    let fx = fixture();
    let tx = signed(&fx, MIN_RING_SIZE, outputs(2, DUST_THRESHOLD));
    assert_eq!(tx.outputs[0].amount, DUST_THRESHOLD);
    assert_accepted(&fx, tx);
}

/// ADR 0014: `MIN_TX_FEE` is relay policy, not a consensus rule. A block may
/// carry a transfer whose fee is below `MIN_TX_FEE` as long as it meets the
/// size-based `consensus_fee_floor`; only mempool admission refuses it.
#[test]
fn block_apply_does_not_enforce_min_tx_fee() {
    use crate::{
        mempool::{Mempool, MempoolError},
        transaction::MIN_TX_FEE,
    };
    let fx = fixture();
    let fee = MIN_TX_FEE - 1;
    let outs = vec![output(AMOUNT - fee)];
    let message = Transaction::new_clsag(vec![], outs.clone(), fee, 100).signing_hash();
    let input = ClsagRingInput::new(
        fx.ring.clone(),
        0,
        &fx.secrets[0],
        AMOUNT,
        &message,
        &mut OsRng,
    )
    .unwrap();
    let tx = Transaction::new_clsag(vec![input], outs, fee, 100);
    let floor = fx.ledger.consensus_fee_floor(&tx, 101).unwrap();
    assert!(
        floor <= fee,
        "fixture must clear the consensus floor ({floor})"
    );
    tx.check_consensus_structure().unwrap();
    assert!(matches!(
        Mempool::new().add_tx(tx.clone(), &fx.ledger),
        Err(MempoolError::InvalidTransaction(msg)) if msg == "Transaction fee below minimum"
    ));
    assert_accepted(&fx, tx);
}
