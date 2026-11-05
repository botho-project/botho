//! Production monetary functions against the landed #1921 Lean model.
//! A is the arithmetic-safe supply domain; B is sufficient for lossless
//! narrowing, while Z is the actual first cast-loss input. See MONETARY.md.

use crate::{
    fixtures::Fixtures,
    gen,
    legs::Leg,
    wire::{self, n},
};
use proptest::prelude::*;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

const H: u64 = 6_307_200;
const A: u128 = u128::MAX / 200;
const B: u128 = u64::MAX as u128 * H as u128 * 40;
const Z: u128 = (1u128 << 64) * H as u128 * 40;

fn safe_supply<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
    let s = String::deserialize(d)?;
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(serde::de::Error::custom("supply must be a decimal string"));
    }
    let value: u128 = s.parse().map_err(serde::de::Error::custom)?;
    if value > A {
        return Err(serde::de::Error::custom(
            "supply exceeds monetary arithmetic-safe domain A",
        ));
    }
    Ok(value)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    #[serde(with = "wire::dec_u64")]
    height: u64,
    #[serde(with = "wire::dec_u64")]
    reward: u64,
    #[serde(with = "wire::dec_u64")]
    wrapper_supply: u64,
    #[serde(
        serialize_with = "wire::dec_u128::serialize",
        deserialize_with = "safe_supply"
    )]
    supply: u128,
}

fn heights() -> Vec<u64> {
    let mut out = vec![0, 1, u64::MAX - 1, u64::MAX];
    for k in 1..=5 {
        out.extend([k * H - 1, k * H, k * H + 1]);
    }
    // The lottery epoch's u32 clamp is independent of the phase boundary.
    let clamp = u32::MAX as u64 * H;
    out.extend([clamp - 1, clamp, clamp + H]);
    out
}

fn supplies() -> Vec<u128> {
    let unit = H as u128 * 40;
    vec![
        0,
        1,
        49,
        50,
        51,
        199,
        200,
        201,
        unit - 1,
        unit,
        unit + 1,
        2 * unit - 1,
        2 * unit,
        2 * unit + 1,
        (1u128 << 53) + 1,
        u64::MAX as u128,
        B - 1,
        B,
        B + 1,
        Z - 1,
        Z,
        Z + 1,
        A - 1,
        A,
    ]
}

pub struct Monetary;

impl Leg for Monetary {
    const NAME: &'static str = "monetary";
    const RUST: &'static str = "bth_cluster_tax::MonetaryPolicy::{is_halving_phase,halving_reward,calculate_tail_reward_u128,calculate_tail_reward,block_reward_at,lottery_emission_bps,lottery_emission_share,demurrage_rate_bps}; botho::block::calculate_block_reward";
    type Input = Input;

    fn strategy() -> BoxedStrategy<Input> {
        let supply = prop_oneof![
            4 => 0u128..=A,
            3 => 0u128..=B,
            2 => B + 1..Z,
            4 => prop::sample::select(supplies()),
            // Annual divisions change at multiples of 50 and 200.
            2 => (0u128..A / 200, prop::sample::select(vec![0u128, 1, 49, 50, 51, 199]))
                .prop_map(|(q, r)| q * 200 + r),
            // Per-block division remainders, without clamping generated inputs.
            2 => (1u128..A / (H as u128 * 40), 0u128..=2)
                .prop_map(|(q, d)| q * (H as u128 * 40) + d - 1),
        ];
        (
            prop_oneof![3 => prop::sample::select(heights()), 2 => 0u64..5 * H, 2 => any::<u64>()],
            gen::u64_edge(),
            gen::u64_edge(),
            supply,
        )
            .prop_map(|(height, reward, wrapper_supply, supply)| Input {
                height,
                reward,
                wrapper_supply,
                supply,
            })
            .boxed()
    }

    fn args(_: &Fixtures, i: &Input) -> Value {
        serde_json::to_value(i).expect("monetary input serialization")
    }

