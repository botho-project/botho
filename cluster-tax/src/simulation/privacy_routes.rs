//! Production decoy-route privacy simulation (#1700).
//!
//! The legacy model in [`super::privacy`] simulates global-gamma age weighting
//! plus cluster-similarity-aware decoy selection — a design that no production
//! ring producer runs. This module instead drives faithful ports of the three
//! decoy pickers that actually build rings today and measures, per route, how
//! identifiable the real input is.
//!
//! # Routes
//!
//! | route         | production code                                                              | used by                                              |
//! |---------------|------------------------------------------------------------------------------|------------------------------------------------------|
//! | `node`        | `GammaDecoySelector::select_decoys_for_input` (`botho/src/decoy_selection.rs`) | `Ledger::get_decoy_outputs_for_input`, node wallet, detached signer |
//! | `cli`         | `select_rpc_decoys_from_history` → `select_rpc_decoy_pool` (`botho-wallet/src/ring_builder.rs`) | `botho-wallet` thin CLI wallet |
//! | `web`         | `selectDecoysForInput` (`web/packages/wasm-signer/src/decoys.ts`, #1735)      | web wallet, MetaMask Snap, claim links               |
//! | `web-pre-1735`| historical: the chain-ordered `decoyPool[(i*19 + j) % len]` window `send.ts` used before #1735 | nothing (kept for before/after comparison; not in `production`) |
//!
//! `cluster-tax` cannot depend on `botho` / `botho-wallet` (they depend on
//! `cluster-tax`), so the native pickers are **ported** here. The ports are
//! pinned to the production functions by
//! `botho/tests/privacy_sim_route_parity.rs`, which feeds identical candidates
//! and identically-seeded RNGs to both and asserts identical output.
//!
//! The `web` port is pinned the same way to the TypeScript picker's recorded
//! output: `web/packages/wasm-signer/test/ct-economics-selection.test.ts` runs
//! the real `send.ts` → `selectDecoysForInput` with `crypto.getRandomValues`
//! stubbed by a seeded mulberry32 stream and records the rings in
//! `scripts/research/ct-economics/web-selection.json`. The parity test replays
//! the same pools through [`web_select_decoys_for_input`] driven by
//! [`Mulberry32`] with the same seeds and requires identical rings (including
//! order), and checks the shared `decoy-age-band.json` fixture.
//!
//! # Chain model
//!
//! Ring privacy depends on the age density of the candidate set, so instead of
//! the legacy gamma-aged pool (which concentrates every output at 13k–33k
//! blocks, exactly where the node picker's edge clamp is weakest) the UTXO set
//! is an explicit chain: `outputs_per_block` outputs created at every height
//! `1..=tip_height`. Cluster-tag profiles for each output come from the legacy
//! [`OutputPoolGenerator`] mix (standard/exchange/whale/coinbase/mixed), drawn
//! deterministically per output id.
//!
//! # What is reported
//!
//! For each `(route, real input age)` row:
//!
//! - the real input's **age rank** within its ring (1 = youngest of 20, 20 =
//!   oldest): the full histogram, the youngest/oldest rates and a χ² statistic
//!   against the uniform distribution an ideal picker would produce;
//! - the **argmax identification rate** of each adversary: the probability the
//!   adversary's single top-scored ring member is the real input (ties split
//!   uniformly, so a uniform adversary scores exactly 1/20 = 5%).
//!
//! Adversaries: `Naive` (uniform), `Age-Heuristic` (gamma spend pdf),
//! `Age-Rank` (picker-aware: always guesses the age rank that was most often
//! the real input's on an independent set of training rings),
//! `Cluster-Fingerprint` (matches each member's tags against the output's
//! published tags) and `Combined` (age-heuristic × cluster, as in the legacy
//! model).
//!
//! The native producers stamp every output with the REAL input's tags decayed
//! one hop, in plaintext (`ClusterTagVector::merge_weighted`; decoys never
//! contribute), and none of the three pickers looks at tags. The web signer
//! publishes empty output tags, so the cluster adversary has nothing to match
//! there.

use rand::{seq::SliceRandom, Rng, RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, Gamma};
use std::collections::{HashMap, HashSet};

use super::privacy::{
    inherited_output_tags, Adversary, AgeAdversary, ClusterAdversary, CombinedAdversary,
    NaiveAdversary, OutputPoolGenerator, PoolConfig, SimulatedOutput, DECOY_COUNT, RING_SIZE,
};

// ============================================================================
// Production constants (mirrored; pinned by the botho parity test)
// ============================================================================

/// `DEFAULT_GAMMA_SHAPE` in `botho/src/decoy_selection.rs`.
pub const NODE_GAMMA_SHAPE: f64 = 19.28;
/// `DEFAULT_GAMMA_SCALE_DAYS` in `botho/src/decoy_selection.rs`.
pub const NODE_GAMMA_SCALE_DAYS: f64 = 1.61;
/// `BLOCKS_PER_DAY` in `botho/src/decoy_selection.rs` (assumes 2-minute
/// blocks; see #1692 for the block-time mismatch).
pub const NODE_BLOCKS_PER_DAY: f64 = 720.0;
/// `MIN_DECOY_AGE_BLOCKS` (node and `botho-wallet`).
pub const MIN_DECOY_AGE_BLOCKS: u64 = 10;
/// `AGE_SIMILARITY_SPREAD_BPS` (node and `botho-wallet`): ±10%.
pub const AGE_SIMILARITY_SPREAD_BPS: u64 = 1_000;

/// χ² critical value for 19 degrees of freedom at p = 0.01. A rank histogram
/// whose χ² exceeds this is distinguishable from uniform at the 1% level.
pub const CHI2_CRIT_DF19_P01: f64 = 36.191;

/// Port of `age_similarity_band` (`botho/src/decoy_selection.rs`).
pub fn age_similarity_band(real_input_age: u64) -> (u64, u64) {
    let delta = (real_input_age as u128 * AGE_SIMILARITY_SPREAD_BPS as u128 / 10_000) as u64;
    let min_age = real_input_age
        .saturating_sub(delta)
        .max(MIN_DECOY_AGE_BLOCKS);
    let max_age = real_input_age.saturating_add(delta);
    (min_age, max_age)
}

/// A decoy candidate as the pickers see it: an identity and an age.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteCandidate {
    /// One-time target key (the identity every producer de-duplicates on).
    pub key: [u8; 32],
    /// Age in blocks at the current tip.
    pub age_blocks: u64,
}

/// Selection failure, mirroring the production error cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// Not enough eligible candidates (node `InsufficientCandidates`, CLI
    /// "Not enough age-similar decoy outputs", web "Not enough decoys").
    InsufficientCandidates { required: usize, available: usize },
    /// The real input is younger than [`MIN_DECOY_AGE_BLOCKS`] (CLI guard).
    InputTooYoung,
    /// Invalid gamma parameters (node).
    InvalidDistribution,
}

fn node_gamma() -> Result<Gamma<f64>, RouteError> {
    Gamma::new(
        NODE_GAMMA_SHAPE,
        NODE_GAMMA_SCALE_DAYS * NODE_BLOCKS_PER_DAY,
    )
    .map_err(|_| RouteError::InvalidDistribution)
}

// ============================================================================
// Route `node`: GammaDecoySelector::select_decoys_for_input
// ============================================================================

