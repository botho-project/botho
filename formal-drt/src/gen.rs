//! Shared input types and proptest generators, biased toward the edges the
//! lottery rules branch on: 0, 1, `u64::MAX`, the eligibility boundaries (age
//! 720 and 10,000, value 1e6), cap boundaries, empty and single candidate
//! sets, and `MAX_LOTTERY_CANDIDATES` ± 1.

use crate::wire::{self, b, n};
use bth_cluster_tax::{LotteryCandidate, LotteryDrawConfig, SelectionMode};
use proptest::{collection::vec, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Selection mode, with `Hybrid` given by its basis points (see
/// `SelectionMode.hybrid` in the model).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum Mode {
    Uniform,
    ValueWeighted,
    ClusterWeighted,
    Hybrid(u32),
}

impl Mode {
    pub fn rust(self) -> SelectionMode {
        match self {
            Mode::Uniform => SelectionMode::Uniform,
            Mode::ValueWeighted => SelectionMode::ValueWeighted,
            Mode::ClusterWeighted => SelectionMode::ClusterWeighted,
            Mode::Hybrid(bps) => SelectionMode::Hybrid {
                alpha: bps as f64 / 10_000.0,
            },
        }
    }

    pub fn wire(self) -> Value {
        match self {
            Mode::Uniform => json!({ "kind": "uniform" }),
            Mode::ValueWeighted => json!({ "kind": "value_weighted" }),
            Mode::ClusterWeighted => json!({ "kind": "cluster_weighted" }),
            Mode::Hybrid(bps) => json!({ "kind": "hybrid", "alpha_bps": n(bps) }),
        }
    }
}

/// `LotteryDrawConfig` without the unused `pool_fraction`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Cfg {
    pub winners_per_draw: usize,
    pub min_utxo_age: u64,
    pub min_utxo_value: u64,
    pub circulation_window: u64,
    pub mode: Mode,
}

impl Cfg {
    pub fn canonical() -> Self {
        let d = LotteryDrawConfig::default();
        assert_eq!(d.selection_mode, SelectionMode::Uniform);
        Self {
            winners_per_draw: d.winners_per_draw,
            min_utxo_age: d.min_utxo_age,
            min_utxo_value: d.min_utxo_value,
            circulation_window: d.circulation_window,
            mode: Mode::Uniform,
        }
    }

    pub fn rust(&self) -> LotteryDrawConfig {
        LotteryDrawConfig {
            winners_per_draw: self.winners_per_draw,
            min_utxo_age: self.min_utxo_age,
            min_utxo_value: self.min_utxo_value,
            circulation_window: self.circulation_window,
            selection_mode: self.mode.rust(),
            ..LotteryDrawConfig::default()
        }
    }

    pub fn wire(&self) -> Value {
        json!({
            "winners_per_draw": n(self.winners_per_draw),
            "min_utxo_age": n(self.min_utxo_age),
            "min_utxo_value": n(self.min_utxo_value),
            "circulation_window": n(self.circulation_window),
            "mode": self.mode.wire(),
        })
    }
}

/// A lottery candidate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cand {
    #[serde(with = "wire::hex_bytes")]
    pub id: Vec<u8>,
    pub value: u64,
    pub cluster_factor: u64,
    pub creation_block: u64,
}

impl Cand {
    /// Built field by field, not with `LotteryCandidate::new`, so an
    /// out-of-range cluster factor reaches `weight` (which clamps it again).
    pub fn rust(&self) -> LotteryCandidate {
        LotteryCandidate {
            id: wire::arr(&self.id),
            value: self.value,
            cluster_factor: self.cluster_factor,
            tag_entropy: 0.0,
            creation_block: self.creation_block,
        }
    }

    pub fn wire(&self) -> Value {
        json!({
            "id": b(&self.id),
            "value": n(self.value),
            "cluster_factor": n(self.cluster_factor),
            "creation_block": n(self.creation_block),
        })
    }
}