    fn rust(_: &Fixtures, i: &Input) -> Value {
        // Public production constructors; no fast-test policy or formula copy.
        let p = bth_cluster_tax::monetary::mainnet_policy_mirror();
        json!({
            "halving_phase": p.is_halving_phase(i.height),
            "halving_reward": p.halving_reward(i.height).map(n),
            "tail_u128": n(p.calculate_tail_reward_u128(i.supply)),
            "tail_u64": n(p.calculate_tail_reward(i.wrapper_supply)),
            "library_reward": n(p.block_reward_at(i.height, i.supply)),
            "node_reward": n(botho::block::calculate_block_reward(i.height, i.supply)),
            "lottery_bps": n(p.lottery_emission_bps(i.height)),
            "lottery_share": n(p.lottery_emission_share(i.height, i.reward)),
            "demurrage_bps": n(p.demurrage_rate_bps(i.height)),
        })
    }

    fn classify(i: &Input, r: &Value) -> String {
        let domain = if i.supply <= B {
            if r["tail_u128"] == "1" {
                "lossless_floor"
            } else {
                "lossless_within_B"
            }
        } else if i.supply < Z {
            "lossless_above_B"
        } else if r["tail_u128"] == "0" {
            "cast_loss_zero"
        } else {
            "cast_loss_nonzero"
        };
        let phase = if i.height < 5 * H { "halving" } else { "tail" };
        let edge = if heights().contains(&i.height) || supplies().contains(&i.supply) {
            "boundary"
        } else {
            "interior"
        };
        format!("{phase}/{domain}/{edge}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{strategy::ValueTree, test_runner::TestRunner};

    #[test]
    fn monetary_fixture_precision_and_domain_rejection() {
        for supply in [0, (1u128 << 53) + 1, u64::MAX as u128, A, B, Z] {
            let input = Input {
                height: u64::MAX,
                reward: (1 << 53) + 1,
                wrapper_supply: u64::MAX,
                supply,
            };
            let v = serde_json::to_value(&input).unwrap();
            assert!(v.as_object().unwrap().values().all(Value::is_string));
            assert_eq!(serde_json::from_value::<Input>(v.clone()).unwrap(), input);
            for field in ["height", "reward", "wrapper_supply", "supply"] {
                for bad in [
                    json!(1),
                    json!(""),
                    json!("-1"),
                    json!("+1"),
                    json!("1.5"),
                    json!(" 1"),
                ] {
                    let mut malformed = v.clone();
                    malformed[field] = bad;
                    assert!(serde_json::from_value::<Input>(malformed).is_err());
                }
            }
            for (field, bad) in [
                ("height", (u64::MAX as u128 + 1).to_string()),
                ("reward", (u64::MAX as u128 + 1).to_string()),
                ("wrapper_supply", (u64::MAX as u128 + 1).to_string()),
                ("supply", (A + 1).to_string()),
                ("supply", u128::MAX.to_string()),
                (
                    "supply",
                    "340282366920938463463374607431768211456".to_string(),
                ),
            ] {
                let mut invalid = v.clone();
                invalid[field] = n(bad);
                assert!(serde_json::from_value::<Input>(invalid).is_err());
            }
        }
    }

    #[test]
    fn monetary_strategy_shrinks_inside_domain() {
        let mut runner = TestRunner::deterministic();
        let mut wide = false;
        let mut independent = false;
        for _ in 0..256 {
            let mut tree = Monetary::strategy().new_tree(&mut runner).unwrap();
            wide |= tree.current().supply > u64::MAX as u128;
            independent |= tree.current().supply != tree.current().wrapper_supply as u128;
            loop {
                let i = tree.current();
                assert!(i.supply <= A);
                assert_eq!(
                    serde_json::from_value::<Input>(serde_json::to_value(&i).unwrap()).unwrap(),
                    i
                );
                if !tree.simplify() {
                    break;
                }
            }
        }
        assert!(wide && independent);
    }

    #[test]
    fn monetary_classification_distinguishes_bound_from_cast_loss() {
        let p = bth_cluster_tax::monetary::mainnet_policy_mirror();
        for (supply, class) in [
            (0, "lossless_floor"),
            (B, "lossless_within_B"),
            (B + 1, "lossless_above_B"),
            (Z - 1, "lossless_above_B"),
            (Z, "cast_loss_zero"),
            (A, "cast_loss_nonzero"),
        ] {
            let i = Input {
                height: 5 * H,
                reward: 1,
                wrapper_supply: 0,
                supply,
            };
            let r = json!({"tail_u128": n(p.calculate_tail_reward_u128(supply))});
            assert_eq!(Monetary::classify(&i, &r), format!("tail/{class}/boundary"));
        }
    }
}
