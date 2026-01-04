// Copyright (c) 2024 Botho Foundation

//! Benchmarks for mempool operations.
//!
//! Run with: cargo bench -p botho --bench mempool_benchmarks
//!
//! These benchmarks measure the performance of:
//! - Mempool operations (get_transactions, evict_old)
//! - PendingTx creation
//! - Fee calculation and estimation
//!
//! Note: add_tx benchmarks require a mock ledger which adds significant
//! complexity. These benchmarks focus on the isolated mempool operations that
//! don't require ledger access.

use botho::{
    mempool::{Mempool, PendingTx},
    transaction::{ClsagRingInput, RingMember, Transaction, TxOutput, MIN_RING_SIZE, MIN_TX_FEE},
};
use bth_cluster_tax::{FeeConfig, TransactionType};
use bth_transaction_types::ClusterTagVector;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

/// Helper to create a test output with raw bytes
fn test_output(amount: u64, id: u8) -> TxOutput {
    TxOutput {
        amount,
        target_key: [id; 32],
        public_key: [id.wrapping_add(1); 32],
        e_memo: None,
        cluster_tags: ClusterTagVector::empty(),
    }
}

/// Helper to create a minimal test ring member
fn test_ring_member(id: u8) -> RingMember {
    RingMember {
        target_key: [id; 32],
        public_key: [id.wrapping_add(1); 32],
        commitment: [id.wrapping_add(2); 32],
    }
}

/// Helper to create a test CLSAG input with MIN_RING_SIZE members
fn test_clsag_input(ring_id: u8) -> ClsagRingInput {
    let ring: Vec<RingMember> = (0..MIN_RING_SIZE)
        .map(|i| test_ring_member(ring_id.wrapping_add(i as u8)))
        .collect();
    ClsagRingInput {
        ring,
        key_image: [ring_id; 32],
        commitment_key_image: [ring_id.wrapping_add(100); 32],
        clsag_signature: vec![0u8; 32 + 32 * MIN_RING_SIZE], // Fake signature
    }
}

/// Create a test transaction with given fee and height
fn test_tx(fee: u64, height: u64) -> Transaction {
    Transaction::new_clsag(
        vec![test_clsag_input(height as u8)],
        vec![test_output(1000, height as u8)],
        fee.max(MIN_TX_FEE),
        height,
    )
}

/// Create a vector of test transactions for benchmarking
fn create_test_transactions(count: usize) -> Vec<Transaction> {
    (0..count)
        .map(|i| test_tx(MIN_TX_FEE + (i as u64 * 100), i as u64))
        .collect()
}

/// Benchmark PendingTx creation
fn bench_pending_tx_new(c: &mut Criterion) {
    let tx = test_tx(MIN_TX_FEE * 5, 0);

    c.bench_function("PendingTx new", |b| {
        b.iter(|| black_box(PendingTx::new(tx.clone())))
    });
}

/// Benchmark Mempool creation
fn bench_mempool_new(c: &mut Criterion) {
    c.bench_function("Mempool new", |b| b.iter(|| black_box(Mempool::new())));
}

/// Benchmark Mempool with_fee_config creation
fn bench_mempool_with_fee_config(c: &mut Criterion) {
    let config = FeeConfig::default();

    c.bench_function("Mempool with_fee_config", |b| {
        b.iter(|| black_box(Mempool::with_fee_config(config.clone())))
    });
}

/// Benchmark get_transactions sorting and retrieval
fn bench_get_transactions(c: &mut Criterion) {
    let mut group = c.benchmark_group("Mempool get_transactions");

    // Note: We can't easily populate a mempool without a ledger, but we can
    // benchmark an empty mempool which exercises the sorting logic with no data.
    // The main cost of get_transactions is sorting, which we can measure.
    let mempool = Mempool::new();

    group.bench_function("empty mempool", |b| {
        b.iter(|| black_box(mempool.get_transactions(100)))
    });

    group.finish();
}

/// Benchmark evict_old (which iterates through all transactions)
fn bench_evict_old(c: &mut Criterion) {
    // Without ledger access, we can only benchmark with empty mempool
    let mut group = c.benchmark_group("Mempool evict_old");

    group.bench_function("empty mempool", |b| {
        b.iter_batched(
            || Mempool::new(),
            |mut mempool| {
                mempool.evict_old();
                black_box(mempool)
            },
            criterion::BatchSize::SmallInput,
        )
    });

    group.finish();
}

/// Benchmark fee estimation for different transaction types
fn bench_estimate_fee(c: &mut Criterion) {
    let mempool = Mempool::new();

    let mut group = c.benchmark_group("Mempool estimate_fee");

    group.bench_function("Hidden (CLSAG)", |b| {
        b.iter(|| black_box(mempool.estimate_fee(TransactionType::Hidden, 1_000_000_000_000, 0)))
    });

    group.bench_function("with memos", |b| {
        b.iter(|| black_box(mempool.estimate_fee(TransactionType::Hidden, 1_000_000_000_000, 2)))
    });

    group.bench_function("Minting", |b| {
        b.iter(|| black_box(mempool.estimate_fee(TransactionType::Minting, 1_000_000_000_000, 0)))
    });

    group.finish();
}