/// Literal port of `GammaDecoySelector::select_decoys_for_input` with the
/// default spend distribution (the configuration every production caller
/// uses; `record_spend` has no caller).
///
/// Phase 1 draws a gamma target age per slot, clamps it into the ±10% band and
/// takes the nearest unused in-band candidate (first in `candidates` order on
/// ties). Phase 2 fills any shortfall with the nearest-aged eligible
/// candidates. Returns the selected keys in selection order.
pub fn node_select_decoys_for_input<R: Rng>(
    candidates: &[RouteCandidate],
    count: usize,
    exclude_keys: &[[u8; 32]],
    real_input_age: u64,
    rng: &mut R,
) -> Result<Vec<[u8; 32]>, RouteError> {
    let eligible: Vec<&RouteCandidate> = candidates
        .iter()
        .filter(|c| c.age_blocks >= MIN_DECOY_AGE_BLOCKS && !exclude_keys.contains(&c.key))
        .collect();

    if eligible.len() < count {
        return Err(RouteError::InsufficientCandidates {
            required: count,
            available: eligible.len(),
        });
    }

    let gamma = node_gamma()?;
    let (min_age, max_age) = age_similarity_band(real_input_age);

    let mut selected = Vec::with_capacity(count);
    let mut used_keys = exclude_keys.to_vec();

    if min_age <= max_age {
        for _ in 0..count {
            let target_age: f64 = gamma.sample(rng);
            let target_age_blocks = (target_age as u64).clamp(min_age, max_age);

            let best = eligible
                .iter()
                .filter(|c| {
                    c.age_blocks >= min_age
                        && c.age_blocks <= max_age
                        && !used_keys.contains(&c.key)
                })
                .min_by_key(|c| (c.age_blocks as i64 - target_age_blocks as i64).unsigned_abs());

            if let Some(candidate) = best {
                selected.push(candidate.key);
                used_keys.push(candidate.key);
            } else {
                break;
            }
        }
    }

    if selected.len() < count {
        let mut leftovers: Vec<&&RouteCandidate> = eligible
            .iter()
            .filter(|c| !used_keys.contains(&c.key))
            .collect();
        leftovers.sort_by_key(|c| (c.age_blocks as i64 - real_input_age as i64).unsigned_abs());
        for candidate in leftovers {
            if selected.len() >= count {
                break;
            }
            if used_keys.contains(&candidate.key) {
                continue;
            }
            selected.push(candidate.key);
            used_keys.push(candidate.key);
        }
    }

    if selected.len() < count {
        return Err(RouteError::InsufficientCandidates {
            required: count,
            available: selected.len(),
        });
    }

    Ok(selected)
}

/// Fast path for [`node_select_decoys_for_input`] over an age-sorted pool.
///
/// `band` must be exactly the eligible (non-excluded) candidates whose age lies
/// in `age_similarity_band(real_input_age)`, **sorted by ascending age**, in
/// the same relative order they have in the full candidate list (true whenever
/// the full list is itself age-sorted). `eligible_total` is the number of
/// eligible candidates in the full list.
///
/// When the band can supply all `count` decoys, phase 1 never exhausts and the
/// result — including RNG consumption and tie-breaking — is identical to the
/// literal port on the full list, but each slot costs a binary search instead
/// of a scan over the whole UTXO set. Returns `None` when that precondition
/// does not hold (degenerate or under-filled band); the caller must then run
/// the literal port. Equivalence is pinned by a unit test.
pub fn node_select_decoys_banded<R: Rng>(
    band: &[RouteCandidate],
    eligible_total: usize,
    count: usize,
    real_input_age: u64,
    rng: &mut R,
) -> Option<Result<Vec<[u8; 32]>, RouteError>> {
    if eligible_total < count {
        return Some(Err(RouteError::InsufficientCandidates {
            required: count,
            available: eligible_total,
        }));
    }
    let (min_age, max_age) = age_similarity_band(real_input_age);
    if min_age > max_age || band.len() < count {
        return None;
    }
    debug_assert!(band.windows(2).all(|w| w[0].age_blocks <= w[1].age_blocks));
    debug_assert!(band
        .iter()
        .all(|c| c.age_blocks >= min_age && c.age_blocks <= max_age));

    let gamma = match node_gamma() {
        Ok(g) => g,
        Err(e) => return Some(Err(e)),
    };

    let mut used: Vec<usize> = Vec::with_capacity(count);
    for _ in 0..count {
        let target_age: f64 = gamma.sample(rng);
        let t = (target_age as u64).clamp(min_age, max_age);
        let idx = nearest_unused(band, &used, t);
        used.push(idx);
    }
    Some(Ok(used.into_iter().map(|i| band[i].key).collect()))
}

/// Index of the unused band member nearest to `t`, breaking distance ties
/// toward the earliest index (matching `Iterator::min_by_key`).
fn nearest_unused(band: &[RouteCandidate], used: &[usize], t: u64) -> usize {
    let p = band.partition_point(|c| c.age_blocks < t);

    let mut r = p;
    while r < band.len() && used.contains(&r) {
        r += 1;
    }
    let right = (r < band.len()).then_some(r);

    let mut left = None;
    let mut l = p;
    while l > 0 {
        l -= 1;
        if !used.contains(&l) {
            left = Some(l);
            break;
        }
    }

    let leftmost_unused_with_age = |age: u64| {
        let mut i = band.partition_point(|c| c.age_blocks < age);
        while used.contains(&i) {
            i += 1;
        }
        i
    };

    match (left, right) {
        (Some(li), Some(ri)) => {
            let dl = t - band[li].age_blocks;
            let dr = band[ri].age_blocks - t;
            if dl <= dr {
                leftmost_unused_with_age(band[li].age_blocks)
            } else {
                ri
            }
        }
        (Some(li), None) => leftmost_unused_with_age(band[li].age_blocks),
        (None, Some(ri)) => ri,
        (None, None) => unreachable!("band holds at least `count` members"),
    }
}

// ============================================================================
// Route `cli`: botho-wallet select_rpc_decoys_from_history
// ============================================================================

/// Port of `prepare_rpc_decoy_pool`: drop excluded keys, sort by target key,
/// de-duplicate. `window` must already be the canonical age window.
pub fn cli_prepare_rpc_decoy_pool(
    window: &[RouteCandidate],
    exclude_keys: &[[u8; 32]],
) -> Vec<[u8; 32]> {
    let mut pool: Vec<[u8; 32]> = window
        .iter()
        .filter(|c| !exclude_keys.contains(&c.key))
        .map(|c| c.key)
        .collect();
    pool.sort();
    pool.dedup();
    pool
}

/// Port of `sample_prepared_rpc_decoy_pool`: full Fisher–Yates shuffle, then
/// take the first `count`.
pub fn cli_sample_prepared_rpc_decoy_pool<R: Rng + ?Sized>(
    mut pool: Vec<[u8; 32]>,
    count: usize,
    rng: &mut R,
) -> Result<Vec<[u8; 32]>, RouteError> {
    if pool.len() < count {
        return Err(RouteError::InsufficientCandidates {
            required: count,
            available: pool.len(),
        });
    }
    pool.shuffle(rng);
    pool.truncate(count);
    Ok(pool)
}

/// Port of `select_rpc_decoys_from_history` over an already-canonical
/// candidate set: refuse inputs younger than [`MIN_DECOY_AGE_BLOCKS`], window
/// to the ±10% age band, prepare, then sample uniformly.
pub fn cli_select_rpc_decoys_for_input<R: Rng + ?Sized>(
    candidates: &[RouteCandidate],
    real_input_age: u64,
    exclude_keys: &[[u8; 32]],
    count: usize,
    rng: &mut R,
) -> Result<Vec<[u8; 32]>, RouteError> {
    if real_input_age < MIN_DECOY_AGE_BLOCKS {
        return Err(RouteError::InputTooYoung);
    }
    let (min_age, max_age) = age_similarity_band(real_input_age);
    let window: Vec<RouteCandidate> = candidates
        .iter()
        .filter(|c| c.age_blocks >= min_age && c.age_blocks <= max_age)
        .copied()
        .collect();
    cli_sample_prepared_rpc_decoy_pool(
        cli_prepare_rpc_decoy_pool(&window, exclude_keys),
        count,
        rng,
    )
}

// ============================================================================
// Route `web`: wasm-signer decoys.ts selectDecoysForInput (#1735)
// ============================================================================

/// mulberry32, the seeded PRNG `ct-economics-selection.test.ts` substitutes
/// for `crypto.getRandomValues` when it records `web-selection.json`. Test
/// fixture only: production always uses the platform CSPRNG.
#[derive(Debug, Clone)]
pub struct Mulberry32(u32);

impl Mulberry32 {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }

    /// Next 32-bit output (identical to the TypeScript stub's per-element
    /// fill).
    pub fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }
}

/// Port of `randomIndex` (`decoys.ts`): uniform integer in `[0, n)` from a
/// u32 source by rejection sampling (no modulo bias). `n == 1` consumes
/// nothing. `n` must be in `[1, 2^32]`.
pub fn web_random_index<F: FnMut() -> u32>(n: usize, next_u32: &mut F) -> usize {
    let n = n as u64;
    assert!(
        (1..=1u64 << 32).contains(&n),
        "web_random_index: invalid bound {n}"
    );
    if n == 1 {
        return 0;
    }
    let range = 1u64 << 32;
    let limit = range - (range % n);
    loop {
        let v = next_u32() as u64;
        if v < limit {
            return (v % n) as usize;
        }
    }
}