pub fn rust_cands(cs: &[Cand]) -> Vec<LotteryCandidate> {
    cs.iter().map(Cand::rust).collect()
}

/// Any `u64`, with the edges over-weighted.
pub fn u64_edge() -> BoxedStrategy<u64> {
    prop_oneof![
        4 => any::<u64>(),
        3 => 0u64..2_000_000,
        1 => Just(0u64),
        1 => Just(1u64),
        1 => Just(u64::MAX),
        1 => Just(u64::MAX - 1),
        1 => (0u64..4).prop_map(|d| (1u64 << 63) - 2 + d),
    ]
    .boxed()
}

/// Any `u128`, with the edges (and the `u64` range) over-weighted.
pub fn u128_edge() -> BoxedStrategy<u128> {
    prop_oneof![
        3 => any::<u128>(),
        3 => any::<u64>().prop_map(u128::from),
        2 => 0u128..100_000_000_000_000_000,
        1 => Just(0u128),
        1 => Just(u128::MAX),
        1 => (0u128..1_000_000_000_000).prop_map(|d| u128::MAX - d),
    ]
    .boxed()
}

/// A block height: small, realistic, or at the top of the range.
pub fn height() -> BoxedStrategy<u64> {
    prop_oneof![
        3 => 0u64..50_000,
        3 => 50_000u64..40_000_000,
        1 => Just(u64::MAX),
        1 => (0u64..20_000).prop_map(|d| u64::MAX - d),
        1 => any::<u64>(),
    ]
    .boxed()
}

/// A UTXO age at or around the eligibility boundaries.
pub fn age() -> BoxedStrategy<u64> {
    prop_oneof![
        2 => Just(0u64),
        2 => 718u64..=722,
        2 => 9_998u64..=10_002,
        6 => 0u64..20_000,
        1 => any::<u64>(),
    ]
    .boxed()
}

/// A UTXO value at or around the dust floor.
pub fn value() -> BoxedStrategy<u64> {
    prop_oneof![
        2 => 999_998u64..=1_000_002,
        4 => 1_000_000u64..1_000_000_000_000_000,
        1 => Just(0u64),
        1 => Just(u64::MAX),
        2 => u64_edge(),
    ]
    .boxed()
}

/// A cluster factor: mostly in range, sometimes outside it.
pub fn factor() -> BoxedStrategy<u64> {
    prop_oneof![
        4 => 1_000u64..=6_000,
        1 => Just(999u64),
        1 => Just(6_001u64),
        1 => u64_edge(),
    ]
    .boxed()
}

/// A 36-byte UTXO id. Ids from a small pool collide on purpose (the draw
/// never requires distinct ids).
pub fn id36() -> BoxedStrategy<Vec<u8>> {
    prop_oneof![
        8 => vec(any::<u8>(), 36),
        1 => (0u8..4).prop_map(|x| vec![x; 36]),
    ]
    .boxed()
}

pub fn bytes32() -> BoxedStrategy<Vec<u8>> {
    prop_oneof![
        8 => vec(any::<u8>(), 32),
        1 => Just(vec![0u8; 32]),
        1 => Just(vec![0xffu8; 32]),
    ]
    .boxed()
}

/// A candidate created `age` blocks before `h` (or in the future).
pub fn cand_at(h: u64) -> BoxedStrategy<Cand> {
    (
        id36(),
        value(),
        factor(),
        prop_oneof![
            8 => age().prop_map(move |a| h.saturating_sub(a)),
            1 => u64_edge(),
        ],
    )
        .prop_map(|(id, value, cluster_factor, creation_block)| Cand {
            id,
            value,
            cluster_factor,
            creation_block,
        })
        .boxed()
}