/// Benchmark fee estimation with cluster wealth
fn bench_estimate_fee_with_wealth(c: &mut Criterion) {
    let mempool = Mempool::new();

    let mut group = c.benchmark_group("Mempool estimate_fee_with_wealth");

    for wealth in [0u64, 1_000_000_000_000, 10_000_000_000_000_000].iter() {
        group.bench_with_input(BenchmarkId::new("wealth", wealth), wealth, |b, &wealth| {
            b.iter(|| {
                black_box(mempool.estimate_fee_with_wealth(
                    TransactionType::Hidden,
                    1_000_000_000_000,
                    1,
                    wealth,
                ))
            })
        });
    }

    group.finish();
}

/// Benchmark cluster factor calculation
fn bench_cluster_factor(c: &mut Criterion) {
    let mempool = Mempool::new();

    let mut group = c.benchmark_group("Mempool cluster_factor");

    for wealth in [0u64, 1_000_000_000_000, 10_000_000_000_000_000].iter() {
        group.bench_with_input(BenchmarkId::new("wealth", wealth), wealth, |b, &wealth| {
            b.iter(|| black_box(mempool.cluster_factor(wealth)))
        });
    }

    group.finish();
}

/// Benchmark suggest_fees
fn bench_suggest_fees(c: &mut Criterion) {
    let mempool = Mempool::new();

    let mut group = c.benchmark_group("Mempool suggest_fees");

    for tx_size in [4000usize, 65000usize].iter() {
        group.bench_with_input(
            BenchmarkId::new("tx_size", tx_size),
            tx_size,
            |b, &tx_size| b.iter(|| black_box(mempool.suggest_fees(tx_size, 1_000_000_000_000))),
        );
    }

    group.finish();
}

/// Benchmark dynamic fee state retrieval
fn bench_dynamic_fee_state(c: &mut Criterion) {
    let mempool = Mempool::new();

    c.bench_function("Mempool dynamic_fee_state", |b| {
        b.iter(|| black_box(mempool.dynamic_fee_state()))
    });
}

/// Benchmark current_fee_base computation
fn bench_current_fee_base(c: &mut Criterion) {
    let mempool = Mempool::new();

    c.bench_function("Mempool current_fee_base", |b| {
        b.iter(|| black_box(mempool.current_fee_base()))
    });
}

/// Benchmark contains lookup with empty mempool
fn bench_contains(c: &mut Criterion) {
    let mempool = Mempool::new();
    let missing_hash = [0xFFu8; 32];

    c.bench_function("Mempool contains (missing)", |b| {
        b.iter(|| black_box(mempool.contains(&missing_hash)))
    });
}

/// Benchmark total_fees computation
fn bench_total_fees(c: &mut Criterion) {
    let mempool = Mempool::new();

    c.bench_function("Mempool total_fees (empty)", |b| {
        b.iter(|| black_box(mempool.total_fees()))
    });
}

/// Benchmark len() calls
fn bench_len(c: &mut Criterion) {
    let mempool = Mempool::new();

    c.bench_function("Mempool len", |b| b.iter(|| black_box(mempool.len())));
}

/// Benchmark is_empty() calls
fn bench_is_empty(c: &mut Criterion) {
    let mempool = Mempool::new();

    c.bench_function("Mempool is_empty", |b| {
        b.iter(|| black_box(mempool.is_empty()))
    });
}

/// Benchmark static fee estimation (bypasses dynamic congestion)
fn bench_estimate_fee_static(c: &mut Criterion) {
    let mempool = Mempool::new();

    let mut group = c.benchmark_group("Mempool estimate_fee_static");

    group.bench_function("Hidden", |b| {
        b.iter(|| black_box(mempool.estimate_fee_static(TransactionType::Hidden, 1)))
    });

    group.bench_function("Minting", |b| {
        b.iter(|| black_box(mempool.estimate_fee_static(TransactionType::Minting, 0)))
    });

    group.finish();
}

/// Benchmark transaction hash computation (used internally by add_tx)
fn bench_transaction_hash(c: &mut Criterion) {
    let transactions = create_test_transactions(5);

    let mut group = c.benchmark_group("Transaction hash");

    for (i, tx) in transactions.iter().enumerate() {
        group.bench_with_input(BenchmarkId::new("tx", i), tx, |b, tx| {
            b.iter(|| black_box(tx.hash()))
        });
    }

    group.finish();
}

/// Benchmark transaction estimate_size (used for fee calculations)
fn bench_transaction_estimate_size(c: &mut Criterion) {
    let tx = test_tx(MIN_TX_FEE, 0);

    c.bench_function("Transaction estimate_size", |b| {
        b.iter(|| black_box(tx.estimate_size()))
    });
}

criterion_group!(
    benches,
    bench_pending_tx_new,
    bench_mempool_new,
    bench_mempool_with_fee_config,
    bench_get_transactions,
    bench_evict_old,
    bench_estimate_fee,
    bench_estimate_fee_with_wealth,
    bench_cluster_factor,
    bench_suggest_fees,
    bench_dynamic_fee_state,
    bench_current_fee_base,
    bench_contains,
    bench_total_fees,
    bench_len,
    bench_is_empty,
    bench_estimate_fee_static,
    bench_transaction_hash,
    bench_transaction_estimate_size,
);

criterion_main!(benches);