/// Port of the in-place `shuffle` (`decoys.ts`): Fisher–Yates from the end.
fn web_shuffle<T, F: FnMut() -> u32>(items: &mut [T], next_u32: &mut F) {
    for i in (1..items.len()).rev() {
        let j = web_random_index(i + 1, next_u32);
        items.swap(i, j);
    }
}

fn web_insufficient(count: usize, available: usize) -> RouteError {
    RouteError::InsufficientCandidates {
        required: count,
        available,
    }
}

/// Literal port of `selectDecoysForInput`
/// (`web/packages/wasm-signer/src/decoys.ts`).
///
/// `pool` must already exclude the transaction's real inputs (as `send.ts`
/// does). Drops candidates younger than [`MIN_DECOY_AGE_BLOCKS`]; phase 1 takes
/// a uniform sample without replacement from the ±10% band by partial
/// Fisher–Yates over the in-band candidates (in `pool` order); phase 2 fills a
/// shortfall with the nearest-aged out-of-band candidates (shuffle, then
/// stable sort by distance); finally the selection is shuffled. Every random
/// draw is [`web_random_index`] over `next_u32`, so a seeded source reproduces
/// the TypeScript output exactly. Returns keys in final ring order.
pub fn web_select_decoys_for_input<F: FnMut() -> u32>(
    pool: &[RouteCandidate],
    count: usize,
    real_input_age: u64,
    next_u32: &mut F,
) -> Result<Vec<[u8; 32]>, RouteError> {
    let eligible: Vec<&RouteCandidate> = pool
        .iter()
        .filter(|c| c.age_blocks >= MIN_DECOY_AGE_BLOCKS)
        .collect();
    if eligible.len() < count {
        return Err(web_insufficient(count, eligible.len()));
    }
    let (min_age, max_age) = age_similarity_band(real_input_age);
    let (mut in_band, out_of_band): (Vec<&RouteCandidate>, Vec<&RouteCandidate>) = eligible
        .into_iter()
        .partition(|c| min_age <= max_age && c.age_blocks >= min_age && c.age_blocks <= max_age);

    let mut selected: Vec<&RouteCandidate> = Vec::with_capacity(count);
    let take = count.min(in_band.len());
    for i in 0..take {
        let j = i + web_random_index(in_band.len() - i, next_u32);
        in_band.swap(i, j);
        selected.push(in_band[i]);
    }

    if selected.len() < count {
        let mut leftovers = out_of_band;
        web_shuffle(&mut leftovers, next_u32);
        // `Array.prototype.sort` is stable, as is `sort_by_key`.
        leftovers.sort_by_key(|c| c.age_blocks.abs_diff(real_input_age));
        let need = count - selected.len();
        selected.extend(leftovers.into_iter().take(need));
    }

    web_shuffle(&mut selected, next_u32);
    Ok(selected.into_iter().map(|c| c.key).collect())
}

/// Fast path of [`web_select_decoys_for_input`] for the simulation when the
/// band alone can fill the ring.
///
/// `band` is exactly the in-band eligible candidates in pool order (the real
/// input excluded). Returns `None` when `band.len() < count` (phase 2 would
/// run; see [`web_fill_nearest`]). Otherwise the result — including every RNG
/// draw — is bit-identical to the literal port over any pool whose in-band
/// subsequence is `band`: phase 1's partial Fisher–Yates is replayed with a
/// sparse swap map instead of copying the band.
pub fn web_select_decoys_banded<F: FnMut() -> u32>(
    band: &[RouteCandidate],
    count: usize,
    next_u32: &mut F,
) -> Option<Vec<[u8; 32]>> {
    if band.len() < count {
        return None;
    }
    let mut swapped: HashMap<usize, usize> = HashMap::with_capacity(2 * count);
    let at = |m: &HashMap<usize, usize>, i: usize| *m.get(&i).unwrap_or(&i);
    let mut selected: Vec<[u8; 32]> = Vec::with_capacity(count);
    for i in 0..count {
        let j = i + web_random_index(band.len() - i, next_u32);
        let (vi, vj) = (at(&swapped, i), at(&swapped, j));
        swapped.insert(i, vj);
        swapped.insert(j, vi);
        selected.push(band[vj].key);
    }
    web_shuffle(&mut selected, next_u32);
    Some(selected)
}

/// Simulation stand-in for the thin-band case (`band.len() < count`) on the
/// chain model, where the literal port would shuffle the entire out-of-band
/// UTXO set per ring.
///
/// Same distribution, different RNG stream: phase 1 takes the whole band (in
/// uniformly random order, as partial Fisher–Yates does); phase 2 takes the
/// out-of-band eligible outputs in increasing age distance, and from the
/// distance group that straddles the remaining slots a uniform subset — what
/// "shuffle, then stable sort by distance, take the first k" yields; the ring
/// is then shuffled. Not bit-identical to the literal port (the parity test
/// pins the literal port; a unit test checks this agrees with it on the
/// selected distance multiset).
fn web_fill_nearest<F: FnMut() -> u32>(
    chain: &ChainModel,
    band: &[RouteCandidate],
    real_key: [u8; 32],
    real_age: u64,
    count: usize,
    next_u32: &mut F,
) -> Result<Vec<[u8; 32]>, RouteError> {
    let real_eligible = usize::from(real_age >= MIN_DECOY_AGE_BLOCKS);
    let eligible_total = chain.eligible_count() - real_eligible;
    if eligible_total < count {
        return Err(web_insufficient(count, eligible_total));
    }
    let (min_age, max_age) = age_similarity_band(real_age);
    let in_band = |a: u64| min_age <= max_age && a >= min_age && a <= max_age;
    let mut selected: Vec<[u8; 32]> = band.iter().map(|c| c.key).collect();
    let mut d = 0u64;
    while selected.len() < count {
        let mut group: Vec<[u8; 32]> = Vec::new();
        let below = (d <= real_age).then(|| real_age - d);
        let above = (d > 0).then(|| real_age + d);
        for age in below.into_iter().chain(above) {
            if age < MIN_DECOY_AGE_BLOCKS || age > chain.max_age() || in_band(age) {
                continue;
            }
            for j in 0..chain.outputs_per_block {
                let key = ChainModel::key(chain.id_at(age, j));
                if key != real_key {
                    group.push(key);
                }
            }
        }
        let need = count - selected.len();
        if group.len() <= need {
            selected.extend(group);
        } else {
            web_shuffle(&mut group, next_u32);
            selected.extend(group.into_iter().take(need));
        }
        d += 1;
    }
    web_shuffle(&mut selected, next_u32);
    Ok(selected)
}

// ============================================================================
// Route `web-pre-1735`: the chain-ordered send.ts window (historical)
// ============================================================================

/// Port of the `buildSendTransaction` decoy window that
/// `web/packages/wasm-signer/src/send.ts` used before #1735 replaced it with
/// [`web_select_decoys_for_input`]. Historical: no production code runs this
/// any more; it backs the `web-pre-1735` comparison route.
///
/// `chain_ordered` is the `chain_getOutputs(0, height)` result in chain order
/// (ascending height). The route keeps the first occurrence of each target
/// key, drops the transaction's real inputs and the all-zero placeholder, and
/// gives input `i` the decoys `pool[(i * count + j) % pool.len()]` — no age
/// band, no confirmation floor, no randomness.
///
/// Passing only a prefix of the chain is equivalent whenever the prefix still
/// holds more than `(input_index + 1) * count` usable rows (the modulo never
/// wraps); the simulation relies on that to avoid materialising the chain.
pub fn web_chain_order_decoys(
    chain_ordered: &[RouteCandidate],
    input_keys: &[[u8; 32]],
    input_index: usize,
    count: usize,
) -> Result<Vec<[u8; 32]>, RouteError> {
    let mut seen = HashSet::new();
    let pool: Vec<[u8; 32]> = chain_ordered
        .iter()
        .filter(|c| seen.insert(c.key))
        .filter(|c| !input_keys.contains(&c.key) && c.key != [0u8; 32])
        .map(|c| c.key)
        .collect();
    if pool.len() < count {
        return Err(RouteError::InsufficientCandidates {
            required: count,
            available: pool.len(),
        });
    }
    Ok((0..count)
        .map(|j| pool[(input_index * count + j) % pool.len()])
        .collect())
}

