//! Fixed, legitimate composition cases; invoked by the bounded RSS collector.
use super::*;

const ORDINARY: (u64, u64, u64, u32, u64, u64) =
    (6000, 3500, 1_234_567, 200, 6_307_200, 31_536_000);

#[test]
#[ignore = "measure_ownership.py runs this exact test in a fresh process per named case"]
fn ownership_resource_case() {
    let case = std::env::var("BOTHO_OWNERSHIP_CASE").expect("explicit named resource case");
    let now = Instant::now();
    let bp = BulletproofGens::new(32768, 1);
    let setup = now.elapsed().as_nanos();
    println!("RESOURCE_SETUP case={case} capacity=32768 setup_ns={setup}");
    if case == "generators" {
        // Keep allocation live until the process observes the setup operation.
        std::hint::black_box(&bp);
        return;
    }
    let (count, samples, value, base, params) = match case.as_str() {
        "ordinary-1" => (1, 3, 1_000_000_000_000, 250_000_000_000, ORDINARY),
        "ordinary-4" => (4, 3, 1_000_000_000_000, 250_000_000_000, ORDINARY),
        "ordinary-16" => (16, 3, 1_000_000_000_000, 250_000_000_000, ORDINARY),
        "zero" => (1, 1, 0, 0, (1000, 1000, 0, 0, 0, 0)),
        "both-caps-cancel" => (1, 1, M, 0, (6000, 5999, 0, u32::MAX, 1, 31_536_000)),
        "one-cap-difference" => (1, 1, 100, 0, (6000, 3500, 0, 200, 1, M - 1)),
        "maximum-sum" => (16, 1, M, 0, (1000, 1000, 0, 0, 1, 0)),
        _ => panic!("unsupported named resource case"),
    };
    let values: Vec<_> = (0..count)
        .map(|i| {
            if case.starts_with("ordinary-") {
                value + 1000 * i as u64
            } else {
                value
            }
        })
        .collect();
    let fixture_start = Instant::now();
    let (public, secret) = fixture_parameters(&values, 2, base, params);
    // Assert the actual charge/cap premises; a case label alone is not evidence.
    for w in &secret.arithmetic.inputs {
        let (fi, fo, elapsed, rate, year, horizon) = params;
        let reference = Candidate::construct(w.amount, fi, fo, elapsed, rate, year, horizon);
        reference.check(w.amount).unwrap();
        match case.as_str() {
            "both-caps-cancel" => {
                assert_eq!(reference.result("capital.in.c"), M);
                assert_eq!(reference.result("capital.out.c"), M);
                assert_eq!(reference.result("difference"), 0);
                assert_eq!(w.charge, 0);
            }
            "one-cap-difference" => {
                assert_eq!(reference.result("capital.in.c"), M);
                assert_eq!(reference.result("capital.out.c"), M - 1);
                assert_eq!(reference.result("difference"), 1);
                assert_eq!(w.charge, 1);
            }
            "zero" => {
                assert_eq!(w.amount, 0);
                assert_eq!(w.charge, 0);
            }
            "maximum-sum" => {
                assert_eq!(w.amount, M);
                assert_eq!(w.charge, 0);
            }
            _ => assert!(w.charge > 0),
        }
    }
    println!(
        "RESOURCE_FIXTURE case={case} fixture_ns={}",
        fixture_start.elapsed().as_nanos()
    );
    for sample in 0..samples {
        let now = Instant::now();
        let (proof, p) = prove_measured(&public, &secret, &bp).unwrap();
        let prove_ns = now.elapsed().as_nanos();
        let now = Instant::now();
        let v = verify_measured(&public, &proof, &bp).unwrap();
        let verify_ns = now.elapsed().as_nanos();
        assert_eq!(
            (p.multipliers, p.constraints),
            (v.multipliers, v.constraints)
        );
        assert_eq!(p.multipliers, 1476 * count + 136);
        assert_eq!(proof.signatures.len(), count);
        assert!(proof
            .signatures
            .iter()
            .all(|s| s.responses.len() == RING_SIZE));
        let fields: usize = proof
            .signatures
            .iter()
            .map(|s| 32 * (s.responses.len() + 3))
            .sum();
        // This is an honest arithmetic allocation comparison, not byte admission.
        assert!(p.multipliers <= 65536 && p.constraints <= 262144);
        println!("RESOURCE_SAMPLE case={case} sample={sample} inputs={count} outputs={count} ring=20 significant_bits=2 multipliers={} constraints={} arithmetic_bytes={} clsag_field_bytes={fields} signatures={} prove_ns={prove_ns} verify_ns={verify_ns}",
            p.multipliers, p.constraints, proof.arithmetic_proof.len(), proof.signatures.len());
    }
}
