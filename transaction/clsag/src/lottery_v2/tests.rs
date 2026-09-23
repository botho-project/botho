use super::*;
fn public(n: u64) -> Hash {
    RistrettoPublic::from(&RistrettoPrivate::from(Scalar::from(n))).to_bytes()
}
fn source() -> Source {
    Source {
        outpoint: Outpoint {
            hash: [3; 32],
            index: 7,
        },
        target: public(7),
        context: Context {
            base_index: 7,
            tweak: [0; 32],
        },
    }
}
fn domain() -> Domain {
    Domain {
        genesis: [1; 32],
        parent: [2; 32],
        height: 11,
        ordinary_root: [4; 32],
        manifest: [5; 32],
        ordinal: 0,
        amount: 900,
    }
}
fn record() -> Record {
    let s = source();
    let d = domain();
    let p = derive(&s, &d).unwrap();
    Record {
        ordinal: 0,
        winner: s.outpoint,
        amount: d.amount,
        target: p.target,
        public_key: public(11),
        ciphertext: Some(vec![42; KEM_BYTES]),
        context: p.context,
    }
}
fn wide(s: Scalar) -> [u8; 64] {
    let mut b = [0; 64];
    b[..32].copy_from_slice(&s.to_bytes());
    b
}
#[test]
fn first_valid_retry_is_deterministic_and_exhaustion_fails() {
    let s = source();
    let d = domain();
    let p = derive_inner(&s, &d, |b| {
        if *b.last().unwrap() == 0 {
            [0; 64]
        } else {
            wide(Scalar::ONE)
        }
    })
    .unwrap();
    assert_eq!(p.counter, 1);
    assert_eq!(p.target, public(8));
    // identity target at counter 0: 7G + (-7)G = 0.
    let p = derive_inner(&s, &d, |b| {
        if *b.last().unwrap() == 0 {
            wide(-Scalar::from(7u64))
        } else {
            wide(Scalar::ONE)
        }
    })
    .unwrap();
    assert_eq!(p.counter, 1);
    let mut s = s;
    s.context.tweak = Scalar::from(3u64).to_bytes();
    let p = derive_inner(&s, &d, |b| {
        if *b.last().unwrap() == 0 {
            wide(-Scalar::from(3u64))
        } else {
            wide(Scalar::ONE)
        }
    })
    .unwrap();
    assert_eq!(p.counter, 1);
    assert_eq!(p.context.tweak, Scalar::from(4u64).to_bytes());
    assert_eq!(derive_inner(&s, &d, |_| [0; 64]), Err(Error::Exhausted));
    // Crossing the scalar order is modular addition, not u256 overflow.
    s.context.tweak = (-Scalar::ONE).to_bytes();
    assert_eq!(
        derive_inner(&s, &d, |_| wide(Scalar::from(2u64)))
            .unwrap()
            .context
            .tweak,
        Scalar::ONE.to_bytes()
    );
}
#[test]
fn every_domain_component_changes_derivation() {
    let s = source();
    let d = domain();
    let original = derive(&s, &d).unwrap();
    let mut variants = Vec::new();
    let mut v = d.clone();
    v.genesis[0] ^= 1;
    variants.push(v);
    let mut v = d.clone();
    v.parent[0] ^= 1;
    variants.push(v);
    let mut v = d.clone();
    v.height += 1;
    variants.push(v);
    let mut v = d.clone();
    v.ordinary_root[0] ^= 1;
    variants.push(v);
    let mut v = d.clone();
    v.manifest[0] ^= 1;
    variants.push(v);
    let mut v = d.clone();
    v.ordinal += 1;
    variants.push(v);
    let mut v = d.clone();
    v.amount += 1;
    variants.push(v);
    for v in variants {
        assert_ne!(derive(&s, &v).unwrap().target, original.target);
    }
    let mut variants = Vec::new();
    let mut v = s.clone();
    v.outpoint.hash[0] ^= 1;
    variants.push(v);
    let mut v = s.clone();
    v.outpoint.index += 1;
    variants.push(v);
    let mut v = s.clone();
    v.target = public(8);
    variants.push(v);
    let mut v = s.clone();
    v.context.base_index += 1;
    variants.push(v);
    let mut v = s.clone();
    v.context.tweak = Scalar::ONE.to_bytes();
    variants.push(v);
    for v in variants {
        assert_ne!(derive(&v, &d).unwrap().target, original.target);
    }
    let mut malformed = s.clone();
    malformed.target = [255; 32];
    assert_eq!(derive(&malformed, &d), Err(Error::Point));
    malformed.target = [0; 32];
    assert_eq!(derive(&malformed, &d), Err(Error::Point));
    malformed = s;
    malformed.context.tweak = [255; 32];
    assert_eq!(derive(&malformed, &d), Err(Error::Scalar));
}
#[test]
fn canonical_codec_rejects_malformed_trailing_and_unknown_fields() {
    let mut order = (-Scalar::ONE).to_bytes();
    order[0] += 1;
    assert_eq!(scalar(order), Err(Error::Scalar));
    let r = record();
    let bytes = r.encode().unwrap();
    assert_eq!(Record::decode(&bytes).unwrap(), r);
    for n in 0..bytes.len() {
        assert!(Record::decode(&bytes[..n]).is_err());
    }
    let mut b = bytes.clone();
    b.push(0);
    assert_eq!(Record::decode(&b), Err(Error::Encoding));
    let mut b = bytes.clone();
    b[112] = 2;
    assert_eq!(Record::decode(&b), Err(Error::Encoding)); // KEM tag
    let mut b = bytes.clone();
    b[113 + KEM_BYTES] = 3;
    assert_eq!(Record::decode(&b), Err(Error::Encoding)); // context tag
    let mut r = r.clone();
    r.ciphertext = Some(vec![0; KEM_BYTES - 1]);
    assert_eq!(r.encode(), Err(Error::Encoding));
    r.ciphertext = None;
    r.context.tweak = [255; 32];
    assert_eq!(r.encode(), Err(Error::Scalar));
    r.context.tweak = [0; 32];
    assert_eq!(r.encode(), Err(Error::Scalar));
    r = record();
    r.target = [255; 32];
    assert_eq!(r.encode(), Err(Error::Point));
    r = record();
    r.public_key = [0; 32];
    assert_eq!(r.encode(), Err(Error::Point));
    r = record();
    r.ordinal = 4;
    assert_eq!(r.encode(), Err(Error::Count));
    assert_eq!(payout_root(&vec![record(); 5]), Err(Error::Count));
}