// ============================================================================
// Chain model
// ============================================================================

/// An explicit UTXO set: `outputs_per_block` outputs at every height
/// `1..=tip_height` (genesis creates no outputs). Output `id` lives at height
/// `id / outputs_per_block + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainModel {
    pub tip_height: u64,
    pub outputs_per_block: u64,
}

impl Default for ChainModel {
    fn default() -> Self {
        Self {
            tip_height: 1_200_000,
            outputs_per_block: 1,
        }
    }
}

impl ChainModel {
    /// Oldest age present in the set.
    pub fn max_age(&self) -> u64 {
        self.tip_height.saturating_sub(1)
    }

    /// Target key for output `id` (never all-zero).
    pub fn key(id: u64) -> [u8; 32] {
        let mut k = [0u8; 32];
        k[..8].copy_from_slice(&(id + 1).to_le_bytes());
        k
    }

    /// Inverse of [`Self::key`].
    pub fn id_of(key: &[u8; 32]) -> u64 {
        u64::from_le_bytes(key[..8].try_into().expect("8 bytes")) - 1
    }

    /// Age of output `id`.
    pub fn age_of(&self, id: u64) -> u64 {
        self.tip_height - (id / self.outputs_per_block + 1)
    }

    /// Output id of the `j`-th output created `age` blocks ago.
    pub fn id_at(&self, age: u64, j: u64) -> u64 {
        (self.tip_height - age - 1) * self.outputs_per_block + j
    }

    fn candidate(&self, id: u64) -> RouteCandidate {
        RouteCandidate {
            key: Self::key(id),
            age_blocks: self.age_of(id),
        }
    }

    /// Every output with `lo <= age <= hi`, ascending age.
    pub fn candidates_by_age(&self, lo: u64, hi: u64) -> Vec<RouteCandidate> {
        let hi = hi.min(self.max_age());
        if lo > hi {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(((hi - lo + 1) * self.outputs_per_block) as usize);
        for age in lo..=hi {
            for j in 0..self.outputs_per_block {
                out.push(self.candidate(self.id_at(age, j)));
            }
        }
        out
    }

    /// The first `n` outputs in chain (RPC) order, oldest first.
    pub fn chain_order_prefix(&self, n: usize) -> Vec<RouteCandidate> {
        let total = self.tip_height * self.outputs_per_block;
        (0..(n as u64).min(total))
            .map(|id| self.candidate(id))
            .collect()
    }

    /// Number of outputs at least [`MIN_DECOY_AGE_BLOCKS`] deep.
    pub fn eligible_count(&self) -> usize {
        (self.tip_height.saturating_sub(MIN_DECOY_AGE_BLOCKS) * self.outputs_per_block) as usize
    }
}

// ============================================================================
// Route simulation
// ============================================================================

/// A production ring producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecoyRoute {
    Node,
    Cli,
    /// `decoys.ts` `selectDecoysForInput` (#1735) — what web/Snap/claim links
    /// run today.
    Web,
    /// The chain-ordered window `send.ts` used before #1735. Historical
    /// comparison only; not part of [`DecoyRoute::PRODUCTION`].
    WebPre1735,
}

impl DecoyRoute {
    /// Every production route (what ring producers on `main` run today).
    pub const PRODUCTION: [DecoyRoute; 3] = [DecoyRoute::Node, DecoyRoute::Cli, DecoyRoute::Web];

    /// Short name used on the command line and in reports.
    pub fn name(&self) -> &'static str {
        match self {
            DecoyRoute::Node => "node",
            DecoyRoute::Cli => "cli",
            DecoyRoute::Web => "web",
            DecoyRoute::WebPre1735 => "web-pre-1735",
        }
    }

    /// The production code this route ports.
    pub fn producer(&self) -> &'static str {
        match self {
            DecoyRoute::Node => {
                "GammaDecoySelector::select_decoys_for_input (botho/src/decoy_selection.rs) — \
                 node ledger/wallet and detached signer"
            }
            DecoyRoute::Cli => {
                "select_rpc_decoys_from_history -> select_rpc_decoy_pool \
                 (botho-wallet/src/ring_builder.rs) — thin CLI wallet"
            }
            DecoyRoute::Web => {
                "selectDecoysForInput (web/packages/wasm-signer/src/decoys.ts, #1735) — \
                 web wallet, Snap, claim links"
            }
            DecoyRoute::WebPre1735 => {
                "HISTORICAL chain-ordered decoyPool[(i*19+j) % len] window that \
                 web/packages/wasm-signer/src/send.ts used before #1735 — no longer run"
            }
        }
    }

    /// Whether this producer publishes the real input's (decayed) cluster tags
    /// on its outputs. The native wallets do; the wasm signer emits empty tags.
    pub fn publishes_inherited_tags(&self) -> bool {
        matches!(self, DecoyRoute::Node | DecoyRoute::Cli)
    }
}

/// How the real input's age is chosen for a report row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RealAgeSpec {
    /// Every ring spends an output exactly this many blocks old.
    Fixed(u64),
    /// Real ages drawn from the node picker's own gamma spend model
    /// (k = 19.28, θ = 1.61 days at 720 blocks/day; mean ≈ 22k blocks) — the
    /// most favourable case for the node route.
    GammaSpend,
}

impl RealAgeSpec {
    /// Report label.
    pub fn label(&self) -> String {
        match self {
            RealAgeSpec::Fixed(a) => a.to_string(),
            RealAgeSpec::GammaSpend => "gamma-spend".to_string(),
        }
    }

    /// Parse `"gamma"` / `"gamma-spend"` or a block count.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        match s {
            "gamma" | "gamma-spend" => Ok(RealAgeSpec::GammaSpend),
            _ => s
                .replace('_', "")
                .parse::<u64>()
                .map(RealAgeSpec::Fixed)
                .map_err(|e| format!("invalid real age {s:?}: {e}")),
        }
    }
}

/// Configuration for the production-route simulation.
#[derive(Debug, Clone)]
pub struct RouteSimConfig {
    /// Evaluation rings per `(route, real age)` row. The same number of
    /// independent training rings is drawn for the `Age-Rank` adversary.
    pub rings: usize,
    /// The UTXO set.
    pub chain: ChainModel,
    /// Cluster-tag profile mix (only the type fractions, `num_clusters` and
    /// `decay_rate` are used).
    pub pool: PoolConfig,
    /// Master seed; every row is reproducible from it.
    pub seed: u64,
    /// Fraction of outputs carrying EMPTY cluster tags (background value,
    /// e.g. settlement outputs or web-signer outputs). Empty-tagged ring
    /// members are indistinguishable to the cluster adversary, so this is the
    /// main knob on the cluster-fingerprint rate. The legacy type mix alone
    /// tags every output.
    pub background_fraction: f64,
}

impl Default for RouteSimConfig {
    fn default() -> Self {
        Self {
            rings: 10_000,
            chain: ChainModel::default(),
            pool: PoolConfig::default(),
            seed: 1700,
            background_fraction: 0.0,
        }
    }
}

/// Adversaries reported, in column order.
pub const ROUTE_ADVERSARIES: [&str; 6] = [
    "Naive",
    "Age-Heuristic",
    "Age-Rank",
    "Band-Center",
    "Cluster-Fingerprint",
    "Combined",
];

/// Result of one `(route, real age)` row.
#[derive(Debug, Clone)]
pub struct RouteRowResult {
    pub route: DecoyRoute,
    pub real_age: RealAgeSpec,
    /// Evaluation rings successfully built.
    pub rings: usize,
    /// Evaluation rings the picker refused (insufficient decoys / too young).
    pub failures: usize,
    /// Real input's age-rank histogram over evaluation rings; index 0 = rank 1
    /// (youngest). Equal ages split their mass evenly across the tied ranks.
    pub rank_histogram: [f64; RING_SIZE],
    /// χ² of `rank_histogram` against uniform (19 degrees of freedom).
    pub chi_squared: f64,
    /// The rank (1-based) the `Age-Rank` adversary guesses, learned from the
    /// training rings.
    pub age_rank_guess: usize,
    /// Argmax identification rate per adversary, in [`ROUTE_ADVERSARIES`]
    /// order.
    pub id_rates: [f64; 6],
}

