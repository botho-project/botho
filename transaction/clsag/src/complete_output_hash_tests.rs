//! #1638 / #1286 D0: every output field is committed to both the transaction
//! ID and the signing hash. Mutating `kem_ciphertext`, `e_memo` or
//! `cluster_tags` on a signed transaction must invalidate its signatures.
use super::*;
use bth_transaction_types::{ClusterId, ClusterTagEntry};

fn full_tags() -> ClusterTagVector {
    // `DecayState` is not re-exported; start from a decay-tracked vector and
    // overwrite every field.
    let mut tags = ClusterTagVector::empty_at_block(0);
    tags.entries = vec![
        ClusterTagEntry {
            cluster_id: ClusterId(0x1112131415161718),
            weight: 0x21222324,
        },
        ClusterTagEntry {
            cluster_id: ClusterId(u64::MAX),
            weight: u32::MAX,
        },
    ];
    let decay = tags.decay_state.as_mut().expect("decay-tracked");
    decay.last_decay_block = 0x3132333435363738;
    decay.decays_this_epoch = 0x41424344;
    decay.epoch_start_block = 0x5152535455565758;
    tags
}

/// Deterministic fixture exercising every optional/variable output field.
fn fixture() -> Transaction {
    let first = TxOutput {
        amount: 0x0102030405060708,
        target_key: std::array::from_fn(|i| i as u8),
        public_key: std::array::from_fn(|i| 255 - i as u8),
        e_memo: Some(EncryptedMemo {
            ciphertext: (0..66).map(|i| (i * 3 + 7) as u8).collect(),
        }),
        cluster_tags: full_tags(),
        kem_ciphertext: Some((0..1088).map(|i| (i * 5 + 11) as u8).collect()),
    };
    let second = TxOutput {
        amount: 0x6162636465666768,
        target_key: [0x71; 32],
        public_key: [0x81; 32],
        e_memo: None,
        cluster_tags: ClusterTagVector::empty(),
        kem_ciphertext: None,
    };
    Transaction::new_settlement(
        vec![],
        vec![first, second],
        0x191a1b1c1d1e1f20,
        0x2122232425262728,
        0x292a2b2c2d2e2f30,
    )
}

