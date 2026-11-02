//! Timed-loop throughput comparison (issue #1715).
//!
//!   cargo run --release --example bench [-- <seconds-per-case>]
//!
//! Reports ops/s and µs/op for the operations that matter to Botho:
//! ML-KEM decapsulation (wallet scanning: one decaps per output per wallet),
//! ML-DSA verify (block/tx validation), plus keygen/encaps/sign for context.
//! Each case runs a warm-up then as many iterations as fit in the budget.

use bth_crypto_pq::{MlDsa65KeyPair, MlDsa65Signature, MlKem768Ciphertext, MlKem768KeyPair};
use libcrux_eval::*;
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

fn run(name: &str, budget: Duration, mut f: impl FnMut()) -> f64 {
    for _ in 0..200 {
        f();
    }
    let start = Instant::now();
    let mut n: u64 = 0;
    while start.elapsed() < budget {
        for _ in 0..100 {
            f();
        }
        n += 100;
    }
    let secs = start.elapsed().as_secs_f64();
    let ops = n as f64 / secs;
    println!("{name:<48} {ops:>12.0} ops/s {:>9.2} µs/op", 1e6 / ops);
    ops
}

fn main() {
    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2.0);
    let budget = Duration::from_secs_f64(secs);
    println!(
        "arch={} os={} libcrux auto backend={} budget={secs}s/case",
        std::env::consts::ARCH,
        std::env::consts::OS,
        Backend::auto_resolves_to()
    );

    // ---------------- ML-KEM-768
    let kp = MlKem768KeyPair::from_seed(&[3u8; 32]);
    let (ct, _) = kp.public_key().encapsulate();
    let sk = *kp.secret_key().as_bytes();
    let pk = *kp.public_key().as_bytes();
    let ctb = *ct.as_bytes();
    let bct = MlKem768Ciphertext::from_bytes(&ctb).unwrap();

    println!("\n# ML-KEM-768 decapsulate (wallet scanning)");
    let base = run(
        "RustCrypto ml-kem 0.2.3 (via bth-crypto-pq)",
        budget,
        || {
            black_box(kp.decapsulate(black_box(&bct)).unwrap());
        },
    );
    run("RustCrypto ml-kem 0.2.3 (raw)", budget, || {
        black_box(rc_kem::decaps(black_box(&sk), black_box(&ctb)));
    });
    for b in Backend::all() {
        let ops = run(&format!("libcrux 0.0.11 {b:?}"), budget, || {
            black_box(lc_kem::decaps(b, black_box(&sk), black_box(&ctb)));
        });
        println!("{:<48} {:>12.2}x vs bth-crypto-pq", "", ops / base);
    }

    println!("\n# ML-KEM-768 encapsulate (deterministic m)");
    run("RustCrypto ml-kem 0.2.3", budget, || {
        black_box(rc_kem::encaps(black_box(&pk), [5u8; 32]));
    });
    for b in Backend::all() {
        run(&format!("libcrux 0.0.11 {b:?}"), budget, || {
            black_box(lc_kem::encaps(b, black_box(&pk), [5u8; 32]));
        });
    }

    println!("\n# ML-KEM-768 keygen");
    run("RustCrypto ml-kem 0.2.3", budget, || {
        black_box(rc_kem::keygen(black_box([7u8; 64])));
    });
    for b in Backend::all() {
        run(&format!("libcrux 0.0.11 {b:?}"), budget, || {
            black_box(lc_kem::keygen(b, black_box([7u8; 64])));
        });
    }

    // ---------------- ML-DSA-65
    let dkp = MlDsa65KeyPair::from_seed(&[4u8; 32]);
    let msg = [0x42u8; 256];
    let sig = dkp.sign(&msg);
    let dpk = *dkp.public_key().as_bytes();
    let dsk = *dkp.secret_key().as_bytes();
    let sigb = *sig.as_bytes();
    let bsig = MlDsa65Signature::from_bytes(&sigb).unwrap();

    println!("\n# ML-DSA-65 verify (256-byte message)");
    let base = run(
        "RustCrypto ml-dsa 0.1.1 (via bth-crypto-pq)",
        budget,
        || {
            black_box(
                dkp.public_key()
                    .verify(black_box(&msg), black_box(&bsig))
                    .is_ok(),
            );
        },
    );
    for b in Backend::all() {
        let ops = run(&format!("libcrux 0.0.11 {b:?}"), budget, || {
            black_box(lc_dsa::verify(
                b,
                black_box(&dpk),
                black_box(&msg),
                b"",
                black_box(&sigb),
            ));
        });
        println!("{:<48} {:>12.2}x vs bth-crypto-pq", "", ops / base);
    }

    println!("\n# ML-DSA-65 sign (deterministic, 256-byte message)");
    run(
        "RustCrypto ml-dsa 0.1.1 (via bth-crypto-pq)",
        budget,
        || {
            black_box(dkp.sign(black_box(&msg)));
        },
    );
    for b in Backend::all() {
        run(&format!("libcrux 0.0.11 {b:?}"), budget, || {
            black_box(lc_dsa::sign(
                b,
                black_box(&dsk),
                black_box(&msg),
                b"",
                [0u8; 32],
            ));
        });
    }

    println!("\n# ML-DSA-65 keygen from seed");
    run("RustCrypto ml-dsa 0.1.1", budget, || {
        black_box(rc_dsa::keygen(black_box([9u8; 32])));
    });
    for b in Backend::all() {
        run(&format!("libcrux 0.0.11 {b:?}"), budget, || {
            black_box(lc_dsa::keygen(b, black_box([9u8; 32])));
        });
    }
}