impl RouteRowResult {
    /// Fraction of rings where the real input is the strictly youngest member.
    pub fn youngest_rate(&self) -> f64 {
        self.rank_fraction(1)
    }

    /// Fraction of rings where the real input is the strictly oldest member.
    pub fn oldest_rate(&self) -> f64 {
        self.rank_fraction(RING_SIZE)
    }

    /// Fraction of rings where the real input has age rank `rank` (1-based).
    pub fn rank_fraction(&self, rank: usize) -> f64 {
        if self.rings == 0 {
            return 0.0;
        }
        self.rank_histogram[rank - 1] / self.rings as f64
    }

    /// Identification rate for a named adversary.
    pub fn id_rate(&self, adversary: &str) -> Option<f64> {
        ROUTE_ADVERSARIES
            .iter()
            .position(|a| *a == adversary)
            .map(|i| self.id_rates[i])
    }
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn row_seed(seed: u64, route: DecoyRoute, spec: RealAgeSpec, phase: u64) -> u64 {
    let route_tag = route as u64 + 1;
    let spec_tag = match spec {
        RealAgeSpec::Fixed(a) => a.wrapping_add(17),
        RealAgeSpec::GammaSpend => u64::MAX,
    };
    splitmix64(seed ^ splitmix64(route_tag ^ splitmix64(spec_tag ^ splitmix64(phase))))
}

/// Per-row cache: for a fixed real age the real input, its band and the CLI
/// prepared pool are the same for every ring.
struct RowContext {
    real_id: u64,
    band: Vec<RouteCandidate>,
    cli_pool: Vec<[u8; 32]>,
}

impl RowContext {
    fn new(chain: &ChainModel, real_age: u64, j: u64) -> Self {
        let real_id = chain.id_at(real_age, j);
        let real_key = ChainModel::key(real_id);
        let (lo, hi) = age_similarity_band(real_age);
        let band: Vec<RouteCandidate> = if lo <= hi {
            chain
                .candidates_by_age(lo, hi)
                .into_iter()
                .filter(|c| c.key != real_key)
                .collect()
        } else {
            Vec::new()
        };
        let cli_pool = cli_prepare_rpc_decoy_pool(&band, &[real_key]);
        Self {
            real_id,
            band,
            cli_pool,
        }
    }
}

/// Build one ring on `route` for `ctx`'s real input; returns decoy ids.
fn build_ring<R: Rng>(
    chain: &ChainModel,
    route: DecoyRoute,
    ctx: &RowContext,
    rng: &mut R,
) -> Result<Vec<u64>, RouteError> {
    let real_key = ChainModel::key(ctx.real_id);
    let real_age = chain.age_of(ctx.real_id);
    let keys = match route {
        DecoyRoute::Node => {
            let real_eligible = usize::from(real_age >= MIN_DECOY_AGE_BLOCKS);
            let eligible_total = chain.eligible_count() - real_eligible;
            match node_select_decoys_banded(&ctx.band, eligible_total, DECOY_COUNT, real_age, rng) {
                Some(result) => result?,
                None => {
                    // Sparse/degenerate band: run the literal port over the
                    // whole (age-sorted) UTXO set.
                    let all = chain.candidates_by_age(0, chain.max_age());
                    node_select_decoys_for_input(&all, DECOY_COUNT, &[real_key], real_age, rng)?
                }
            }
        }
        DecoyRoute::Cli => {
            if real_age < MIN_DECOY_AGE_BLOCKS {
                return Err(RouteError::InputTooYoung);
            }
            cli_sample_prepared_rpc_decoy_pool(ctx.cli_pool.clone(), DECOY_COUNT, rng)?
        }
        DecoyRoute::Web => {
            let mut next_u32 = || rng.next_u32();
            match web_select_decoys_banded(&ctx.band, DECOY_COUNT, &mut next_u32) {
                Some(keys) => keys,
                None => web_fill_nearest(
                    chain,
                    &ctx.band,
                    real_key,
                    real_age,
                    DECOY_COUNT,
                    &mut next_u32,
                )?,
            }
        }
        DecoyRoute::WebPre1735 => {
            let prefix = chain.chain_order_prefix(DECOY_COUNT + 2);
            web_chain_order_decoys(&prefix, &[real_key], 0, DECOY_COUNT)?
        }
    };
    Ok(keys.iter().map(ChainModel::id_of).collect())
}

/// Argmax identification: probability that the adversary's top-scored member
/// (ties split uniformly) is index 0, the real input.
fn argmax_share(probs: &[f64]) -> f64 {
    let max = probs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let tol = max.abs() * 1e-12;
    let ties = probs.iter().filter(|&&p| p >= max - tol).count();
    if probs[0] >= max - tol {
        1.0 / ties as f64
    } else {
        0.0
    }
}

/// `Band-Center` adversary: every native picker centres its ±10% age band on
/// the real input's age, so the midrange of the ring's ages estimates the real
/// age. Score each member by closeness to that midrange (the closest member is
/// the argmax). Uses only public ring ages.
fn band_center_scores(ring: &[SimulatedOutput]) -> Vec<f64> {
    let min = ring.iter().map(|o| o.age_blocks).min().unwrap_or(0) as f64;
    let max = ring.iter().map(|o| o.age_blocks).max().unwrap_or(0) as f64;
    let mid = (min + max) / 2.0;
    ring.iter()
        .map(|o| -(o.age_blocks as f64 - mid).abs())
        .collect()
}

/// Add the real input's (fractional, tie-split) age rank to `hist`.
fn add_rank(hist: &mut [f64; RING_SIZE], real_age: u64, decoy_ages: &[u64]) {
    let younger = decoy_ages.iter().filter(|&&a| a < real_age).count();
    let equal = decoy_ages.iter().filter(|&&a| a == real_age).count();
    let share = 1.0 / (equal + 1) as f64;
    for slot in &mut hist[younger..=younger + equal] {
        *slot += share;
    }
}

fn chi_squared(hist: &[f64; RING_SIZE], n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let expected = n as f64 / RING_SIZE as f64;
    hist.iter()
        .map(|&o| (o - expected).powi(2) / expected)
        .sum()
}

struct RingScore {
    hist: [f64; RING_SIZE],
    id: [f64; 6],
}

/// Simulate `n` rings and accumulate rank histogram plus per-adversary argmax
/// shares (the `Age-Rank` slot is filled by the caller from the histogram).
fn simulate_rings(
    config: &RouteSimConfig,
    route: DecoyRoute,
    spec: RealAgeSpec,
    n: usize,
    rng: &mut ChaCha8Rng,
) -> (RingScore, usize, usize) {
    let chain = &config.chain;
    let tags = OutputPoolGenerator::new(config.pool.clone());
    let gamma = node_gamma().expect("valid gamma");
    let fixed_ctx = match spec {
        RealAgeSpec::Fixed(a) => Some(RowContext::new(chain, a.min(chain.max_age()), 0)),
        RealAgeSpec::GammaSpend => None,
    };
    let output_with = |r: &mut ChaCha8Rng, id: u64| -> SimulatedOutput {
        let background = r.random::<f64>() < config.background_fraction;
        let mut output = tags.output_with_age(r, id, chain.age_of(id));
        if background {
            output.cluster_tags.clear();
        }
        output
    };
    // Decoys are fixed chain outputs: their tags are a deterministic function
    // of the output id.
    let output_for = |id: u64| -> SimulatedOutput {
        let mut r = ChaCha8Rng::seed_from_u64(splitmix64(config.seed ^ splitmix64(id)));
        output_with(&mut r, id)
    };
    let age_adv = AgeAdversary::default();
    let cluster_adv = ClusterAdversary::default();
    let combined_adv = CombinedAdversary::default();
    let empty_tags: HashMap<u64, u32> = HashMap::new();

    let mut score = RingScore {
        hist: [0.0; RING_SIZE],
        id: [0.0; 6],
    };
    let mut ok = 0usize;
    let mut failures = 0usize;

    for _ in 0..n {
        let gamma_ctx;
        let ctx = match &fixed_ctx {
            Some(ctx) => ctx,
            None => {
                let age = (gamma.sample(rng) as u64).clamp(MIN_DECOY_AGE_BLOCKS, chain.max_age());
                let j = rng.random_range(0..chain.outputs_per_block);
                gamma_ctx = RowContext::new(chain, age, j);
                &gamma_ctx
            }
        };
        let decoys = match build_ring(chain, route, ctx, rng) {
            Ok(d) => d,
            Err(_) => {
                failures += 1;
                continue;
            }
        };
        ok += 1;

        // The spender is a fresh draw each ring (a fixed-age row would
        // otherwise spend the same output, with the same tags, every time).
        let mut ring = Vec::with_capacity(RING_SIZE);
        ring.push(output_with(rng, ctx.real_id));
        ring.extend(decoys.iter().map(|&id| output_for(id)));

        let decoy_ages: Vec<u64> = ring[1..].iter().map(|o| o.age_blocks).collect();
        add_rank(&mut score.hist, ring[0].age_blocks, &decoy_ages);

        let output_tags = if route.publishes_inherited_tags() {
            inherited_output_tags(&ring[0])
        } else {
            empty_tags.clone()
        };
        score.id[0] += argmax_share(&NaiveAdversary.analyze(&ring, &output_tags));
        score.id[1] += argmax_share(&age_adv.analyze(&ring, &output_tags));
        score.id[3] += argmax_share(&band_center_scores(&ring));
        score.id[4] += argmax_share(&cluster_adv.analyze(&ring, &output_tags));
        score.id[5] += argmax_share(&combined_adv.analyze(&ring, &output_tags));
    }
    (score, ok, failures)
}

/// Run one `(route, real age)` row: training rings for the `Age-Rank`
/// adversary, then evaluation rings.
pub fn run_route_row(
    config: &RouteSimConfig,
    route: DecoyRoute,
    spec: RealAgeSpec,
) -> RouteRowResult {
    let mut train_rng = ChaCha8Rng::seed_from_u64(row_seed(config.seed, route, spec, 0));
    let (train, _, _) = simulate_rings(config, route, spec, config.rings, &mut train_rng);
    let guess_idx =
        train.hist.iter().enumerate().fold(
            0usize,
            |best, (i, &v)| if v > train.hist[best] { i } else { best },
        );

    let mut eval_rng = ChaCha8Rng::seed_from_u64(row_seed(config.seed, route, spec, 1));
    let (eval, rings, failures) = simulate_rings(config, route, spec, config.rings, &mut eval_rng);

    let mut id_rates = [0.0; 6];
    if rings > 0 {
        for (i, rate) in id_rates.iter_mut().enumerate() {
            *rate = eval.id[i] / rings as f64;
        }
        id_rates[2] = eval.hist[guess_idx] / rings as f64;
    }

    RouteRowResult {
        route,
        real_age: spec,
        rings,
        failures,
        rank_histogram: eval.hist,
        chi_squared: chi_squared(&eval.hist, rings),
        age_rank_guess: guess_idx + 1,
        id_rates,
    }
}

/// Run every `(route, real age)` row sequentially. The CLI parallelises over
/// rows itself; every row is independently seeded, so the order of execution
/// does not change the results.
pub fn run_route_simulation(
    config: &RouteSimConfig,
    routes: &[DecoyRoute],
    real_ages: &[RealAgeSpec],
) -> Vec<RouteRowResult> {
    routes
        .iter()
        .flat_map(|&route| {
            real_ages
                .iter()
                .map(move |&spec| run_route_row(config, route, spec))
        })
        .collect()
}

fn pct(x: f64) -> String {
    format!("{:.1}%", x * 100.0)
}

/// Format results as Markdown (one table per route plus a rank-histogram
/// table), suitable for pasting into the privacy docs verbatim.
pub fn format_route_report(config: &RouteSimConfig, results: &[RouteRowResult]) -> String {
    let mut s = String::new();
    s.push_str("## Production decoy routes: real-input identifiability\n\n");
    s.push_str(&format!(
        "Ring size {RING_SIZE} (1 real + {DECOY_COUNT} decoys). Chain model: {} outputs/block at \
         heights 1..={} (UTXO ages 0..={}). {} evaluation rings + {} training rings per row, \
         seed {}. Background (empty-tag) output fraction: {:.0}%.\n\n",
        config.chain.outputs_per_block,
        config.chain.tip_height,
        config.chain.max_age(),
        config.rings,
        config.rings,
        config.seed,
        config.background_fraction * 100.0,
    ));
    s.push_str(
        "ID = argmax identification rate (adversary's single top suspect is the real input; \
         ties split; uniform = 5.0%). Age rank 1 = youngest ring member. \
         χ² is the real input's age-rank histogram against uniform (df 19); \
         above 36.19 it is distinguishable from uniform at p = 0.01.\n",
    );

    let mut routes: Vec<DecoyRoute> = Vec::new();
    for r in results {
        if !routes.contains(&r.route) {
            routes.push(r.route);
        }
    }

    for route in routes {
        s.push_str(&format!("\n### Route `{}`\n\n", route.name()));
        s.push_str(&format!("Ports: {}.\n", route.producer()));
        s.push_str(if route.publishes_inherited_tags() {
            "Outputs carry the real input's tags decayed one hop (plaintext).\n\n"
        } else {
            "Outputs carry empty cluster tags.\n\n"
        });
        s.push_str(
            "| real age | rings | fail | youngest | oldest | rank χ² | ID Naive | ID Age-Heuristic \
             | ID Age-Rank (guess) | ID Band-Center | ID Cluster-Fingerprint | ID Combined |\n",
        );
        s.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        for r in results.iter().filter(|r| r.route == route) {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {:.0} | {} | {} | {} (r{}) | {} | {} | {} |\n",
                r.real_age.label(),
                r.rings,
                r.failures,
                pct(r.youngest_rate()),
                pct(r.oldest_rate()),
                r.chi_squared,
                pct(r.id_rates[0]),
                pct(r.id_rates[1]),
                pct(r.id_rates[2]),
                r.age_rank_guess,
                pct(r.id_rates[3]),
                pct(r.id_rates[4]),
                pct(r.id_rates[5]),
            ));
        }
    }

    s.push_str("\n### Real-input age-rank histogram (% of rings at each rank)\n\n");
    s.push_str("| route | real age |");
    for rank in 1..=RING_SIZE {
        s.push_str(&format!(" {rank} |"));
    }
    s.push_str("\n|---|---|");
    for _ in 0..RING_SIZE {
        s.push_str("---|");
    }
    s.push('\n');
    for r in results {
        s.push_str(&format!("| {} | {} |", r.route.name(), r.real_age.label()));
        for rank in 1..=RING_SIZE {
            s.push_str(&format!(" {:.0} |", r.rank_fraction(rank) * 100.0));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;

    fn k(id: u64) -> [u8; 32] {
        ChainModel::key(id)
    }

    #[test]
    fn band_matches_production_policy() {
        assert_eq!(age_similarity_band(100), (90, 110));
        assert_eq!(age_similarity_band(1_000_000), (900_000, 1_100_000));
        // Degenerate band for an input below the confirmation floor.
        assert_eq!(age_similarity_band(5), (10, 5));
    }

    /// The banded fast path must be bit-identical to the literal port —
    /// including RNG consumption and first-in-order tie breaks — on age-sorted
    /// pools with gaps and duplicate ages.
    #[test]
    fn banded_node_path_equals_literal_port() {
        let mut gen = StdRng::seed_from_u64(1700);
        let mut checked = 0;
        for trial in 0..60u64 {
            // Pool with gaps (step 0..=3) and duplicates (step 0).
            let mut ages = Vec::new();
            let mut age = 10 + gen.random_range(0..5u64);
            let len = 200 + gen.random_range(0..400usize);
            for _ in 0..len {
                ages.push(age);
                age += gen.random_range(0..4u64);
            }
            let all: Vec<RouteCandidate> = ages
                .iter()
                .enumerate()
                .map(|(i, &a)| RouteCandidate {
                    key: k(i as u64 + trial * 10_000),
                    age_blocks: a,
                })
                .collect();
            let real_idx = gen.random_range(0..all.len());
            let real = all[real_idx];
            let (lo, hi) = age_similarity_band(real.age_blocks);
            let band: Vec<RouteCandidate> = all
                .iter()
                .filter(|c| c.key != real.key && c.age_blocks >= lo && c.age_blocks <= hi)
                .copied()
                .collect();
            let eligible_total = all
                .iter()
                .filter(|c| c.key != real.key && c.age_blocks >= MIN_DECOY_AGE_BLOCKS)
                .count();
            for seed in 0..5u64 {
                let fast = node_select_decoys_banded(
                    &band,
                    eligible_total,
                    DECOY_COUNT,
                    real.age_blocks,
                    &mut StdRng::seed_from_u64(seed),
                );
                let literal = node_select_decoys_for_input(
                    &all,
                    DECOY_COUNT,
                    &[real.key],
                    real.age_blocks,
                    &mut StdRng::seed_from_u64(seed),
                );
                if let Some(fast) = fast {
                    assert_eq!(fast, literal, "trial {trial} seed {seed}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 100, "fast path exercised only {checked} times");
    }

    /// The `web-pre-1735` port reproduces every ring the pre-#1735 `send.ts`
    /// route recorded in `scripts/research/ct-economics/web-selection.json`
    /// BEFORE #1735 re-recorded that file (`git show
    /// 596f64c0:scripts/research/ct-economics/web-selection.json`; pools from
    /// `pools.json`; inputs are keys 10000/10001 listed first by the RPC mock).
    /// The constants below are from that historical recording, not the current
    /// file: the 16 scenarios reduce to these four pool shapes, each checked
    /// for one- and two-input spends. The current recording pins the `web`
    /// route instead (`botho/tests/privacy_sim_route_parity.rs`).
    #[test]
    fn web_pre_1735_port_matches_pre_1735_recording() {
        // The RPC mock lists the spend's owned outputs (keys 10000, 10001, ...)
        // first, then the pool rows in order.
        let rows = |n_inputs: u64, ids: &[u64]| -> Vec<RouteCandidate> {
            (10_000..10_000 + n_inputs)
                .chain(ids.iter().copied())
                .map(|i| RouteCandidate {
                    key: k(i),
                    age_blocks: 100,
                })
                .collect()
        };
        let inputs = [k(10_000), k(10_001)];
        let ids_of = |ring: Vec<[u8; 32]>| ring.iter().map(ChainModel::id_of).collect::<Vec<_>>();
        // Rings the recorded route produced for a one-input and a two-input
        // spend over `ids`.
        let rings = |ids: &[u64]| -> (Vec<u64>, Vec<u64>, Vec<u64>) {
            let one = rows(1, ids);
            let two = rows(2, ids);
            (
                ids_of(web_chain_order_decoys(&one, &inputs[..1], 0, 19).unwrap()),
                ids_of(web_chain_order_decoys(&two, &inputs, 0, 19).unwrap()),
                ids_of(web_chain_order_decoys(&two, &inputs, 1, 19).unwrap()),
            )
        };

        // background / old-input / sparse-age / ...: ids 1..=512 in order.
        let forward: Vec<u64> = (1..=512).collect();
        let first: Vec<u64> = (1..=19).collect();
        assert_eq!(
            rings(&forward),
            (first.clone(), first.clone(), (20..=38).collect())
        );

        // rpc-reversed: ids 512..=1.
        let reversed: Vec<u64> = (1..=512).rev().collect();
        let first: Vec<u64> = (494..=512).rev().collect();
        assert_eq!(
            rings(&reversed),
            (first.clone(), first.clone(), (475..=493).rev().collect())
        );

        // nineteen-decoys: the second input wraps onto the same 19.
        let nineteen: Vec<u64> = (1..=19).collect();
        assert_eq!(
            rings(&nineteen),
            (nineteen.clone(), nineteen.clone(), nineteen.clone())
        );

        // eighteen-decoys: insufficient_decoys for both spends.
        let eighteen: Vec<u64> = (1..=18).collect();
        for n in 1..=2u64 {
            assert_eq!(
                web_chain_order_decoys(&rows(n, &eighteen), &inputs[..n as usize], 0, 19),
                Err(RouteError::InsufficientCandidates {
                    required: 19,
                    available: 18
                })
            );
        }
    }

    #[test]
    fn chain_model_round_trips() {
        let chain = ChainModel {
            tip_height: 1_000,
            outputs_per_block: 3,
        };
        for age in [0u64, 1, 10, 500, 999] {
            for j in 0..3 {
                let id = chain.id_at(age, j);
                assert_eq!(chain.age_of(id), age);
                assert_eq!(ChainModel::id_of(&ChainModel::key(id)), id);
            }
        }
        let band = chain.candidates_by_age(90, 110);
        assert_eq!(band.len(), 21 * 3);
        assert!(band.windows(2).all(|w| w[0].age_blocks <= w[1].age_blocks));
        // Chain order is oldest first.
        assert_eq!(chain.chain_order_prefix(1)[0].age_blocks, 999);
        assert_eq!(chain.eligible_count(), 990 * 3);
    }

    #[test]
    fn argmax_share_splits_ties() {
        assert_eq!(argmax_share(&[0.05; 20]), 0.05);
        assert_eq!(argmax_share(&[0.5, 0.5, 0.0]), 0.5);
        assert_eq!(argmax_share(&[0.2, 0.8]), 0.0);
        assert_eq!(argmax_share(&[0.9, 0.1]), 1.0);
    }

    fn small_config() -> RouteSimConfig {
        RouteSimConfig {
            rings: 300,
            chain: ChainModel {
                tip_height: 60_000,
                outputs_per_block: 1,
            },
            pool: PoolConfig::default(),
            seed: 1700,
            background_fraction: 0.0,
        }
    }

    /// With every output tagged, the native routes' plaintext inherited tags
    /// identify the real input; background (empty-tag) outputs dilute it.
    #[test]
    fn cluster_fingerprint_depends_on_background_fraction() {
        let tagged = run_route_row(&small_config(), DecoyRoute::Cli, RealAgeSpec::Fixed(5_000));
        assert!(
            tagged.id_rates[4] > 0.95,
            "cluster ID {}",
            tagged.id_rates[4]
        );
        let half = RouteSimConfig {
            background_fraction: 0.5,
            ..small_config()
        };
        let diluted = run_route_row(&half, DecoyRoute::Cli, RealAgeSpec::Fixed(5_000));
        assert!(diluted.id_rates[4] < tagged.id_rates[4]);
        assert!(
            diluted.id_rates[4] > 0.4,
            "cluster ID {}",
            diluted.id_rates[4]
        );
        // The web signer publishes empty output tags: nothing to match.
        let web = run_route_row(&small_config(), DecoyRoute::Web, RealAgeSpec::Fixed(5_000));
        assert!(web.id_rates[4] < 0.2, "web cluster ID {}", web.id_rates[4]);
    }

    /// The CLI route draws uniformly from the ±10% band CENTRED on the real
    /// age, so each decoy is younger than the real input with probability ½:
    /// the real input's rank is Binomial(19, ½) + 1, not uniform. It is the
    /// median-age member, and the midrange of the ring's ages pins it down.
    #[test]
    fn cli_route_centred_band_leaks_the_median() {
        let r = run_route_row(&small_config(), DecoyRoute::Cli, RealAgeSpec::Fixed(5_000));
        assert_eq!(r.failures, 0);
        assert!(
            (r.id_rates[0] - 0.05).abs() < 1e-9,
            "Naive must be exactly 1/20"
        );
        assert!(r.chi_squared > CHI2_CRIT_DF19_P01, "χ² = {}", r.chi_squared);
        assert!(
            (10..=11).contains(&r.age_rank_guess),
            "guess r{}",
            r.age_rank_guess
        );
        assert!(r.id_rates[2] > 0.12, "Age-Rank ID {}", r.id_rates[2]);
        assert!(r.id_rates[3] > 0.20, "Band-Center ID {}", r.id_rates[3]);
    }

    /// Pins the current node picker's edge clamp (#1692): at real ages well
    /// below the gamma bulk every target clamps to the band's upper edge, so
    /// the real input is the youngest ring member. When #1692 lands the botho
    /// parity test forces this port to change and this pin must follow.
    #[test]
    fn node_route_pins_edge_clamp() {
        let r = run_route_row(&small_config(), DecoyRoute::Node, RealAgeSpec::Fixed(1_000));
        assert!(r.youngest_rate() > 0.95, "youngest {}", r.youngest_rate());
        assert_eq!(r.age_rank_guess, 1);
        assert!(r.id_rates[2] > 0.95);
    }

    /// The pre-#1735 web route always uses the chain's oldest outputs.
    #[test]
    fn web_pre_1735_route_pins_chain_order() {
        let r = run_route_row(
            &small_config(),
            DecoyRoute::WebPre1735,
            RealAgeSpec::Fixed(1_000),
        );
        assert_eq!(r.youngest_rate(), 1.0);
        assert_eq!(r.id_rates[2], 1.0);
    }

    /// Same outputs as the TypeScript mulberry32 stub
    /// (`ct-economics-selection.test.ts`, seed 1306 * 2 + 1).
    #[test]
    fn mulberry32_matches_typescript_stub() {
        let mut m = Mulberry32::new(1306 * 2 + 1);
        let got: Vec<u32> = (0..4).map(|_| m.next_u32()).collect();
        assert_eq!(got, [3_491_642_415, 324_970_104, 922_888_339, 560_157_628]);
    }

    /// `randomIndex` rejection sampling: draws at or above the largest
    /// multiple of `n` are rejected; `n == 1` consumes nothing.
    #[test]
    fn web_random_index_rejects_biased_draws() {
        let mut draws = vec![u32::MAX, 7].into_iter();
        let mut next = || draws.next().expect("draw");
        // 2^32 % 3 == 1, so u32::MAX is rejected and 7 % 3 == 1 accepted.
        assert_eq!(web_random_index(3, &mut next), 1);
        let mut none = || -> u32 { panic!("n == 1 must not draw") };
        assert_eq!(web_random_index(1, &mut none), 0);
        let mut one = || 5u32;
        assert_eq!(web_random_index(1 << 32, &mut one), 5);
    }

    fn aged_pool(gen: &mut StdRng, trial: u64) -> Vec<RouteCandidate> {
        let mut out = Vec::new();
        let mut age = gen.random_range(0..15u64);
        let len = 50 + gen.random_range(0..400usize);
        for i in 0..len {
            out.push(RouteCandidate {
                key: k(i as u64 + trial * 10_000),
                age_blocks: age,
            });
            age += gen.random_range(0..4u64);
        }
        // Shuffle so pool order is not age order.
        out.shuffle(gen);
        out
    }

    /// The banded fast path (sparse-swap partial Fisher–Yates) is
    /// bit-identical to the literal `selectDecoysForInput` port, RNG
    /// consumption included, whenever the band fills the ring.
    #[test]
    fn web_banded_path_equals_literal_port() {
        let mut gen = StdRng::seed_from_u64(1735);
        let mut checked = 0;
        for trial in 0..80u64 {
            let pool = aged_pool(&mut gen, trial);
            let real_age = pool[gen.random_range(0..pool.len())].age_blocks;
            let (lo, hi) = age_similarity_band(real_age);
            let band: Vec<RouteCandidate> = pool
                .iter()
                .filter(|c| {
                    c.age_blocks >= MIN_DECOY_AGE_BLOCKS
                        && lo <= hi
                        && c.age_blocks >= lo
                        && c.age_blocks <= hi
                })
                .copied()
                .collect();
            for seed in 0..4u32 {
                let mut a = Mulberry32::new(seed);
                let mut b = Mulberry32::new(seed);
                let literal =
                    web_select_decoys_for_input(&pool, DECOY_COUNT, real_age, &mut || a.next_u32());
                let fast = web_select_decoys_banded(&band, DECOY_COUNT, &mut || b.next_u32());
                if let Some(fast) = fast {
                    assert_eq!(Ok(fast), literal, "trial {trial} seed {seed}");
                    // Both consumed the same number of draws.
                    assert_eq!(a.next_u32(), b.next_u32());
                    checked += 1;
                }
            }
        }
        assert!(checked > 100, "fast path exercised only {checked} times");
    }

    /// Thin band on the chain model: the simulation's nearest-age fill picks
    /// the same multiset of age distances as the literal port (only the
    /// random choice within the straddling distance group differs).
    #[test]
    fn web_thin_band_fill_matches_literal_distances() {
        let chain = ChainModel {
            tip_height: 400,
            outputs_per_block: 2,
        };
        let all = chain.candidates_by_age(0, chain.max_age());
        for real_age in [0u64, 5, 9, 10, 12, 20, 40] {
            let real_key = ChainModel::key(chain.id_at(real_age, 0));
            let pool: Vec<RouteCandidate> =
                all.iter().filter(|c| c.key != real_key).copied().collect();
            let (lo, hi) = age_similarity_band(real_age);
            let band: Vec<RouteCandidate> = pool
                .iter()
                .filter(|c| {
                    c.age_blocks >= MIN_DECOY_AGE_BLOCKS
                        && lo <= hi
                        && c.age_blocks >= lo
                        && c.age_blocks <= hi
                })
                .copied()
                .collect();
            assert!(band.len() < DECOY_COUNT, "age {real_age} band not thin");
            for seed in 0..5u32 {
                let mut a = Mulberry32::new(seed);
                let mut b = Mulberry32::new(seed + 100);
                let dists = |keys: Vec<[u8; 32]>| {
                    let mut d: Vec<u64> = keys
                        .iter()
                        .map(|key| chain.age_of(ChainModel::id_of(key)).abs_diff(real_age))
                        .collect();
                    d.sort();
                    d
                };
                let literal =
                    web_select_decoys_for_input(&pool, DECOY_COUNT, real_age, &mut || a.next_u32())
                        .unwrap();
                let fast =
                    web_fill_nearest(&chain, &band, real_key, real_age, DECOY_COUNT, &mut || {
                        b.next_u32()
                    })
                    .unwrap();
                assert_eq!(fast.iter().collect::<HashSet<_>>().len(), DECOY_COUNT);
                assert!(!fast.contains(&real_key));
                assert_eq!(dists(fast), dists(literal), "age {real_age} seed {seed}");
            }
        }
    }

    /// #1735's web picker draws uniformly from the same centred ±10% band as
    /// the CLI route, so it inherits the centred-band median leak (#1755)
    /// instead of the pre-#1735 "real input is the youngest" leak.
    #[test]
    fn web_route_centred_band_leaks_the_median() {
        let r = run_route_row(&small_config(), DecoyRoute::Web, RealAgeSpec::Fixed(5_000));
        assert_eq!(r.failures, 0);
        assert!(r.youngest_rate() < 0.01, "youngest {}", r.youngest_rate());
        assert!(r.chi_squared > CHI2_CRIT_DF19_P01, "χ² = {}", r.chi_squared);
        assert!(
            (10..=11).contains(&r.age_rank_guess),
            "guess r{}",
            r.age_rank_guess
        );
        assert!(r.id_rates[3] > 0.20, "Band-Center ID {}", r.id_rates[3]);
        // Empty output tags: the cluster adversary has nothing to match.
        assert!(r.id_rates[4] < 0.2, "cluster ID {}", r.id_rates[4]);
    }

    #[test]
    fn rows_are_reproducible() {
        let cfg = RouteSimConfig {
            rings: 50,
            ..small_config()
        };
        let a = run_route_row(&cfg, DecoyRoute::Cli, RealAgeSpec::GammaSpend);
        let b = run_route_row(&cfg, DecoyRoute::Cli, RealAgeSpec::GammaSpend);
        assert_eq!(a.rank_histogram, b.rank_histogram);
        assert_eq!(a.id_rates, b.id_rates);
    }

    #[test]
    fn signature_size_matches_production_layout() {
        // c_zero (32) + 20 responses (640) + key image (32) + D (32).
        assert_eq!(super::super::privacy::signature_size_bytes(RING_SIZE), 736);
    }

    #[test]
    fn real_age_spec_parses() {
        assert_eq!(RealAgeSpec::parse("gamma"), Ok(RealAgeSpec::GammaSpend));
        assert_eq!(RealAgeSpec::parse("1_000"), Ok(RealAgeSpec::Fixed(1000)));
        assert!(RealAgeSpec::parse("x").is_err());
    }
}
