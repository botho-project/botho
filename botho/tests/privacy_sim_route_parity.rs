//! Pins the privacy simulation's decoy-picker ports to the production pickers
//! (#1700).
//!
//! `bth_cluster_tax::simulation::privacy_routes` cannot depend on `botho` or
//! `botho-wallet` (both depend on `cluster-tax`), so it carries ports of
//! `GammaDecoySelector::select_decoys_for_input` and
//! `select_rpc_decoys_from_history`. These tests feed identical candidate sets
//! and identically-seeded RNGs to the port and to the production function and
//! require identical output. If a production picker changes, this fails until
//! the port — and the privacy report — are updated. Since #1692 every picker
//! draws a random-offset age window; the window math is pinned for the node,
//! the wallet and the port below.
//!
//! The web picker (`web/packages/wasm-signer/src/decoys.ts`
//! `selectDecoysForInput`, #1735) is TypeScript, so its port is pinned to the
//! rings the real `send.ts` recorded in
//! `scripts/research/ct-economics/web-selection.json`
//! (`web/packages/wasm-signer/test/ct-economics-selection.test.ts`, which
//! replaces `crypto.getRandomValues` with a seeded mulberry32 stream) and to
//! the shared `test/fixtures/decoy-age-band.json` vectors. Re-recording that
//! file (`CT_ECONOMICS_WRITE_WEB=1`) after a picker change fails this test
//! until the port follows.
//!
//! Synthetic key bytes are identifiers only; no signatures are built.

use botho::decoy_selection::{
    age_similarity_band, age_similarity_envelope, age_similarity_window,
    age_similarity_window_width, draw_age_similarity_window, ClusterTags, DecoySelectionError,
    GammaDecoySelector, OutputCandidate, AGE_SIMILARITY_SPREAD_BPS, BLOCKS_PER_DAY, FACTOR_SCALE,
};
use botho_wallet::{
    ring_builder::select_rpc_decoys_from_history,
    rpc_pool::{BlockOutputs, TxOutput as RpcOutput},
};
use bth_cluster_tax::simulation::privacy_routes::{self as port, RouteCandidate, RouteError};
use bth_transaction_clsag::TxOutput;
use bth_transaction_types::ClusterTagVector;
use rand::{rngs::StdRng, RngExt, SeedableRng};

const HEIGHT: u64 = 400_000;
const COUNT: usize = 19;

fn key(id: u64) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[..8].copy_from_slice(&(id + 1).to_le_bytes());
    // Spread keys so the CLI route's sort-by-target-key is not id order.
    k[8..16].copy_from_slice(&id.wrapping_mul(0x9E37_79B9_7F4A_7C15).to_le_bytes());
    k
}

fn tx_output(id: u64) -> TxOutput {
    TxOutput {
        amount: 1_000_000_000_000,
        target_key: key(id),
        public_key: key(id + 1_000_000),
        e_memo: None,
        cluster_tags: ClusterTagVector::empty(),
        kem_ciphertext: None,
    }
}

/// A pool with mixed density: dense young outputs, sparser old ones,
/// duplicate ages (several outputs per block) and gaps. Returned in a shuffled
/// order so the node picker's first-in-order tie-break is exercised on a
/// non-sorted list, as with LMDB iteration in production.
fn pool(seed: u64) -> Vec<(u64, u64)> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut rows = Vec::new();
    let mut id = 0u64;
    let mut age = 0u64;
    while age < 300_000 {
        let outputs = rng.random_range(0..3u64);
        for _ in 0..outputs {
            rows.push((id, age));
            id += 1;
        }
        age += 1 + age / 2_000 * rng.random_range(0..3u64);
    }
    // Fisher-Yates with the same RNG for a deterministic shuffle.
    for i in (1..rows.len()).rev() {
        let j = rng.random_range(0..=i);
        rows.swap(i, j);
    }
    rows
}

fn real_ages() -> [u64; 9] {
    [5, 10, 100, 1_000, 5_000, 22_000, 30_000, 100_000, 250_000]
}

