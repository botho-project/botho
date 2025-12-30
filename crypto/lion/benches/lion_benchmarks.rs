// Copyright (c) 2024 Botho Foundation

//! Benchmarks for Lion ring signatures.
//!
//! Run with: cargo bench -p bth-crypto-lion

use criterion::{criterion_group, criterion_main, Criterion};

fn lion_benchmarks(_c: &mut Criterion) {
    // Placeholder for future benchmarks
}

criterion_group!(benches, lion_benchmarks);
criterion_main!(benches);
