//! DRT for the #1711 counterexample models against actual cluster-tax
//! functions.
use crate::{fixtures::Fixtures, gen, legs::Leg, wire};
use bth_cluster_tax::{demurrage_charge, ring_centroid_implied_factor, ClusterFactorCurve};
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
            prop_oneof![3 => Just(6_307_200u64), 1 => gen::u64_edge()],
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
            "positive inputs, six-block dead zone".into()
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
            "weighted numerator saturates"
        } else {
            "weighted numerator exact"
        }
        .into()
    }
}