/// A list of `item`s whose length is mostly small, sometimes empty or
/// single, rarely around `MAX_LOTTERY_CANDIDATES`. Each arm is a length
/// range, so shrinking can drop elements.
pub fn sized_vec<T: std::fmt::Debug + Clone + 'static>(
    item: BoxedStrategy<T>,
) -> BoxedStrategy<Vec<T>> {
    prop_oneof![
        999 => prop_oneof![
            2 => vec(item.clone(), 0..=1),
            8 => vec(item.clone(), 0..=12),
            3 => vec(item.clone(), 0..=64),
            1 => vec(item.clone(), 0..=300),
        ],
        1 => vec(item, 9_999..=10_002),
    ]
    .boxed()
}

/// A candidate set for height `h`.
pub fn cands_at(h: u64) -> BoxedStrategy<Vec<Cand>> {
    sized_vec(cand_at(h))
}

pub fn mode() -> BoxedStrategy<Mode> {
    prop_oneof![
        6 => Just(Mode::Uniform),
        1 => Just(Mode::ValueWeighted),
        1 => Just(Mode::ClusterWeighted),
        1 => prop_oneof![Just(0u32), Just(10_000u32), 0u32..=10_000].prop_map(Mode::Hybrid),
    ]
    .boxed()
}

/// A draw configuration: mostly the canonical one, sometimes varied.
/// `winners_per_draw` is at least 1 (the Rust divides by it; the model states
/// it as a precondition).
pub fn cfg() -> BoxedStrategy<Cfg> {
    prop_oneof![
        3 => Just(Cfg::canonical()),
        1 => mode().prop_map(|mode| Cfg { mode, ..Cfg::canonical() }),
        1 => (
            1usize..=8,
            prop_oneof![Just(720u64), Just(0u64), 0u64..2_000, Just(u64::MAX)],
            prop_oneof![Just(1_000_000u64), Just(0u64), u64_edge()],
            prop_oneof![Just(10_000u64), Just(0u64), 0u64..30_000, Just(u64::MAX)],
            mode(),
        )
            .prop_map(
                |(winners_per_draw, min_utxo_age, min_utxo_value, circulation_window, mode)| Cfg {
                    winners_per_draw,
                    min_utxo_age,
                    min_utxo_value,
                    circulation_window,
                    mode,
                },
            ),
    ]
    .boxed()
}

/// `LotteryFeeConfig::pool_fraction_permille`: 800 in consensus.
pub fn permille() -> BoxedStrategy<u32> {
    prop_oneof![
        4 => Just(800u32),
        1 => Just(0u32),
        1 => Just(1_000u32),
        1 => Just(1_001u32),
        1 => 0u32..=1_000,
        1 => any::<u32>(),
    ]
    .boxed()
}

/// A full fee configuration.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FeeCfg {
    pub pool_fraction_permille: u32,
    pub draw: Cfg,
}

impl FeeCfg {
    pub fn rust(&self) -> botho::consensus::LotteryFeeConfig {
        botho::consensus::LotteryFeeConfig {
            pool_fraction_permille: self.pool_fraction_permille,
            draw_config: self.draw.rust(),
        }
    }

    pub fn wire(&self) -> Value {
        json!({
            "pool_fraction_permille": n(self.pool_fraction_permille),
            "draw": self.draw.wire(),
        })
    }
}

pub fn fee_cfg() -> BoxedStrategy<FeeCfg> {
    (permille(), cfg())
        .prop_map(|(pool_fraction_permille, draw)| FeeCfg {
            pool_fraction_permille,
            draw,
        })
        .boxed()
}

#[cfg(test)]
mod tests {

    /// The model takes `Hybrid` as basis points; the Rust converts `alpha`
    /// back with `(alpha.clamp(0, 1) * 10_000).round()`. That round trip is
    /// exact for every basis-point value the generator produces.
    #[test]
    fn hybrid_alpha_round_trips() {
        for bps in 0u32..=10_000 {
            let alpha = bps as f64 / 10_000.0;
            assert_eq!(
                (alpha.clamp(0.0, 1.0) * 10_000.0).round() as u32,
                bps,
                "bps {bps}"
            );
        }
    }
}
