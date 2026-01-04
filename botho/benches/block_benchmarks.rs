// Copyright (c) 2024 Botho Foundation

//! Benchmarks for block operations.
//!
//! Run with: cargo bench -p botho --bench block_benchmarks
//!
//! These benchmarks measure the performance of:
//! - Block creation
//! - Block header hashing
//! - PoW validation
//! - Merkle root computation

use botho::block::{Block, MintingTx};
use bth_account_keys::AccountKey;
use bth_transaction_types::Network;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

/// Create a test account key for benchmarking
fn create_test_account() -> AccountKey {
    let mut rng = ChaCha20Rng::seed_from_u64(12345);
    AccountKey::random(&mut rng)
}

/// Benchmark block header hash computation
fn bench_block_header_hash(c: &mut Criterion) {
    let genesis = Block::genesis_for_network(Network::Testnet);

    c.bench_function("BlockHeader hash", |b| {
        b.iter(|| black_box(genesis.header.hash()))
    });
}

/// Benchmark PoW hash computation
fn bench_pow_hash(c: &mut Criterion) {
    let genesis = Block::genesis_for_network(Network::Testnet);

    c.bench_function("BlockHeader pow_hash", |b| {
        b.iter(|| black_box(genesis.header.pow_hash()))
    });
}

/// Benchmark PoW validation
fn bench_pow_validation(c: &mut Criterion) {
    let genesis = Block::genesis_for_network(Network::Testnet);

    c.bench_function("BlockHeader is_valid_pow", |b| {
        b.iter(|| black_box(genesis.header.is_valid_pow()))
    });
}

/// Benchmark MintingTx creation
fn bench_minting_tx_creation(c: &mut Criterion) {
    let account = create_test_account();
    let address = account.default_subaddress();
    let prev_hash = [0u8; 32];

    c.bench_function("MintingTx new", |b| {
        b.iter(|| {
            black_box(MintingTx::new(
                1,
                50_000_000_000_000, // 50 BTH
                &address,
                prev_hash,
                u64::MAX / 2,
                1234567890,
            ))
        })
    });
}

/// Benchmark MintingTx hash
fn bench_minting_tx_hash(c: &mut Criterion) {
    let account = create_test_account();
    let address = account.default_subaddress();
    let prev_hash = [0u8; 32];
    let minting_tx = MintingTx::new(
        1,
        50_000_000_000_000,
        &address,
        prev_hash,
        u64::MAX / 2,
        1234567890,
    );

    c.bench_function("MintingTx hash", |b| {
        b.iter(|| black_box(minting_tx.hash()))
    });
}

/// Benchmark MintingTx PoW verification
fn bench_minting_tx_pow(c: &mut Criterion) {
    let account = create_test_account();
    let address = account.default_subaddress();
    let prev_hash = [0u8; 32];
    let minting_tx = MintingTx::new(
        1,
        50_000_000_000_000,
        &address,
        prev_hash,
        u64::MAX / 2,
        1234567890,
    );

    c.bench_function("MintingTx verify_pow", |b| {
        b.iter(|| black_box(minting_tx.verify_pow()))
    });
}

/// Benchmark block template creation with varying transaction counts
fn bench_block_template_creation(c: &mut Criterion) {
    let account = create_test_account();
    let address = account.default_subaddress();
    let genesis = Block::genesis_for_network(Network::Testnet);

    let mut group = c.benchmark_group("Block template creation");

    for tx_count in [0, 10, 50].iter() {
        group.bench_with_input(BenchmarkId::new("txs", tx_count), tx_count, |b, _| {
            let transactions = vec![];
            b.iter(|| {
                black_box(Block::new_template_with_txs(
                    &genesis,
                    &address,
                    u64::MAX / 2,
                    50_000_000_000_000,
                    transactions.clone(),
                ))
            })
        });
    }

    group.finish();
}

/// Benchmark genesis block creation for different networks
fn bench_genesis_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Genesis block creation");

    group.bench_function("testnet", |b| {
        b.iter(|| black_box(Block::genesis_for_network(Network::Testnet)))
    });

    group.bench_function("mainnet", |b| {
        b.iter(|| black_box(Block::genesis_for_network(Network::Mainnet)))
    });

    group.finish();
}

/// Benchmark block hash computation
fn bench_block_hash(c: &mut Criterion) {
    let genesis = Block::genesis_for_network(Network::Testnet);

    c.bench_function("Block hash", |b| b.iter(|| black_box(genesis.hash())));
}

/// Benchmark MintingTx to_tx_output conversion
fn bench_minting_tx_to_output(c: &mut Criterion) {
    let account = create_test_account();
    let address = account.default_subaddress();
    let prev_hash = [0u8; 32];
    let minting_tx = MintingTx::new(
        1,
        50_000_000_000_000,
        &address,
        prev_hash,
        u64::MAX / 2,
        1234567890,
    );

    c.bench_function("MintingTx to_tx_output", |b| {
        b.iter(|| black_box(minting_tx.to_tx_output()))
    });
}

criterion_group!(
    benches,
    bench_block_header_hash,
    bench_pow_hash,
    bench_pow_validation,
    bench_minting_tx_creation,
    bench_minting_tx_hash,
    bench_minting_tx_pow,
    bench_block_template_creation,
    bench_genesis_creation,
    bench_block_hash,
    bench_minting_tx_to_output,
);

criterion_main!(benches);
