//! DRT for the #1711 demurrage models against actual cluster-tax
//! functions. Since #1875 (protocol 9.0.0) the `demurrage_charge` leg runs
//! the exact model `Botho.Demurrage.exactCharge`; the legacy quantized model
//! `Botho.Demurrage.charge` keeps its counterexample theorems.
use crate::{fixtures::Fixtures, gen, legs::Leg, wire};
use bth_cluster_tax::{
    capitalized_reset_charge, demurrage_charge, demurrage_settlement_charge,
    ring_centroid_implied_factor, ring_elapsed_centroid, ring_elapsed_quantile,
    spend_demurrage_charge, ClusterFactorCurve, SETTLEMENT_HORIZON_BLOCKS,
};
use proptest::{collection::vec, prelude::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub struct Charge;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChargeIn {
    pub value: u64,
    pub factor: u64,
    pub elapsed: u64,
    pub rate: u32,
    pub year: u64,
}

impl Leg for Charge {
    const NAME: &'static str = "demurrage_charge";
    const RUST: &'static str = "bth_cluster_tax::demurrage_charge";
    type Input = ChargeIn;
    fn strategy() -> BoxedStrategy<ChargeIn> {
        (
            gen::u64_edge(),
            gen::factor(),
            prop_oneof![
                gen::u64_edge(),
                0u64..=14,
                Just(6_307_200),
                Just(321_667_200)
            ],
            prop_oneof![Just(0u32), Just(200u32), any::<u32>()],
            // 368_934_881_474 is where the #1908 study's u128 kernel stopped
            // being exact; the production kernel splits elapsed so it is
            // exact on both sides (#1875).
            prop_oneof![
                3 => Just(6_307_200u64),
                1 => gen::u64_edge(),
                1 => prop_oneof![
                    Just(368_934_881_474u64),
                    Just(368_934_881_475u64),
                    368_934_881_475u64..=u64::MAX
                ]
            ],
        )
            .prop_map(|(value, factor, elapsed, rate, year)| ChargeIn {
                value,
                factor,
                elapsed,
                rate,
                year,
            })
            .boxed()
    }
    fn args(_: &Fixtures, i: &ChargeIn) -> Value {
        json!({"value":wire::n(i.value), "factor":wire::n(i.factor),
            "elapsed":wire::n(i.elapsed), "rate":wire::n(i.rate), "year":wire::n(i.year)})
    }
    fn rust(_: &Fixtures, i: &ChargeIn) -> Value {
        wire::n(demurrage_charge(
            i.value, i.factor, i.elapsed, i.rate, i.year,
        ))
    }
    fn classify(i: &ChargeIn, r: &Value) -> String {
        if i.year == 6_307_200
            && (1..=6).contains(&i.elapsed)
            && i.rate > 0
            && i.factor > 1000
            && i.value > 0
        {
            "positive inputs, short hold (pre-9.0.0 dead zone)".into()
        } else if r == &wire::n(u64::MAX) {
            "clamped to u64 max".into()
        } else if r == &wire::n(0) {
            "zero".into()
        } else {
            "positive".into()
        }
    }
}

pub struct Factor;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactorIn {
    #[serde(with = "wire::dec_u128")]
    pub wealth: u128,
}

impl Leg for Factor {
    const NAME: &'static str = "default_cluster_factor";
    const RUST: &'static str = "bth_cluster_tax::ClusterFactorCurve::default_params().factor";
    type Input = FactorIn;
    fn strategy() -> BoxedStrategy<FactorIn> {
        prop_oneof![gen::u128_edge(), 0u128..=1_000_000_000_000_000_000_000]
            .prop_map(|wealth| FactorIn { wealth })
            .boxed()
    }
    fn args(_: &Fixtures, i: &FactorIn) -> Value {
        json!({"wealth":wire::n(i.wealth)})
    }
    fn rust(_: &Fixtures, i: &FactorIn) -> Value {
        wire::n(ClusterFactorCurve::default_params().factor(i.wealth))
    }
    fn classify(_: &FactorIn, r: &Value) -> String {
        if r == &wire::n(1000) {
            "minimum"
        } else if r == &wire::n(6000) {
            "maximum"
        } else {
            "interior"
        }
        .into()
    }
}

pub struct RingFactor;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub value: u64,
    #[serde(with = "wire::dec_u128")]
    pub wealth: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RingIn {
    pub members: Vec<Member>,
}

impl Leg for RingFactor {
    const NAME: &'static str = "ring_centroid_factor";
    const RUST: &'static str = "bth_cluster_tax::ring_centroid_implied_factor";
    type Input = RingIn;
    fn strategy() -> BoxedStrategy<RingIn> {
        let member = (gen::u64_edge(), gen::u128_edge())
            .prop_map(|(value, wealth)| Member { value, wealth });
        prop_oneof![
            vec(member, 0..=40),
            (gen::u64_edge(), gen::u128_edge()).prop_map(|(value, wealth)| vec![
                Member {
                    value,
                    wealth
                };
                20
            ]),
        ]
        .prop_map(|members| RingIn { members })
        .boxed()
    }
    fn args(_: &Fixtures, i: &RingIn) -> Value {
        json!({"members":wire::list(&i.members, |m|
            json!({"value":wire::n(m.value), "wealth":wire::n(m.wealth)}))})
    }
    fn rust(_: &Fixtures, i: &RingIn) -> Value {
        let members: Vec<_> = i.members.iter().map(|m| (m.value, m.wealth)).collect();
        wire::n(ring_centroid_implied_factor(
            &members,
            &ClusterFactorCurve::default_params(),
        ))
    }
    fn classify(i: &RingIn, _: &Value) -> String {
        let sum = i.members.iter().try_fold(0u128, |s, m| {
            (m.value as u128)
                .checked_mul(m.wealth)
                .and_then(|p| s.checked_add(p))
        });
        if i.members.is_empty() {
            "empty"
        } else if sum.is_none() {
            "weighted numerator exceeds u128 (pre-#1876 saturated)"
        } else {
            "weighted numerator fits u128"
        }
        .into()
    }
}

/// One case compares all three wrappers; horizon, elapsed and year are
/// independent.
pub struct Composition;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompositionIn {
    pub value: u64,
    pub input_factor: u64,
    pub output_factor: u64,
    pub horizon: u64,
    pub elapsed: u64,
    pub rate: u32,
    pub year: u64,
}

impl Leg for Composition {
    const NAME: &'static str = "demurrage_composition";
    const RUST: &'static str = "bth_cluster_tax::{capitalized_reset_charge, demurrage_settlement_charge, spend_demurrage_charge}";
    type Input = CompositionIn;

    fn strategy() -> BoxedStrategy<Self::Input> {
        let factor = || {
            prop_oneof![
                3 => gen::u64_edge(),
                3 => 1000u64..=6000,
                2 => proptest::sample::select(vec![0u64, 999, 1000, 1001, 5999, 6000, 6001]),
            ]
        };
        let blocks = || {
            prop_oneof![
                3 => gen::u64_edge(),
                2 => 0u64..=630_720_000,
                1 => proptest::sample::select(vec![0u64, 1, 6_307_200, 31_536_000, 315_360_000, 630_720_000]),
            ]
        };
        let wide = (
            gen::u64_edge(),
            factor(),
            factor(),
            blocks(),
            blocks(),
            prop_oneof![Just(0u32), Just(200u32), Just(10_000u32), any::<u32>()],
            prop_oneof![
                3 => gen::u64_edge(),
                2 => Just(6_307_200u64),
                1 => proptest::sample::select(vec![0u64, 1, 368_934_881_474, 368_934_881_475, u64::MAX]),
            ],
        );
        // Half ordinary-sized inputs: independent factors/times and positive
        // rates avoid an all-zero/all-capped full-width campaign. The other
        // half still has support over every typed input, including year zero.
        let ordinary = (
            1u64..=1_000_000_000_000_000_000,
            1000u64..=6000,
            1000u64..=6000,
            0u64..=63_072_000,
            0u64..=630_720_000,
            1u32..=1000,
            Just(6_307_200u64),
        );
        prop_oneof![wide, ordinary]
            .prop_map(
                |(value, input_factor, output_factor, horizon, elapsed, rate, year)| {
                    CompositionIn {
                        value,
                        input_factor,
                        output_factor,
                        horizon,
                        elapsed,
                        rate,
                        year,
                    }
                },
            )
            .boxed()
    }

    fn args(_: &Fixtures, i: &Self::Input) -> Value {
        json!({"value":wire::n(i.value), "input_factor":wire::n(i.input_factor),
            "output_factor":wire::n(i.output_factor), "horizon":wire::n(i.horizon),
            "elapsed":wire::n(i.elapsed), "rate":wire::n(i.rate), "year":wire::n(i.year)})
    }

    fn rust(_: &Fixtures, i: &Self::Input) -> Value {
        json!({
            "reset": wire::n(capitalized_reset_charge(i.value, i.input_factor, i.output_factor, i.horizon, i.rate, i.year)),
            "settlement": wire::n(demurrage_settlement_charge(i.value, i.input_factor, i.rate, i.year)),
            "spend": wire::n(spend_demurrage_charge(i.value, i.input_factor, i.output_factor, i.elapsed, i.rate, i.year)),
        })
    }

    fn classify(i: &Self::Input, r: &Value) -> String {
        let outcome = |field: &str| {
            if r[field] == wire::n(0) {
                "zero"
            } else if r[field] == wire::n(u64::MAX) {
                "cap"
            } else {
                "positive"
            }
        };
        // Classification calls the existing Rust kernels; it does not supply
        // an expected answer or implement a second charge formula.
        let floor = demurrage_charge(i.value, i.input_factor, i.horizon, i.rate, i.year);
        let output = demurrage_charge(i.value, i.output_factor, i.horizon, i.rate, i.year);
        let caps = u8::from(floor == u64::MAX) + u8::from(output == u64::MAX);
        let accrued = demurrage_charge(i.value, i.input_factor, i.elapsed, i.rate, i.year);
        let reset = capitalized_reset_charge(
            i.value,
            i.input_factor,
            i.output_factor,
            SETTLEMENT_HORIZON_BLOCKS,
            i.rate,
            i.year,
        );
        let dominant = match accrued.cmp(&reset) {
            std::cmp::Ordering::Less => "reset",
            std::cmp::Ordering::Equal => "equal",
            std::cmp::Ordering::Greater => "accrued",
        };
        let year = if i.year == 0 {
            "zero"
        } else if i.year > 368_934_881_474 {
            "wide"
        } else {
            "ordinary"
        };
        format!(
            "reset={};settlement={};spend={};reset_caps={caps};dominant={dominant};year={year}",
            outcome("reset"),
            outcome("settlement"),
            outcome("spend")
        )
    }
}

#[cfg(test)]
mod composition_tests {
    use super::*;
    use crate::model::Model;
    use std::path::Path;

    #[derive(Deserialize)]
    struct Vector {
        input: CompositionIn,
        expected: Value,
        note: String,
    }

    /// Unlike runner replay (inputs only), this test asserts live numerical
    /// outputs against reviewed fixture values on BOTH independent sides.
    #[test]
    fn composition_live_rust_and_lean_vectors() {
        let vectors: Vec<Vector> = include_str!("../regressions/demurrage_composition.jsonl")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let fx = Fixtures::new();
        let mut model = Model::spawn(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("lean/.lake/build/bin/botho-model"),
        )
        .expect("build the Lean DRT executable before running DRT tests");
        let requests: Vec<_> = vectors
            .iter()
            .map(|v| wire::request(Composition::NAME, &Composition::args(&fx, &v.input)))
            .collect();
        let lean = model.call_batch(&requests);
        for (v, actual_lean) in vectors.iter().zip(lean) {
            assert_eq!(
                Composition::rust(&fx, &v.input),
                v.expected,
                "Rust: {}",
                v.note
            );
            assert_eq!(actual_lean, v.expected, "Lean: {}", v.note);
            println!("composition vector {}: {}", v.note, actual_lean);
            let stored = serde_json::to_value(&v.input).unwrap();
            assert_eq!(
                serde_json::from_value::<CompositionIn>(stored.clone()).unwrap(),
                v.input
            );
            let args = Composition::args(&fx, &v.input);
            for (field, number) in stored.as_object().unwrap() {
                assert_eq!(
                    args[field].as_str().unwrap().parse::<u64>().unwrap(),
                    number.as_u64().unwrap(),
                    "decimal input {field}"
                );
            }
        }
        let distinct = vectors
            .iter()
            .find(|v| v.note == "distinct fields and independent times")
            .unwrap();
        assert_eq!(
            distinct.expected,
            json!({"reset":"10000", "settlement":"100000", "spend":"200000"})
        );
    }
}

/// Scalar age quantile; 64 is a campaign budget, not a production bound.
pub struct RingAge;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgeMember {
    #[serde(with = "wire::dec_u64")]
    pub value: u64,
    #[serde(with = "wire::dec_u64")]
    pub creation_height: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RingAgeIn {
    pub members: Vec<AgeMember>,
    #[serde(with = "wire::dec_u64")]
    pub current_height: u64,
    pub quantile_bps: u32,
}

impl Leg for RingAge {
    const NAME: &'static str = "ring_elapsed_quantile";
    const RUST: &'static str = "bth_cluster_tax::ring_elapsed_quantile";
    type Input = RingAgeIn;

    fn strategy() -> BoxedStrategy<Self::Input> {
        let members = vec((gen::u64_edge(), gen::u64_edge()), 0..=64);
        (
            prop_oneof![gen::u64_edge(), 1u64..=2_000_000],
            members,
            0u8..4,
            any::<bool>(),
        )
            .prop_map(|(current_height, raw, layout, ordered)| {
                let equal = raw.first().map_or(0, |m| m.1);
                let mut members: Vec<_> = raw
                    .into_iter()
                    .map(|(value, height)| AgeMember {
                        value,
                        creation_height: match layout {
                            // Full-width independent scalars remain supported.
                            0 => height,
                            // Relative heights produce positive, mixed ages.
                            1 => current_height.saturating_sub(height % 1_000_001),
                            2 => equal,
                            _ => current_height,
                        },
                    })
                    .collect();
                if ordered {
                    members.sort_by_key(|m| current_height.saturating_sub(m.creation_height));
                }
                (current_height, members)
            })
            .prop_flat_map(|(current_height, members)| {
                let n = members.len().max(1) as u32;
                let boundary = (1u32..=n, 0u32..=2).prop_map(move |(rank, delta)| {
                    (rank * 10_000 / n).saturating_sub(1) + delta
                });
                let q = prop_oneof![
                    3 => 1u32..10_000,
                    3 => boundary,
                    2 => proptest::sample::select(vec![0, 1, 3333, 3334, 5000, 6666, 6667, 9999, 10_000, 10_001, u32::MAX]),
                    1 => any::<u32>(),
                ];
                q.prop_map(move |quantile_bps| RingAgeIn {
                    members: members.clone(),
                    current_height,
                    quantile_bps,
                })
            })
            .boxed()
    }

    fn args(_: &Fixtures, i: &Self::Input) -> Value {
        json!({"members":wire::list(&i.members, |m|
            json!({"value":wire::n(m.value), "creation_height":wire::n(m.creation_height)})),
            "current_height":wire::n(i.current_height), "quantile_bps":wire::n(i.quantile_bps)})
    }

    fn rust(_: &Fixtures, i: &Self::Input) -> Value {
        let members: Vec<_> = i
            .members
            .iter()
            .map(|m| (m.value, m.creation_height))
            .collect();
        wire::n(ring_elapsed_quantile(
            &members,
            i.current_height,
            i.quantile_bps,
        ))
    }

    fn classify(i: &Self::Input, r: &Value) -> String {
        let ages: Vec<_> = i
            .members
            .iter()
            .map(|m| i.current_height.saturating_sub(m.creation_height))
            .collect();
        let age = if ages.is_empty() {
            "empty"
        } else if ages.iter().all(|a| *a == 0) {
            "zero"
        } else if ages.iter().all(|a| Some(a) == ages.first()) {
            "equal_positive"
        } else {
            "mixed"
        };
        let q = match i.quantile_bps {
            0 => "zero",
            1..=9999 => "interior",
            10_000 => "maximum",
            u32::MAX => "u32_max",
            _ => "above",
        };
        // Rank is diagnostic only: the adapter above calls the real helper.
        let n = ages.len() as u64;
        let rank = (u64::from(i.quantile_bps.min(10_000)) * n).div_ceil(10_000);
        let rank = if n == 0 {
            "empty"
        } else if rank <= 1 {
            "first"
        } else if rank >= n {
            "last"
        } else {
            "interior"
        };
        let output = if r == &wire::n(0) {
            "zero"
        } else if r == &wire::n(u64::MAX) {
            "u64_max"
        } else {
            "positive"
        };
        format!("len={n};age={age};ordered={};q={q};rank={rank};output={output};height_wide={};height_zero={};creation_zero={};creation_equal={};creation_future={};value_zero={};value_max={}",
            ages.windows(2).all(|w| w[0] <= w[1]), i.current_height > u32::MAX as u64, i.current_height == 0,
            i.members.iter().any(|m| m.creation_height == 0),
            i.members.iter().any(|m| m.creation_height == i.current_height),
            i.members.iter().any(|m| m.creation_height > i.current_height),
            i.members.iter().any(|m| m.value == 0), i.members.iter().any(|m| m.value == u64::MAX))
    }
}

#[cfg(test)]
mod age_tests {
    use super::*;
    use crate::model::Model;
    use std::path::Path;

    #[derive(Deserialize)]
    struct Vector {
        input: RingAgeIn,
        expected: Value,
        note: String,
    }

    #[test]
    fn quantile_live_rust_and_lean_vectors() {
        let vectors: Vec<Vector> = include_str!("../regressions/ring_elapsed_quantile.jsonl")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let fx = Fixtures::new();
        let mut model = Model::spawn(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("lean/.lake/build/bin/botho-model"),
        )
        .expect("build the Lean DRT executable before running DRT tests");
        let mut transformations = 0;
        for v in &vectors {
            let mut inputs = vec![v.input.clone()];
            let ages: Vec<_> = v
                .input
                .members
                .iter()
                .map(|m| v.input.current_height.saturating_sub(m.creation_height))
                .collect();
            if ages.windows(2).any(|w| w[0] != w[1]) {
                for variant in 0..6 {
                    let mut i = v.input.clone();
                    match variant {
                        0 => i.members.reverse(),
                        1 => i.members.rotate_left(1),
                        2 => {
                            let last = i.members.len() - 1;
                            i.members.swap(0, last);
                        }
                        _ => {
                            for (idx, m) in i.members.iter_mut().enumerate() {
                                m.value = match variant {
                                    3 => 0,
                                    4 => u64::MAX,
                                    _ => [0, 1, u64::MAX][idx % 3],
                                };
                            }
                        }
                    }
                    assert_eq!(
                        (i.current_height, i.quantile_bps),
                        (v.input.current_height, v.input.quantile_bps)
                    );
                    inputs.push(i);
                    transformations += 1;
                }
            }
            let requests: Vec<_> = inputs
                .iter()
                .map(|i| wire::request(RingAge::NAME, &RingAge::args(&fx, i)))
                .collect();
            let lean = model.call_batch(&requests);
            assert_eq!(lean.len(), inputs.len());
            for (i, actual_lean) in inputs.iter().zip(lean) {
                assert_eq!(RingAge::rust(&fx, i), v.expected, "Rust: {}", v.note);
                assert_eq!(actual_lean, v.expected, "Lean: {}", v.note);
                let stored = serde_json::to_value(i).unwrap();
                assert_eq!(
                    serde_json::from_value::<RingAgeIn>(stored.clone()).unwrap(),
                    *i
                );
                let args = RingAge::args(&fx, i);
                assert_eq!(args["members"], stored["members"]);
                assert_eq!(args["current_height"], stored["current_height"]);
                assert_eq!(
                    args["quantile_bps"],
                    wire::n(stored["quantile_bps"].as_u64().unwrap())
                );
                assert_eq!(
                    args["current_height"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap(),
                    i.current_height
                );
                for (member, encoded) in i.members.iter().zip(args["members"].as_array().unwrap()) {
                    assert_eq!(
                        encoded["value"].as_str().unwrap().parse::<u64>().unwrap(),
                        member.value
                    );
                    assert_eq!(
                        encoded["creation_height"]
                            .as_str()
                            .unwrap()
                            .parse::<u64>()
                            .unwrap(),
                        member.creation_height
                    );
                }
            }
            println!("quantile vector {}: {}", v.note, v.expected);
        }
        println!(
            "quantile live counts: vectors={} transformations={transformations} implementations=2",
            vectors.len()
        );
        assert_eq!(vectors.len(), 27);
        assert_eq!(transformations, 114);
    }
}

/// Exact weighted age; the 64-member limit bounds test work only.
pub struct ElapsedCentroid;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ElapsedCentroidIn {
    pub members: Vec<AgeMember>,
    #[serde(with = "wire::dec_u64")]
    pub current_height: u64,
}

impl Leg for ElapsedCentroid {
    const NAME: &'static str = "ring_elapsed_centroid";
    const RUST: &'static str = "bth_cluster_tax::ring_elapsed_centroid";
    type Input = ElapsedCentroidIn;

    fn strategy() -> BoxedStrategy<Self::Input> {
        (
            proptest::sample::select(vec![0, 0, 1, 1, 2, 3, 3, 4, 5]),
            gen::u64_edge(),
            vec((gen::u64_edge(), gen::u64_edge()), 0..=64),
        )
            .prop_map(|(layout, height, mut raw)| {
                // High-product classes deliberately exercise multi-limb division.
                if layout == 3 && raw.len() < 2 {
                    raw.resize(2, (0, 0));
                }
                let current_height = match layout {
                    0 | 4 => 1 + height % 1_000_000,
                    3 | 5 => u64::MAX,
                    _ => height,
                };
                let members = raw
                    .into_iter()
                    .enumerate()
                    .map(|(idx, (v, h))| {
                        let (value, creation_height) = match layout {
                            0 => (1 + v % 1_000_000, h % (current_height + 1)),
                            1 => (v, current_height.saturating_sub(h % 1_000_001)),
                            3 => (u64::MAX - v % 1024, h % 1024),
                            4 => (
                                if idx % 3 == 0 { 0 } else { v },
                                current_height.saturating_add(h % 100),
                            ),
                            5 => (v, 0),
                            _ => (v, h),
                        };
                        AgeMember {
                            value,
                            creation_height,
                        }
                    })
                    .collect();
                ElapsedCentroidIn {
                    members,
                    current_height,
                }
            })
            .boxed()
    }

    fn args(_: &Fixtures, i: &Self::Input) -> Value {
        // The stored input and model wire both use decimal strings for every u64.
        serde_json::to_value(i).expect("centroid input")
    }

    fn rust(_: &Fixtures, i: &Self::Input) -> Value {
        let members: Vec<_> = i
            .members
            .iter()
            .map(|m| (m.value, m.creation_height))
            .collect();
        wire::n(ring_elapsed_centroid(&members, i.current_height))
    }

    fn classify(i: &Self::Input, r: &Value) -> String {
        let ages: Vec<_> = i
            .members
            .iter()
            .map(|m| i.current_height.saturating_sub(m.creation_height))
            .collect();
        // Sticky checked overflow is diagnostic only, never the comparison oracle.
        let numerator = i
            .members
            .iter()
            .zip(&ages)
            .try_fold(0u128, |sum, (m, age)| {
                sum.checked_add(u128::from(m.value) * u128::from(*age))
            });
        let denominator: u128 = i.members.iter().map(|m| u128::from(m.value)).sum();
        let output = if r == &wire::n(0) {
            "zero"
        } else if r == &wire::n(u64::MAX) {
            "max"
        } else {
            "positive"
        };
        format!("len={};numerator_exceeds_u128={};denominator_exceeds_u64={};output={output};mixed_ages={};unequal_weights={};height_wide={};zero_weight={};future={};equal_height={}",
            i.members.len(), numerator.is_none(), denominator > u128::from(u64::MAX),
            ages.windows(2).any(|w| w[0] != w[1]), i.members.windows(2).any(|w| w[0].value != w[1].value),
            i.current_height > u32::MAX as u64, i.members.iter().any(|m| m.value == 0),
            i.members.iter().any(|m| m.creation_height > i.current_height),
            i.members.iter().any(|m| m.creation_height == i.current_height))
    }
}

#[cfg(test)]
mod centroid_tests {
    use super::*;
    use crate::model::Model;
    use std::path::Path;

    #[derive(Deserialize)]
    struct Vector {
        input: ElapsedCentroidIn,
        expected: Value,
        note: String,
    }

    #[test]
    fn centroid_live_rust_and_lean_vectors() {
        let vectors: Vec<Vector> = include_str!("../regressions/ring_elapsed_centroid.jsonl")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let fx = Fixtures::new();
        let mut model = Model::spawn(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("lean/.lake/build/bin/botho-model"),
        )
        .expect("build the Lean DRT executable before running DRT tests");
        let (mut permutations, mut zero_insertions, mut scalings) = (0, 0, 0);
        for v in &vectors {
            let mut inputs = vec![v.input.clone()];
            for rotate in [false, true] {
                let mut i = v.input.clone();
                if rotate && !i.members.is_empty() {
                    i.members.rotate_left(1);
                } else {
                    i.members.reverse();
                }
                if !inputs.contains(&i) {
                    inputs.push(i);
                    permutations += 1;
                }
            }
            let mut i = v.input.clone();
            i.members.extend([
                AgeMember {
                    value: 0,
                    creation_height: 0,
                },
                AgeMember {
                    value: 0,
                    creation_height: u64::MAX,
                },
            ]);
            inputs.push(i);
            zero_insertions += 1;
            // Only positive common scaling with every resulting u64 weight fitting.
            if v.input.members.iter().any(|m| m.value > 0)
                && v.input.members.iter().all(|m| m.value <= u64::MAX / 2)
            {
                let mut i = v.input.clone();
                for m in &mut i.members {
                    m.value *= 2;
                }
                inputs.push(i);
                scalings += 1;
            }
            let requests: Vec<_> = inputs
                .iter()
                .map(|i| wire::request(ElapsedCentroid::NAME, &ElapsedCentroid::args(&fx, i)))
                .collect();
            let lean = model.call_batch(&requests);
            assert_eq!(lean.len(), inputs.len());
            for (i, actual_lean) in inputs.iter().zip(lean) {
                assert_eq!(
                    ElapsedCentroid::rust(&fx, i),
                    v.expected,
                    "Rust: {}",
                    v.note
                );
                assert_eq!(actual_lean, v.expected, "Lean: {}", v.note);
                let args = ElapsedCentroid::args(&fx, i);
                assert_eq!(
                    serde_json::from_value::<ElapsedCentroidIn>(args.clone()).unwrap(),
                    *i
                );
                assert_eq!(
                    args["current_height"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap(),
                    i.current_height
                );
                for (member, encoded) in i.members.iter().zip(args["members"].as_array().unwrap()) {
                    assert_eq!(
                        encoded["value"].as_str().unwrap().parse::<u64>().unwrap(),
                        member.value
                    );
                    assert_eq!(
                        encoded["creation_height"]
                            .as_str()
                            .unwrap()
                            .parse::<u64>()
                            .unwrap(),
                        member.creation_height
                    );
                }
                assert_eq!(
                    wire::n(actual_lean.as_str().unwrap().parse::<u64>().unwrap()),
                    v.expected
                );
            }
            println!("centroid vector {}: {}", v.note, v.expected);
        }
        println!("centroid live counts: vectors={} permutations={permutations} zero_insertions={zero_insertions} scalings={scalings} implementations=2", vectors.len());
        assert_eq!(vectors.len(), 20);
        assert!(permutations > 0 && scalings > 0);
        assert_eq!(zero_insertions, 20);
    }
}
