//! Inactive proposed policy history, source-review draft. No production caller.
#[path = "common/funded_model.rs"]
mod funded_model;
#[path = "../../scripts/research/payout-policy/policy.rs"]
mod policy;
#[path = "../../scripts/research/ct-economics/reference.rs"]
mod reference;
#[path = "common/reinvestment_model.rs"]
mod reinvestment_model;
use botho::consensus::lottery::{compute_pool_accounting, reward_cap, LotteryFeeConfig};
use bth_cluster_tax::{count_eligible, draw_winners, LotteryDrawConfig};
use funded_model::{Model, ATTACKER};
use policy::{decide, Policy};
use serde_json::{json, Value};
fn policy_awards(
    model: &mut Model,
    height: u64,
    seed: u64,
    fees: u64,
    reserve: u128,
    mode: &str,
    policy: Policy,
    observe: &mut impl FnMut(Value),
) -> (
    u128,
    u128,
    u128,
    Vec<(usize, u64)>,
    usize,
    usize,
    usize,
    f64,
) {
    let mut hash = [0; 32];
    hash[..8].copy_from_slice(&(seed ^ height).to_le_bytes());
    let mut cfg = LotteryDrawConfig::default();
    let candidates = model.candidates(height, &hash);
    let eligible = count_eligible(&candidates, height, &cfg) as usize;
    assert_eq!(eligible, candidates.len());
    let spent = candidates
        .iter()
        .filter(|c| model.coins[&c.id].spent)
        .count();
    let payout = candidates
        .iter()
        .filter(|c| model.coins[&c.id].payout)
        .count();
    let designated = candidates
        .iter()
        .filter(|c| model.coins[&c.id].owner == ATTACKER)
        .count();
    let reward = botho::block::calculate_block_reward(height, 0);
    let emission = botho::monetary::mainnet_policy().lottery_emission_share(height, reward);
    assert_eq!(emission, 0);
    let fees_cfg = LotteryFeeConfig {
        pool_fraction_permille: 800,
        draw_config: cfg.clone(),
    };
    let cap = reward_cap(&candidates, height, reward, &fees_cfg);
    let accounting = compute_pool_accounting(fees, emission, reserve, cap, &fees_cfg);
    let decision = decide(policy, accounting.available, cap, eligible);
    cfg.winners_per_draw = decision.winners;
    let draw = if decision.winners == 0 {
        None
    } else {
        draw_winners(&candidates, decision.distribution, height, &hash, &cfg)
    };
    let mut award_rows = vec![];
    if eligible > 0 && decision.distribution > 0 {
        assert!(draw.is_some());
    }
    let mut distributed = 0u64;
    let mut receipts = vec![];
    if let Some(draw) = draw {
        for winner in draw.winners {
            let source = &model.coins[&winner.utxo_id];
            let (owner, inherited) = (source.owner, source.target_key);
            let id = model.add(owner, winner.payout, height, true);
            if mode == "legacy_locked" {
                model.coins.get_mut(&id).unwrap().target_key = inherited;
            }
            // Other modes retain distinct synthetic IDs/keys, NOT V2 derivation.
            distributed = distributed.checked_add(winner.payout).unwrap();
            receipts.push((owner, winner.payout));
            award_rows.push(
                json!({"id":hex::encode(winner.utxo_id),"owner":owner,"value":winner.payout}),
            );
        }
    }
    assert_eq!(distributed, decision.distribution);
    assert_eq!(accounting.carryover_after(distributed), decision.reserve);
    let mut cohort_expiry = std::collections::BTreeMap::<u64, usize>::new();
    if fees > 0 {
        for candidate in &candidates {
            *cohort_expiry
                .entry(candidate.creation_block + 10001)
                .or_default() += 1;
        }
    }
    observe(
        json!({"height":height,"policy":match policy {Policy::Baseline=>"baseline",Policy::ThresholdReserve=>"threshold_reserve",Policy::AdaptiveCount=>"adaptive_count"},
        "fees":fees,"emission":emission,"available":accounting.available,"cap":cap,"block_reward":reward,
        "eligible":eligible,"burn":accounting.fee_burn,"distribution":distributed,"reserve":decision.reserve,
        "reason":decision.reason,"awards":award_rows,"cohort_expiry":cohort_expiry,
        "expiring_next_block":candidates.iter().filter(|c|height-c.creation_block==10000).count()}),
    );
    let expected = if eligible == 0 {
        0.0
    } else {
        decision.distribution as f64 * designated as f64 / eligible as f64
    };
    (
        accounting.carryover_after(distributed),
        accounting.fee_burn as u128,
        distributed as u128,
        receipts,
        eligible,
        spent,
        payout,
        expected,
    )
}

#[test]
fn fixed_six_histories_source_review_draft() {
    let baseline: Value = serde_json::from_slice(
        &std::fs::read(
            std::env::var_os("PAYOUT_BASELINE_JSON")
                .expect("decompressed archived PR1370 raw required"),
        )
        .unwrap(),
    )
    .unwrap();
    let config: Value = serde_json::from_str(include_str!(
        "../../scripts/research/payout-policy/config.json"
    ))
    .unwrap();
    let old_config = &baseline["config"];
    for key in [
        "seeds",
        "start_height",
        "blocks",
        "honest_owners",
        "owners",
        "initial_bth_per_owner",
        "honest_cadence",
        "consolidation_cadence",
        "max_award_inputs",
        "payment_pico_cycle",
        "factor",
        "rate_bps",
        "bucket_bits",
        "compile_seconds",
        "matrix_seconds",
    ] {
        assert_eq!(
            config[key], old_config[key],
            "fixed inherited config: {key}"
        );
    }
    assert_eq!(config["schema"], 3);
    assert_eq!(config["threshold_pico"], policy::THRESHOLD);
    assert_eq!(config["baseline_mode"], "candidate_age720");
    assert_eq!(config["replays"], 1);
    assert_eq!(
        config["policies"],
        json!(["baseline", "threshold_reserve", "adaptive_count"])
    );
    let mut rows = vec![];
    for seed in [1306, 902] {
        for policy in [
            Policy::Baseline,
            Policy::ThresholdReserve,
            Policy::AdaptiveCount,
        ] {
            let mut blocks = vec![];
            let row = reinvestment_model::history_with(
                seed,
                "candidate_age720",
                |m, h, s, f, r, mode| {
                    policy_awards(m, h, s, f, r, mode, policy, &mut |v| blocks.push(v))
                },
            );
            if policy == Policy::Baseline {
                let old = baseline["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| r["seed"] == seed && r["mode"] == "candidate_age720")
                    .unwrap();
                assert_eq!(
                    serde_json::to_vec(&row).unwrap(),
                    serde_json::to_vec(old).unwrap()
                );
            }
            rows.push(json!({"row":row,"blocks":blocks}));
        }
    }
    let replay = reinvestment_model::history(1306, "candidate_age720");
    assert_eq!(replay, rows[0]["row"]);
    std::fs::write(
        std::env::var_os("PAYOUT_POLICY_OUTPUT").expect("collector output required"),
        serde_json::to_vec(&json!({"schema":3,"config":config,"rows":rows})).unwrap(),
    )
    .unwrap();
}
