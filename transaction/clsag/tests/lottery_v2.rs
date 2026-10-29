#[path = "support/lottery_vectors.rs"]
mod vectors;
use bth_transaction_clsag::lottery_v2::*;
#[test]
fn committed_native_vectors() {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/lottery-v2.json")).unwrap();
    assert_eq!(vectors::vectors(), expected);
}
/// Fixed byte vectors for the three contexts the payout primitive refuses
/// (#1667): the zero tweak, a non-canonical tweak (T >= l little-endian) and an
/// unknown context version. Each fails with its own error, before the wallet
/// callback runs, in both the context codec and `recover_with_context`.
#[test]
fn rejected_context_vectors() {
    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
    // Target and public key of the committed classical record: valid points.
    let output = bth_transaction_clsag::TxOutput {
        amount: 1,
        target_key: unhex("7e533e092d9dbca8090dfbe34949ece2062d00cd35a02c7eec9a8ddbddc33f50")
            .try_into()
            .unwrap(),
        public_key: unhex("bce83f8ba5dd2fa572864c24ba1810f9522bc6004afe95877ac73241cafdab42")
            .try_into()
            .unwrap(),
        kem_ciphertext: None,
        e_memo: None,
        cluster_tags: Default::default(),
    };
    let never = |_: &bth_transaction_clsag::TxOutput, _| -> Option<_> {
        panic!("wallet callback reached for a rejected context")
    };
    // version || base_index (LE u32 = 3) || tweak (LE scalar)
    let cases = [
        (
            "zero tweak",
            "0203000000",
            "0000000000000000000000000000000000000000000000000000000000000000",
            Ok(()),
            Error::ZeroTweak,
        ),
        (
            "tweak = l",
            "0203000000",
            "edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010",
            Err(Error::NonCanonicalTweak),
            Error::NonCanonicalTweak,
        ),
        (
            "tweak = l + 1",
            "0203000000",
            "eed3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010",
            Err(Error::NonCanonicalTweak),
            Error::NonCanonicalTweak,
        ),
        (
            "tweak = 2^256 - 1",
            "0203000000",
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            Err(Error::NonCanonicalTweak),
            Error::NonCanonicalTweak,
        ),
    ];
    for (name, head, tweak, codec, recover) in cases {
        let bytes = unhex(&format!("{head}{tweak}"));
        let context = Context {
            base_index: 3,
            tweak: unhex(tweak).try_into().unwrap(),
        };
        assert_eq!(
            Context::decode(&bytes).map(|c| assert_eq!(c, context)),
            codec,
            "{name}: decode"
        );
        assert_eq!(
            recover_with_context(&output, &context, never).map(|_| ()),
            Err(recover),
            "{name}: recover"
        );
    }
    // A canonical nonzero tweak (scalar 1) under every version byte other than
    // CONTEXT_VERSION.
    let one = "0100000000000000000000000000000000000000000000000000000000000000";
    for version in (0..=u8::MAX).filter(|v| *v != CONTEXT_VERSION) {
        let bytes = unhex(&format!("{version:02x}03000000{one}"));
        assert_eq!(
            Context::decode(&bytes),
            Err(Error::Version),
            "version {version}"
        );
    }
    let good = unhex(&format!("0203000000{one}"));
    assert_eq!(
        Context::decode(&good).unwrap().encode().unwrap(),
        good,
        "the same bytes under CONTEXT_VERSION are accepted"
    );
    // The version check also applies to a context inside a payout record.
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/lottery-v2.json")).unwrap();
    let record = unhex(fixture["classical_record"].as_str().unwrap());
    assert!(Record::decode(&record).is_ok());
    let at = record.len() - (1 + 4 + 32);
    assert_eq!(record[at], CONTEXT_VERSION);
    for version in [0, 1, 3, 0xff] {
        let mut bad = record.clone();
        bad[at] = version;
        assert_eq!(
            Record::decode(&bad),
            Err(Error::Version),
            "record version {version}"
        );
    }
}
#[cfg(feature = "pq")]
#[test]
fn actual_clsag_source_repeated_and_nested_spends() {
    use bth_account_keys::QuantumSafeAccountKey;
    use bth_crypto_keys::RistrettoPublic;
    use bth_transaction_clsag::{ClsagRingInput, RingMember, Transaction, TxOutput, MIN_RING_SIZE};
    use bth_transaction_types::ClusterTagVector;
    use rand::{rngs::StdRng, SeedableRng};
    let account=QuantumSafeAccountKey::from_mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about");
    for hybrid in [false, true] {
        let original = if hybrid {
            TxOutput::new_hybrid_to_address(
                100_000,
                &account.default_subaddress(),
                3,
                None,
                ClusterTagVector::empty(),
            )
            .unwrap()
        } else {
            TxOutput::new(100_000, &account.default_subaddress())
        };
        let original_source = Source {
            outpoint: Outpoint {
                hash: [7; 32],
                index: 3,
            },
            target: original.target_key,
            context: Context {
                base_index: 3,
                tweak: [0; 32],
            },
        };
        let mut outputs = vec![original.clone()];
        let mut sources = vec![original_source.clone()];
        for (n, parent) in [0usize, 0, 1, 3].into_iter().enumerate() {
            let source = &sources[parent];
            let source_output = &outputs[parent];
            let amount = 900 - n as u64;
            let d = Domain {
                genesis: [1; 32],
                parent: [n as u8 + 2; 32],
                height: n as u64 + 10,
                ordinary_root: [3; 32],
                manifest: manifest(&[Award {
                    winner: source.outpoint.clone(),
                    amount,
                }])
                .unwrap(),
                ordinal: 0,
                amount,
            };
            let derived = derive(source, &d).unwrap();
            let record = Record {
                ordinal: 0,
                winner: source.outpoint.clone(),
                amount,
                target: derived.target,
                public_key: source_output.public_key,
                ciphertext: source_output.kem_ciphertext.clone(),
                context: derived.context.clone(),
            };
            record.validate(source, source_output, &d).unwrap();
            let mut bad = record.clone();
            bad.winner.index += 1;
            assert!(bad.validate(source, source_output, &d).is_err());
            let mut bad = record.clone();
            bad.context.base_index += 1;
            assert!(bad.validate(source, source_output, &d).is_err());
            let mut bad = record.clone();
            bad.amount += 1;
            assert!(bad.validate(source, source_output, &d).is_err());
            let mut output = source_output.clone();
            output.amount = amount;
            output.target_key = derived.target;
            outputs.push(output);
            sources.push(Source {
                outpoint: Outpoint {
                    hash: [n as u8 + 20; 32],
                    index: 1,
                },
                target: derived.target,
                context: derived.context,
            });
        }
        let mut ring: Vec<_> = outputs.iter().map(RingMember::from_output).collect();
        while ring.len() < MIN_RING_SIZE {
            ring.push(RingMember::from_output(&TxOutput::new(
                500,
                &account.default_subaddress(),
            )));
        }
        let mut spent = std::collections::HashSet::new();
        let mut rng = StdRng::seed_from_u64(1293);
        for (i, (output, source)) in outputs.iter().zip(&sources).enumerate() {
            assert_eq!(source.context.base_index, 3);
            let recover = |base: &TxOutput, index| {
                let sub =
                    base.belongs_to_account(account.classical(), account.pq_kem_keypair(), index)?;
                base.recover_spend_key_for(
                    account.classical(),
                    account.pq_kem_keypair(),
                    sub,
                    index,
                )
            };
            // The original output is a direct lineage start (zero tweak); it
            // recovers through recover_direct, and the payout primitive
            // refuses its zero context (#1667).
            let key = if source.context.tweak == [0; 32] {
                assert_eq!(
                    recover_with_context(output, &source.context, recover).map(|_| ()),
                    Err(Error::ZeroTweak)
                );
                recover_direct(output, source.context.base_index, recover).unwrap()
            } else {
                recover_with_context(output, &source.context, recover).unwrap()
            };
            assert_eq!(RistrettoPublic::from(&key).to_bytes(), output.target_key);
            if hybrid {
                let mut bad = source.context.clone();
                bad.base_index = 0;
                assert!(recover_with_context(output, &bad, recover).is_err());
            }
            let fee = 10;
            let tx_outputs = vec![TxOutput::new(
                output.amount - fee,
                &account.default_subaddress(),
            )];
            let preliminary = Transaction::new_clsag(vec![], tx_outputs.clone(), fee, 50);
            let wrong_key =
                bth_crypto_keys::RistrettoPrivate::from(bth_crypto_ring_signature::Scalar::ONE);
            if let Ok(forged) = ClsagRingInput::new(
                ring.clone(),
                i,
                &wrong_key,
                output.amount,
                &preliminary.signing_hash(),
                &mut rng,
            ) {
                assert!(!forged.verify(&preliminary.signing_hash()));
            }
            let input = ClsagRingInput::new(
                ring.clone(),
                i,
                &key,
                output.amount,
                &preliminary.signing_hash(),
                &mut rng,
            )
            .unwrap();
            let tx = Transaction::new_clsag(vec![input.clone()], tx_outputs, fee, 50);
            tx.verify_ring_signatures().unwrap();
            assert!(
                spent.insert(input.key_image),
                "source/repeated/nested outputs must spend independently"
            );
            assert!(
                !spent.insert(input.key_image),
                "replay must hit the spent-image set"
            );
            let mut changed = input.clone();
            changed.pseudo_output_amount += 1;
            assert!(!changed.verify(&preliminary.signing_hash()));
            let mut changed = input.clone();
            changed.ring[i].target_key = ring[(i + 1) % ring.len()].target_key;
            assert!(!changed.verify(&preliminary.signing_hash()));
            let mut changed = tx;
            changed.outputs[0].amount += 1;
            assert!(changed.verify_ring_signatures().is_err());
            let mut bad = source.context.clone();
            bad.tweak = [0xff; 32];
            assert!(recover_with_context(output, &bad, recover).is_err());
        }
        assert_eq!(spent.len(), 5);
    }
}