#[test]
fn constants_and_band_match_production() {
    assert_eq!(port::AGE_SIMILARITY_SPREAD_BPS, AGE_SIMILARITY_SPREAD_BPS);
    assert_eq!(
        port::MIN_DECOY_AGE_BLOCKS,
        botho_wallet::decoy_selection::MIN_DECOY_AGE_BLOCKS
    );
    use botho_wallet::decoy_selection as wallet;
    for age in (0..2_000).chain([22_000, 1_000_000, u64::MAX / 3]) {
        assert_eq!(
            port::age_similarity_band(age),
            age_similarity_band(age),
            "age {age}"
        );
        // Random-offset window (#1692): node, wallet and port agree.
        let width = age_similarity_window_width(age);
        assert_eq!(port::age_similarity_window_width(age), width, "age {age}");
        assert_eq!(wallet::age_similarity_window_width(age), width, "age {age}");
        assert_eq!(
            port::age_similarity_envelope(age),
            age_similarity_envelope(age)
        );
        assert_eq!(
            wallet::age_similarity_envelope(age),
            age_similarity_envelope(age)
        );
        for offset in [0, 1, width / 2, width.saturating_sub(1), width, width + 3] {
            let want = age_similarity_window(age, offset);
            assert_eq!(
                port::age_similarity_window(age, offset),
                want,
                "{age}@{offset}"
            );
            assert_eq!(
                wallet::age_similarity_window(age, offset),
                want,
                "{age}@{offset}"
            );
        }
    }
    // The window draw consumes the RNG identically everywhere.
    for (seed, age) in [(1u64, 0u64), (2, 9), (3, 10), (4, 1_000), (5, 1_000_000)] {
        let want = draw_age_similarity_window(age, &mut StdRng::seed_from_u64(seed));
        assert_eq!(
            port::draw_age_similarity_window(age, &mut StdRng::seed_from_u64(seed)),
            want
        );
        assert_eq!(
            wallet::draw_age_similarity_window(age, &mut StdRng::seed_from_u64(seed)),
            want
        );
    }
    // The ported gamma is the selector's default (19.28, 1.61 days at the
    // policy block time, #1692).
    assert_eq!(port::NODE_BLOCKS_PER_DAY, BLOCKS_PER_DAY as f64);
    let selector = GammaDecoySelector::new();
    assert_eq!(selector.distribution().shape(), port::NODE_GAMMA_SHAPE);
    assert_eq!(
        selector.distribution().scale_blocks(),
        port::NODE_GAMMA_SCALE_DAYS * port::NODE_BLOCKS_PER_DAY
    );
}