fn hex32(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Golden vectors, independently assembled in Python (`struct.pack('<Q'/'<I')`,
/// raw key arrays, one-byte option tags, u64 length/count prefixes, then
/// `hashlib.sha256`). Changing either value is a consensus change.
#[test]
fn complete_output_golden_vectors() {
    let tx = fixture();
    assert_eq!(
        hex32(tx.signing_hash()),
        "1840e25dfbb7dd231d16a81a0b66cd45946bf88c984eaeaf073cf1727977c6e1",
        "signing_hash golden vector"
    );
    assert_eq!(
        hex32(tx.hash()),
        "939fe53e5c6adc54cf46df976b844c97af0559ebdd11a8064ba78e880be773d7",
        "tx hash golden vector"
    );
}

/// One-input golden vector: covers the key-image prefix of `hash()` with a
/// nonzero count (and the `None` settlement tag). Independently assembled in
/// Python the same way as `complete_output_golden_vectors`: `hash()` =
/// SHA-256(domain || u64 1 || key_image || body), `signing_hash()` =
/// SHA-256(domain || body). Ring members, the commitment key image, the
/// signature bytes and `pseudo_output_amount` are not part of either
/// transcript, so they are filled with unrelated bytes here.
#[test]
fn complete_output_one_input_golden_vectors() {
    let base = fixture();
    let input = ClsagRingInput {
        ring: vec![RingMember {
            target_key: [0xd1; 32],
            public_key: [0xd2; 32],
            commitment: [0xd3; 32],
        }],
        key_image: std::array::from_fn(|i| 0xc0u8.wrapping_add(i as u8)),
        commitment_key_image: [0xe1; 32],
        clsag_signature: vec![0xf1; 96],
        pseudo_output_amount: 0x7172737475767778,
    };
    let tx = Transaction::new_clsag(
        vec![input],
        base.outputs.clone(),
        base.fee,
        base.created_at_height,
    );
    assert_eq!(
        hex32(tx.signing_hash()),
        "1f11b712b988434d78e1100dd1a8ad5e22783cd7d4e0d4f769f9d42eb3cacc94",
        "one-input signing_hash golden vector"
    );
    assert_eq!(
        hex32(tx.hash()),
        "b2719f58bdd1255c27e0c80991dd1b207d75233ef6b77b63f37db95b89c51497",
        "one-input tx hash golden vector"
    );
    // The key image is committed to the ID but not to the signing hash.
    let mut other = tx.clone();
    other.inputs = TxInputs::new(vec![ClsagRingInput {
        key_image: [0; 32],
        ..tx.inputs.clsag()[0].clone()
    }]);
    assert_ne!(other.hash(), tx.hash(), "key image changes tx hash");
    assert_eq!(
        other.signing_hash(),
        tx.signing_hash(),
        "key image is excluded from the signing hash"
    );
}

/// Every individual output-field mutation changes both hashes.
#[test]
fn every_output_field_changes_both_hashes() {
    type Mutation = fn(&mut TxOutput);
    let mutations: &[(&str, Mutation)] = &[
        ("amount", |o| o.amount ^= 1),
        ("target_key", |o| o.target_key[0] ^= 1),
        ("public_key", |o| o.public_key[0] ^= 1),
        ("e_memo byte", |o| {
            o.e_memo.as_mut().unwrap().ciphertext[0] ^= 1
        }),
        ("e_memo strip", |o| o.e_memo = None),
        ("cluster tag id", |o| {
            o.cluster_tags.entries[0].cluster_id.0 ^= 1
        }),
        ("cluster tag weight", |o| {
            o.cluster_tags.entries[0].weight ^= 1
        }),
        ("cluster tag drop", |o| {
            o.cluster_tags.entries.pop();
        }),
        ("decay last block", |o| {
            o.cluster_tags
                .decay_state
                .as_mut()
                .unwrap()
                .last_decay_block ^= 1
        }),
        ("decay count", |o| {
            o.cluster_tags
                .decay_state
                .as_mut()
                .unwrap()
                .decays_this_epoch ^= 1
        }),
        ("decay epoch start", |o| {
            o.cluster_tags
                .decay_state
                .as_mut()
                .unwrap()
                .epoch_start_block ^= 1
        }),
        ("decay strip", |o| o.cluster_tags.decay_state = None),
        ("kem byte", |o| {
            o.kem_ciphertext.as_mut().unwrap()[1087] ^= 1
        }),
        ("kem truncate", |o| {
            o.kem_ciphertext.as_mut().unwrap().pop();
        }),
        ("kem strip", |o| o.kem_ciphertext = None),
    ];
    let base = fixture();
    for (name, mutate) in mutations {
        let mut tx = base.clone();
        mutate(&mut tx.outputs[0]);
        assert_ne!(
            tx.signing_hash(),
            base.signing_hash(),
            "{name}: signing_hash"
        );
        assert_ne!(tx.hash(), base.hash(), "{name}: tx hash");
    }
    // Adding optional fields to the bare second output is also covered.
    let mut tx = base.clone();
    tx.outputs[1].kem_ciphertext = Some(vec![]);
    assert_ne!(tx.signing_hash(), base.signing_hash(), "kem add (empty)");
    let mut tx = base.clone();
    tx.outputs[1].e_memo = Some(EncryptedMemo { ciphertext: vec![] });
    assert_ne!(tx.signing_hash(), base.signing_hash(), "memo add (empty)");
    let mut tx = base.clone();
    tx.outputs[1].cluster_tags = ClusterTagVector::empty_at_block(0);
    assert_ne!(tx.signing_hash(), base.signing_hash(), "decay add");
    // Output order is committed.
    let mut tx = base.clone();
    tx.outputs.swap(0, 1);
    assert_ne!(tx.signing_hash(), base.signing_hash(), "output order");
}

/// A signed, balanced transaction verifies; mutating any of `kem_ciphertext`,
/// `e_memo` or `cluster_tags` after signing makes CLSAG verification fail.
#[test]
fn mutating_kem_memo_or_tags_invalidates_signature() {
    let amount = crate::tests::TEST_AMOUNT;
    let output = TxOutput {
        amount,
        target_key: [7; 32],
        public_key: [8; 32],
        e_memo: Some(EncryptedMemo {
            ciphertext: vec![0x5a; ENCRYPTED_MEMO_SIZE],
        }),
        cluster_tags: full_tags(),
        kem_ciphertext: Some(vec![0xa5; 1088]),
    };
    let preliminary = Transaction::new_clsag(vec![], vec![output.clone()], MIN_TX_FEE, 1);
    let message = preliminary.signing_hash();
    let input = crate::tests::signed_clsag_input(amount + MIN_TX_FEE, &message);
    let tx = Transaction::new_clsag(vec![input], vec![output], MIN_TX_FEE, 1);
    assert!(
        tx.verify_ring_signatures().is_ok(),
        "unmodified tx verifies"
    );

    type Mutation = fn(&mut TxOutput);
    let mutations: &[(&str, Mutation)] = &[
        ("kem_ciphertext byte", |o| {
            o.kem_ciphertext.as_mut().unwrap()[0] ^= 1
        }),
        ("kem_ciphertext strip", |o| o.kem_ciphertext = None),
        ("e_memo byte", |o| {
            o.e_memo.as_mut().unwrap().ciphertext[0] ^= 1
        }),
        ("e_memo strip", |o| o.e_memo = None),
        ("cluster_tags weight", |o| {
            o.cluster_tags.entries[0].weight ^= 1
        }),
        ("cluster_tags clear", |o| {
            o.cluster_tags = ClusterTagVector::empty()
        }),
        ("cluster_tags decay", |o| o.cluster_tags.decay_state = None),
    ];
    for (name, mutate) in mutations {
        let mut forged = tx.clone();
        mutate(&mut forged.outputs[0]);
        assert!(
            forged.verify_ring_signatures().is_err(),
            "{name}: mutated tx must fail signature verification"
        );
    }
}