#[test]
fn context_codec_is_versioned_canonical_and_syntax_only() {
    let context = Context {
        base_index: 19,
        tweak: [0; 32],
    };
    let bytes = context.encode().unwrap();
    assert_eq!(bytes.len(), 1 + 4 + 32);
    assert_eq!(bytes[0], CONTEXT_VERSION);
    assert_eq!(Context::decode(&bytes).unwrap(), context);

    for n in 0..bytes.len() {
        assert!(Context::decode(&bytes[..n]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(Context::decode(&trailing), Err(Error::Encoding));

    let mut unknown = bytes.clone();
    unknown[0] = CONTEXT_VERSION + 1;
    assert_eq!(Context::decode(&unknown), Err(Error::Encoding));

    let mut malformed = bytes;
    malformed[5] = 0xff;
    malformed[6] = 0xff;
    malformed[7] = 0xff;
    malformed[8] = 0xff;
    malformed[9..].fill(0xff);
    assert_eq!(Context::decode(&malformed), Err(Error::Scalar));
}
#[test]
fn ordered_records_bind_every_field_and_body_component() {
    let r = record();
    let root = payout_root(std::slice::from_ref(&r)).unwrap();
    let mut variants = Vec::new();
    let mut v = r.clone();
    v.winner.hash[0] ^= 1;
    variants.push(v);
    let mut v = r.clone();
    v.winner.index += 1;
    variants.push(v);
    let mut v = r.clone();
    v.amount += 1;
    variants.push(v);
    let mut v = r.clone();
    v.target = public(15);
    variants.push(v);
    let mut v = r.clone();
    v.public_key = public(16);
    variants.push(v);
    let mut v = r.clone();
    v.ciphertext.as_mut().unwrap()[0] ^= 1;
    variants.push(v);
    let mut v = r.clone();
    v.ciphertext = None;
    variants.push(v);
    let mut v = r.clone();
    v.context.base_index += 1;
    variants.push(v);
    let mut v = r.clone();
    v.context.tweak = Scalar::from(9u64).to_bytes();
    variants.push(v);
    for v in variants {
        assert_ne!(payout_root(&[v]).unwrap(), root);
    }
    let mut next = r.clone();
    next.ordinal = 1;
    next.amount += 1;
    assert!(payout_root(&[next.clone(), r.clone()]).is_err());
    let first = payout_root(&[r.clone(), next.clone()]).unwrap();
    next.ordinal = 0;
    let mut second = r;
    second.ordinal = 1;
    assert_ne!(payout_root(&[next, second]).unwrap(), first);
    let awards = [
        Award {
            winner: source().outpoint,
            amount: 1,
        },
        Award {
            winner: Outpoint {
                hash: [1; 32],
                index: 2,
            },
            amount: 2,
        },
    ];
    assert_ne!(
        manifest(&awards).unwrap(),
        manifest(&[awards[1].clone(), awards[0].clone()]).unwrap()
    );
    let s = Summary {
        fees: 3,
        distributed: 2,
        burned: 1,
        seed: [1; 32],
    };
    let sr = summary_root(&s);
    for i in 0..4 {
        let mut v = s.clone();
        match i {
            0 => v.fees += 1,
            1 => v.distributed += 1,
            2 => v.burned += 1,
            _ => v.seed[0] ^= 1,
        };
        assert_ne!(summary_root(&v), sr);
    }
    let body = body_root([1; 32], [2; 32], [3; 32], [4; 32]);
    for i in 0..4 {
        let mut h = [[1; 32], [2; 32], [3; 32], [4; 32]];
        h[i][0] ^= 1;
        assert_ne!(body_root(h[0], h[1], h[2], h[3]), body);
    }
}
#[test]
fn recovery_never_returns_wrong_key_even_if_callback_lies() {
    let r = record();
    let out = TxOutput {
        amount: r.amount,
        target_key: r.target,
        public_key: r.public_key,
        kem_ciphertext: r.ciphertext,
        e_memo: None,
        cluster_tags: Default::default(),
    };
    let key = recover_with_context(&out, &r.context, |_, i| {
        assert_eq!(i, 7);
        Some(RistrettoPrivate::from(Scalar::from(7u64)))
    })
    .unwrap();
    assert_eq!(RistrettoPublic::from(&key).to_bytes(), out.target_key);
    assert!(matches!(
        recover_with_context(&out, &r.context, |_, _| Some(RistrettoPrivate::from(
            Scalar::ONE
        ))),
        Err(Error::Ownership)
    ));
    let mut bad = r.context;
    bad.base_index = 0;
    assert!(matches!(
        recover_with_context(&out, &bad, |_, i| if i == 7 {
            Some(RistrettoPrivate::from(Scalar::from(7u64)))
        } else {
            None
        }),
        Err(Error::Ownership)
    ));
}