#[test]
fn node_port_matches_select_decoys_for_input() {
    let selector = GammaDecoySelector::new();
    let mut compared = 0;
    let mut matched_rings = 0;
    for pool_seed in [1700u64, 1692] {
        let rows = pool(pool_seed);
        let production: Vec<OutputCandidate> = rows
            .iter()
            .map(|&(id, age)| OutputCandidate {
                output: tx_output(id),
                created_at: HEIGHT - age,
                age_blocks: age,
                cluster_tags: ClusterTags::empty(),
                effective_factor: FACTOR_SCALE,
            })
            .collect();
        let ported: Vec<RouteCandidate> = rows
            .iter()
            .map(|&(id, age)| RouteCandidate {
                key: key(id),
                age_blocks: age,
            })
            .collect();
        for real_age in real_ages() {
            // Exclude one in-band output as the "real input" plus an unrelated key.
            let real = rows
                .iter()
                .min_by_key(|(_, a)| a.abs_diff(real_age))
                .map(|&(id, _)| key(id))
                .unwrap();
            let exclude = [real, key(7)];
            for seed in 0..8u64 {
                let want = selector
                    .select_decoys_for_input(
                        &production,
                        COUNT,
                        &exclude,
                        real_age,
                        &mut StdRng::seed_from_u64(seed),
                    )
                    .map(|v| v.iter().map(|o| o.target_key).collect::<Vec<_>>());
                let got = port::node_select_decoys_for_input(
                    &ported,
                    COUNT,
                    &exclude,
                    real_age,
                    &mut StdRng::seed_from_u64(seed),
                );
                match (want, got) {
                    (Ok(w), Ok(g)) => {
                        assert_eq!(w, g, "pool {pool_seed} age {real_age} seed {seed}");
                        matched_rings += 1;
                    }
                    (
                        Err(DecoySelectionError::InsufficientCandidates {
                            required,
                            available,
                        }),
                        Err(e),
                    ) => {
                        assert_eq!(
                            e,
                            RouteError::InsufficientCandidates {
                                required,
                                available
                            }
                        )
                    }
                    (w, g) => panic!("divergence at age {real_age} seed {seed}: {w:?} vs {g:?}"),
                }
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 2 * real_ages().len() * 8);
    // Sparse young bands (and the below-floor age) may legitimately fail on
    // both sides; most comparisons must still be real rings.
    assert!(
        matched_rings >= compared / 2,
        "only {matched_rings} rings compared"
    );

    // Too few candidates: both refuse with the same counts.
    let tiny: Vec<(u64, u64)> = (0..18).map(|i| (i, 100 + i)).collect();
    let production: Vec<OutputCandidate> = tiny
        .iter()
        .map(|&(id, age)| OutputCandidate {
            output: tx_output(id),
            created_at: HEIGHT - age,
            age_blocks: age,
            cluster_tags: ClusterTags::empty(),
            effective_factor: FACTOR_SCALE,
        })
        .collect();
    let ported: Vec<RouteCandidate> = tiny
        .iter()
        .map(|&(id, age)| RouteCandidate {
            key: key(id),
            age_blocks: age,
        })
        .collect();
    assert!(selector
        .select_decoys_for_input(&production, COUNT, &[], 100, &mut StdRng::seed_from_u64(0))
        .is_err());
    assert_eq!(
        port::node_select_decoys_for_input(&ported, COUNT, &[], 100, &mut StdRng::seed_from_u64(0)),
        Err(RouteError::InsufficientCandidates {
            required: 19,
            available: 18
        })
    );
}

fn rpc_output(id: u64) -> RpcOutput {
    RpcOutput {
        tx_hash: hex::encode(key(id)),
        output_index: 0,
        crypto_output_index: None,
        coinbase: false,
        lottery: false,
        ledger_outpoint: None,
        target_key: hex::encode(key(id)),
        public_key: hex::encode(key(id + 1_000_000)),
        amount_commitment: hex::encode(1_000_000_000_000u64.to_le_bytes()),
        cluster_tags: vec![],
        kem_ciphertext: None,
        lottery_context: None,
        winner_outpoint: None,
    }
}

#[test]
fn cli_port_matches_select_rpc_decoys_from_history() {
    let mut compared = 0;
    let mut matched_rings = 0;
    for pool_seed in [1700u64, 1693] {
        let rows = pool(pool_seed);
        // Group into RPC blocks by height (production sorts by height itself).
        let mut by_height: std::collections::BTreeMap<u64, Vec<RpcOutput>> = Default::default();
        for &(id, age) in &rows {
            by_height
                .entry(HEIGHT - age)
                .or_default()
                .push(rpc_output(id));
        }
        let blocks: Vec<BlockOutputs> = by_height
            .into_iter()
            .map(|(height, outputs)| BlockOutputs { height, outputs })
            .collect();
        let ported: Vec<RouteCandidate> = rows
            .iter()
            .map(|&(id, age)| RouteCandidate {
                key: key(id),
                age_blocks: age,
            })
            .collect();
        for real_age in real_ages() {
            let real = rows
                .iter()
                .min_by_key(|(_, a)| a.abs_diff(real_age))
                .map(|&(id, _)| key(id))
                .unwrap();
            let exclude = [real];
            for seed in 0..8u64 {
                let want = select_rpc_decoys_from_history(
                    &blocks,
                    HEIGHT,
                    real_age,
                    &exclude,
                    COUNT,
                    &mut StdRng::seed_from_u64(seed),
                )
                .map(|v| v.iter().map(|m| m.target_key).collect::<Vec<_>>());
                let got = port::cli_select_rpc_decoys_for_input(
                    &ported,
                    real_age,
                    &exclude,
                    COUNT,
                    &mut StdRng::seed_from_u64(seed),
                );
                match (want, got) {
                    (Ok(w), Ok(g)) => {
                        assert_eq!(w, g, "pool {pool_seed} age {real_age} seed {seed}");
                        matched_rings += 1;
                    }
                    (Err(_), Err(_)) => {}
                    (w, g) => panic!("divergence at age {real_age} seed {seed}: {w:?} vs {g:?}"),
                }
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 2 * real_ages().len() * 8);
    // Sparse young bands (and the below-floor age) may legitimately fail on
    // both sides; most comparisons must still be real rings.
    assert!(
        matched_rings >= compared / 2,
        "only {matched_rings} rings compared"
    );
}

// ============================================================================
// Web route: decoys.ts selectDecoysForInput (#1735)
// ============================================================================

const CT_SCENARIOS: &str = include_str!("../../scripts/research/ct-economics/scenarios.json");
const CT_POOLS: &str = include_str!("../../scripts/research/ct-economics/pools.json");
const CT_WEB_SELECTION: &str =
    include_str!("../../scripts/research/ct-economics/web-selection.json");
const DECOY_AGE_BAND: &str =
    include_str!("../../web/packages/wasm-signer/test/fixtures/decoy-age-band.json");

/// Replays every scenario of the CT-economics web recording through the
/// `web` port. As in `ct-economics-selection.test.ts`: the decoy pool is the
/// scenario's `pools.json` rows in RPC order (the owned real inputs are
/// excluded by `send.ts`), every real input is `real_age` old, and one
/// mulberry32 stream seeded `seeds[0] * 2 + inputs` drives the per-input
/// selections in input order. Rings must match exactly, order included.
#[test]
fn web_port_matches_recorded_decoys_ts_rings() {
    let scenarios: serde_json::Value = serde_json::from_str(CT_SCENARIOS).unwrap();
    let pools: serde_json::Value = serde_json::from_str(CT_POOLS).unwrap();
    let recorded: serde_json::Value = serde_json::from_str(CT_WEB_SELECTION).unwrap();
    let seed0 = scenarios["seeds"][0].as_u64().unwrap();
    let mut rings_compared = 0;
    let mut failures_compared = 0;
    for scenario in scenarios["scenarios"].as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap();
        let real_age = scenario["real_age"].as_u64().unwrap();
        let pool: Vec<RouteCandidate> = pools[name]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| RouteCandidate {
                key: key(row[0].as_u64().unwrap()),
                age_blocks: row[1].as_u64().unwrap(),
            })
            .collect();
        let rows = recorded[name].as_array().unwrap();
        assert_eq!(rows.len(), 2, "{name}");
        for row in rows {
            let inputs = row["inputs"].as_u64().unwrap();
            let mut prng = port::Mulberry32::new((seed0 * 2 + inputs) as u32);
            let mut next = || prng.next_u32();
            let got: Result<Vec<Vec<u64>>, RouteError> = (0..inputs)
                .map(|_| {
                    port::web_select_decoys_for_input(&pool, COUNT, real_age, &mut next).map(
                        |ring| {
                            ring.iter()
                                .map(|k| u64::from_le_bytes(k[..8].try_into().unwrap()) - 1)
                                .collect()
                        },
                    )
                })
                .collect();
            if row.get("error").is_some() {
                assert_eq!(row["error"], "insufficient_decoys", "{name}/{inputs}");
                assert!(
                    matches!(got, Err(RouteError::InsufficientCandidates { .. })),
                    "{name}/{inputs}: {got:?}"
                );
                failures_compared += 1;
            } else {
                let want: Vec<Vec<u64>> = serde_json::from_value(row["rings"].clone()).unwrap();
                assert_eq!(got.as_ref().ok(), Some(&want), "{name}/{inputs} rings");
                rings_compared += want.len();
            }
        }
    }
    // 16 scenarios x {1, 2} inputs; eighteen-decoys refuses both spends.
    assert_eq!(failures_compared, 2);
    assert_eq!(rings_compared, 15 * 3);
}

/// The shared web<->native conformance vectors (#1693, #1692): the band and
/// window math and the RNG-independent fallback selections.
#[test]
fn web_port_matches_decoy_age_band_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(DECOY_AGE_BAND).unwrap();
    assert_eq!(
        fixture["minDecoyAgeBlocks"].as_u64(),
        Some(port::MIN_DECOY_AGE_BLOCKS)
    );
    assert_eq!(
        fixture["ageSimilaritySpreadBps"].as_u64(),
        Some(port::AGE_SIMILARITY_SPREAD_BPS)
    );
    for band in fixture["bands"].as_array().unwrap() {
        let age = band["realAge"].as_u64().unwrap();
        let (lo, hi) = port::age_similarity_band(age);
        assert_eq!(lo, band["minAge"].as_u64().unwrap(), "age {age}");
        assert_eq!(hi, band["maxAge"].as_u64().unwrap(), "age {age}");
        assert_eq!(lo > hi, band["degenerate"].as_bool().unwrap(), "age {age}");
    }
    for w in fixture["windows"].as_array().unwrap() {
        let age = w["realAge"].as_u64().unwrap();
        let offset = w["offset"].as_u64().unwrap();
        assert_eq!(
            port::age_similarity_window_width(age),
            w["width"].as_u64().unwrap()
        );
        let (lo, hi) = port::age_similarity_window(age, offset);
        assert_eq!(lo, w["minAge"].as_u64().unwrap(), "{age}@{offset}");
        assert_eq!(hi, w["maxAge"].as_u64().unwrap(), "{age}@{offset}");
        assert_eq!(
            lo > hi,
            w["degenerate"].as_bool().unwrap(),
            "{age}@{offset}"
        );
    }
    for case in fixture["fallback"].as_array().unwrap() {
        let ages: Vec<u64> = serde_json::from_value(case["poolAges"].clone()).unwrap();
        let pool: Vec<RouteCandidate> = ages
            .iter()
            .enumerate()
            .map(|(i, &a)| RouteCandidate {
                key: key(i as u64),
                age_blocks: a,
            })
            .collect();
        let count = case["count"].as_u64().unwrap() as usize;
        let real_age = case["realAge"].as_u64().unwrap();
        let mut want: Vec<u64> = serde_json::from_value(case["expectedAges"].clone()).unwrap();
        want.sort();
        for seed in 0..8u32 {
            let mut prng = port::Mulberry32::new(seed);
            let ring =
                port::web_select_decoys_for_input(&pool, count, real_age, &mut || prng.next_u32())
                    .unwrap();
            let mut got: Vec<u64> = ring
                .iter()
                .map(|k| ages[(u64::from_le_bytes(k[..8].try_into().unwrap()) - 1) as usize])
                .collect();
            got.sort();
            assert_eq!(got, want, "{} seed {seed}", case["name"]);
        }
    }
}
