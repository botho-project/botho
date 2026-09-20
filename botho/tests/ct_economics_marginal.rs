//! Inactive fixed-capital controls; no production policy or sampler changes.
use botho::consensus::lottery::{compute_pool_accounting, reward_cap, LotteryFeeConfig};
use bth_cluster_tax::{
    count_eligible, draw_winners, LotteryCandidate, LotteryDrawConfig, TagVector,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
const BTH: u64 = 1_000_000_000_000;
const FEE: u64 = BTH / 4;

fn allocation(budget: u64, count: u64, minimum: u64) -> Option<(u64, u64, u64)> {
    if count == 0 {
        return Some((0, 0, budget));
    }
    let fees = count.checked_mul(FEE)?;
    let available = budget.checked_sub(fees)?;
    let each = available / count;
    (each >= minimum).then_some((fees, each, available % count))
}

#[test]
fn allocation_boundaries_charge_fees_once() {
    assert_eq!(allocation(2 * BTH, 16, 1_000_000), None);
    assert_eq!(allocation(FEE + 999_999, 1, 1_000_000), None);
    assert_eq!(
        allocation(FEE + 1_000_000, 1, 1_000_000),
        Some((FEE, 1_000_000, 0))
    );
    assert_eq!(
        allocation(2 * FEE + 2_000_001, 2, 1_000_000),
        Some((2 * FEE, 1_000_000, 1))
    );
    assert_eq!(allocation(2 * BTH, 0, 1_000_000), Some((0, 0, 2 * BTH)));
}

fn history(budget: u64, count: u64, seed: u64, honest_fee: u64) -> Value {
    let config = LotteryDrawConfig::default();
    assert_eq!(config.min_utxo_age, 720);
    let fee_config = LotteryFeeConfig {
        pool_fraction_permille: 800,
        draw_config: config.clone(),
    };
    let allocated = allocation(budget, count, config.min_utxo_value);
    let (fees, each, cash) = allocated.unwrap_or((0, 0, budget));
    let active_count = if allocated.is_some() { count } else { 0 };
    let mut candidates = Vec::new();
    let mut attacker_ids = BTreeSet::new();
    for i in 1u64..=100 + active_count {
        let mut id = [0; 36];
        id[..8].copy_from_slice(&i.to_le_bytes());
        let attacker = i > 100;
        if attacker {
            attacker_ids.insert(id);
        }
        candidates.push(LotteryCandidate::new(
            id,
            if attacker { each } else { BTH },
            1000,
            &TagVector::new(),
            if attacker { 1000 } else { 0 },
        ));
    }
    let (mut reserve, mut burn, mut awarded, mut capture, mut warmup_capture, mut emitted) =
        (0u128, 0u128, 0u128, 0u128, 0u128, 0u128);
    let mut expected = 0f64;
    let mut capped_blocks = 0;
    let mut measured_awarded = 0u128;
    for offset in 0..720 + 512 {
        let height = 1000 + offset;
        let creation_fee = if offset == 0 { fees } else { 0 };
        let reward = botho::block::calculate_block_reward(height, 0);
        let emission = botho::monetary::mainnet_policy().lottery_emission_share(height, reward);
        let cap = reward_cap(&candidates, height, reward, &fee_config);
        let accounting = compute_pool_accounting(
            creation_fee + honest_fee,
            emission,
            reserve,
            cap,
            &fee_config,
        );
        burn += accounting.fee_burn as u128;
        emitted += emission as u128;
        if accounting.available > accounting.payout as u128 {
            capped_blocks += 1;
        }
        let mut hash = [0; 32];
        hash[..8].copy_from_slice(&(seed ^ height).to_le_bytes());
        let rho = count_eligible(&candidates, height, &config);
        assert!(rho >= 100);
        let winners = match draw_winners(&candidates, accounting.payout, height, &hash, &config) {
            Some(draw) => {
                assert!(accounting.payout > 0);
                draw.winners
            }
            None => {
                assert_eq!(accounting.payout, 0);
                Vec::new()
            }
        };
        let distributed: u64 = winners.iter().map(|w| w.payout).sum();
        assert_eq!(distributed, accounting.payout);
        let won: u128 = winners
            .iter()
            .filter(|w| attacker_ids.contains(&w.utxo_id))
            .map(|w| w.payout as u128)
            .sum();
        if offset < 720 {
            warmup_capture += won;
            assert_eq!(won, 0);
        } else {
            measured_awarded += distributed as u128;
            expected += distributed as f64 * active_count as f64 / rho as f64;
        }
        capture += won;
        awarded += distributed as u128;
        reserve = accounting.carryover_after(distributed);
        assert_eq!(
            fees as u128 + honest_fee as u128 * (offset + 1) as u128 + emitted,
            awarded + reserve + burn
        );
        // Gross fees are already deducted from principal; burn is not a second charge.
        assert_eq!(budget, fees + each * active_count + cash);
        let honest_fee_budget = honest_fee as u128 * 1232;
        let honest_cash_left = honest_fee_budget - honest_fee as u128 * (offset + 1) as u128;
        assert_eq!(
            budget as u128 + 100 * BTH as u128 + honest_fee_budget + emitted,
            each as u128 * active_count as u128
                + cash as u128
                + capture
                + 100 * BTH as u128
                + honest_cash_left
                + (awarded - capture)
                + reserve
                + burn
        );
    }
    let final_wealth = each as u128 * active_count as u128 + cash as u128 + capture;
    assert_eq!(
        final_wealth as i128 - budget as i128,
        capture as i128 - fees as i128
    );
    json!({"budget":budget.to_string(),"outputs":count,"seed":seed,"honest_fee_per_block":honest_fee.to_string(),"status":if allocated.is_some(){"funded"}else{"unaffordable"},"gross_creation_fee":fees.to_string(),"requested_creation_fee":(count*FEE).to_string(),"principal":(each*active_count).to_string(),"old_cash":cash.to_string(),"warmup_capture":warmup_capture.to_string(),"measured_capture":capture.to_string(),"final_wealth":final_wealth.to_string(),"delta_idle":(final_wealth as i128-budget as i128).to_string(),"expected_uniform_capture":format!("{expected:.3}"),"honest_paid":(honest_fee as u128*1232).to_string(),"system_burn":burn.to_string(),"system_awarded":awarded.to_string(),"measured_system_awarded":measured_awarded.to_string(),"reserve":reserve.to_string(),"emission":emitted.to_string(),"cap_bound_blocks":capped_blocks})
}

#[test]
fn fixed_capital_paired_histories() {
    let mut rows = Vec::new();
    for budget in [2 * BTH, 32 * BTH] {
        for honest_fee in [0, 10 * FEE] {
            for seed in [1306, 902] {
                let baseline = history(budget, 1, seed, honest_fee);
                let baseline_wealth: i128 =
                    baseline["final_wealth"].as_str().unwrap().parse().unwrap();
                for count in [0, 1, 2, 16] {
                    let mut row = history(budget, count, seed, honest_fee);
                    let wealth: i128 = row["final_wealth"].as_str().unwrap().parse().unwrap();
                    row["delta_unsplit"] = json!((wealth - baseline_wealth).to_string());
                    let expected: f64 = row["expected_uniform_capture"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap();
                    let baseline_expected: f64 = baseline["expected_uniform_capture"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap();
                    let fee: f64 = row["gross_creation_fee"].as_str().unwrap().parse().unwrap();
                    row["expected_uniform_delta_unsplit"] = json!(format!(
                        "{:.3}",
                        expected - fee - (baseline_expected - FEE as f64)
                    ));
                    if count == 0 {
                        assert_eq!(row["delta_idle"], "0");
                    }
                    if count == 1 {
                        assert_eq!(row["delta_unsplit"], "0");
                    }
                    if honest_fee == 0 {
                        assert_eq!(row["measured_capture"], "0");
                    }
                    rows.push(row);
                }
            }
        }
    }
    assert_eq!(rows.len(), 32);
    assert_eq!(
        rows.iter()
            .filter(|r| r["status"] == "unaffordable")
            .count(),
        4
    );
    if std::env::var_os("CT_ECONOMICS_WRITE_REPORT").is_some() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/research/ct-economics/marginal.json");
        std::fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({"warmup_blocks":720,"measured_blocks":512,"total_blocks":1232,"rows":rows}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}
