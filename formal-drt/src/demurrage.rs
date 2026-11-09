//! DRT for the #1711 demurrage models against actual cluster-tax
//! functions. Since #1875 (protocol 9.0.0) the `demurrage_charge` leg runs
//! the exact model `Botho.Demurrage.exactCharge`; the legacy quantized model
//! `Botho.Demurrage.charge` keeps its counterexample theorems.
use crate::{fixtures::Fixtures, gen, legs::Leg, wire};
use bth_cluster_tax::{
    capitalized_reset_charge, demurrage_charge, demurrage_settlement_charge,
    ring_centroid_implied_factor, spend_demurrage_charge, ClusterFactorCurve,
    SETTLEMENT_HORIZON_BLOCKS,
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
