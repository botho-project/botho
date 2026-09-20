use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::Scalar;
use bth_transaction_clsag::lottery_v2::*;
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
pub fn vectors() -> serde_json::Value {
    let source = Source {
        outpoint: Outpoint {
            hash: [3; 32],
            index: 3,
        },
        target: RistrettoPublic::from(&RistrettoPrivate::from(Scalar::from(7u64))).to_bytes(),
        context: Context {
            base_index: 3,
            tweak: [0; 32],
        },
    };
    let awards = vec![
        Award {
            winner: source.outpoint.clone(),
            amount: 900,
        },
        Award {
            winner: Outpoint {
                hash: [4; 32],
                index: 5,
            },
            amount: 901,
        },
    ];
    let d = Domain {
        genesis: [1; 32],
        parent: [2; 32],
        height: 17,
        ordinary_root: [5; 32],
        manifest: manifest(&awards).unwrap(),
        ordinal: 0,
        amount: 900,
    };
    let derived = derive(&source, &d).unwrap();
    let record = Record {
        ordinal: 0,
        winner: source.outpoint.clone(),
        amount: 900,
        target: derived.target,
        public_key: RistrettoPublic::from(&RistrettoPrivate::from(Scalar::from(11u64))).to_bytes(),
        ciphertext: Some(vec![0x42; KEM_BYTES]),
        context: derived.context.clone(),
    };
    assert_eq!(Record::decode(&record.encode().unwrap()).unwrap(), record);
    let mut classical = record.clone();
    classical.ciphertext = None;
    assert_eq!(
        Record::decode(&classical.encode().unwrap()).unwrap(),
        classical
    );
    let mut repeated_domain = d.clone();
    repeated_domain.height += 1;
    let repeated = derive(&source, &repeated_domain).unwrap();
    let repeated = serde_json::json!({"preimage":hex(&preimage(&source,&repeated_domain).unwrap()),"target":hex(&repeated.target),"tweak":hex(&repeated.context.tweak),"delta":hex(&repeated.delta),"counter":repeated.counter});
    let summary = Summary {
        fees: 1125,
        distributed: 900,
        burned: 225,
        seed: [9; 32],
    };
    let mut lineage = Vec::new();
    let mut parent = source.clone();
    for i in 0..3 {
        let mut nd = d.clone();
        nd.height += i;
        nd.parent = [20 + i as u8; 32];
        nd.manifest = manifest(&[Award {
            winner: parent.outpoint.clone(),
            amount: 900,
        }])
        .unwrap();
        let next = derive(&parent, &nd).unwrap();
        lineage.push(serde_json::json!({"source_outpoint_index":parent.outpoint.index,"base_index":next.context.base_index,"preimage":hex(&preimage(&parent,&nd).unwrap()),"target":hex(&next.target),"tweak":hex(&next.context.tweak),"delta":hex(&next.delta),"counter":next.counter}));
        parent = Source {
            outpoint: Outpoint {
                hash: [30 + i as u8; 32],
                index: 1 + i as u32,
            },
            target: next.target,
            context: next.context,
        };
    }
    let pr = payout_root(std::slice::from_ref(&record)).unwrap();
    let sr = summary_root(&summary);
    serde_json::json!({"version":1,"source_target":hex(&source.target),"manifest":hex(&d.manifest),"preimage":hex(&preimage(&source,&d).unwrap()),"counter":derived.counter,"delta":hex(&derived.delta),"target":hex(&derived.target),"context_tweak":hex(&derived.context.tweak),"record":hex(&record.encode().unwrap()),"payout_root":hex(&pr),"summary_root":hex(&sr),"body_root":hex(&body_root([6;32],d.ordinary_root,pr,sr)),"lineage":lineage,"repeated":repeated,"classical_record":hex(&classical.encode().unwrap()),"classical_payout_root":hex(&payout_root(&[classical]).unwrap())})
}
