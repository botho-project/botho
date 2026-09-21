//! Inactive funded-payment/public-ticket model. No cryptography or policy
//! activation.
#[path = "../../scripts/research/ct-economics/reference.rs"]
mod reference;
use botho::{
    consensus::lottery::{compute_pool_accounting, reward_cap, LotteryFeeConfig},
    decoy_selection::{GammaDecoySelector, OutputCandidate},
    ledger::{ChainState, Ledger, UtxoSnapshot},
    transaction::{Utxo, UtxoId},
};
use bth_cluster_tax::{count_eligible, draw_winners, LotteryDrawConfig};
use rand::{rngs::StdRng, SeedableRng};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[path = "common/funded_model.rs"]
mod funded_model;
use funded_model::*;

#[test]
fn inherited_keys_preserve_selected_record_age() {
    let mut model = Model::default();
    let old = model.add(0, BTH, 1000, false);
    let payout = model.add(0, BTH, 2000, true);
    model.coins.get_mut(&payout).unwrap().target_key = model.coins[&old].target_key;
    let pool: Vec<_> = [old, payout]
        .iter()
        .map(|id| OutputCandidate::from_utxo(&model.coins[id].utxo(), 3000))
        .collect();
    assert_eq!(pool[0].output.target_key, pool[1].output.target_key);
    for (candidate, expected) in pool.iter().zip([2000, 1000]) {
        let selected = GammaDecoySelector::new()
            .select_decoys_for_input(
                std::slice::from_ref(candidate),
                1,
                &[],
                expected,
                &mut StdRng::seed_from_u64(1323),
            )
            .unwrap();
        assert_eq!(selected_record_age(&selected[0], &pool), expected);
    }
}

fn config() -> Value {
    serde_json::from_slice(
        &std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/research/ct-economics/workload-config.json"),
        )
        .unwrap(),
    )
    .unwrap()
}
fn history(seed: u64, cadence: u64, strategy: &str) -> Value {
    let cfg = config();
    let start = cfg["start_height"].as_u64().unwrap();
    let blocks = cfg["blocks"].as_u64().unwrap();
    assert_eq!(cfg["schema"], 1);
    assert_eq!(cfg["initial_bth_per_owner"], 32);
    assert!(blocks <= 11232 && blocks > 10720);
    assert!(cfg["max_public_outputs"].as_u64().unwrap() <= 5000);
    assert_eq!(cfg["max_eligible_candidates"], 10000);
    assert!((720..=10000).contains(&cfg["refresh_interval"].as_u64().unwrap()));
    assert_eq!(cfg["honest_owners"], 100);
    assert_eq!(cfg["factor"], 1000);
    assert_eq!(cfg["rate_bps"], 200);
    assert_eq!(cfg["bucket_bits"], 2);
    let mut model = Model::default();
    let initial = cfg["initial_bth_per_owner"].as_u64().unwrap() * BTH;
    for owner in 0..100 {
        model.add(owner, initial, start - 1000, false);
    }
    let funding = model.add(ATTACKER, initial, 0, false);
    let mut position = vec![funding];
    let lottery = LotteryDrawConfig::default();
    let fees_cfg = LotteryFeeConfig {
        pool_fraction_permille: 800,
        draw_config: lottery.clone(),
    };
    let (mut reserve, mut burn, mut gross, mut capture, mut awarded, mut expected) =
        (0u128, 0u128, 0u128, 0u128, 0u128, 0f64);
    let (mut honest_attempts, mut honest_success, mut refresh_attempts, mut refresh_success) =
        (0u64, 0u64, 0u64, 0u64);
    let (mut honest_fees, mut attacker_fees) = (0u128, 0u128);
    let mut failures = BTreeMap::<String, u64>::new();
    let mut fee_histogram = BTreeMap::<String, u64>::new();
    let mut snapshots = vec![];
    let (mut max_eligible, mut max_spent_eligible, mut max_payout_eligible, mut cap_blocks) =
        (0usize, 0usize, 0usize, 0u64);
    // Observations only: these counters never select an input, change a fee,
    // fund a transfer, select a winner or advance an RNG.
    let mut owner_fees = [0u128; ATTACKER + 1];
    let mut owner_capture = [0u128; ATTACKER + 1];
    let mut payments_sent = [0u128; ATTACKER + 1];
    let mut payments_received = [0u128; ATTACKER + 1];
    let mut transcript = Sha256::new();
    for offset in 0..blocks {
        let height = start + offset;
        let mut block_fees = 0u64;
        if offset % cadence == 0 {
            honest_attempts += 1;
            let sender = ((offset / cadence) % 100) as usize;
            let recipient = (sender + 1) % 100;
            let cycle = cfg["payment_pico_cycle"].as_array().unwrap();
            let payment = cycle[(offset / cadence) as usize % cycle.len()]
                .as_u64()
                .unwrap();
            let input = model.largest(sender, height);
            let result = model.transfer(
                &input.into_iter().collect::<Vec<_>>(),
                sender,
                recipient,
                Some(payment),
                2,
                height,
                seed,
            );
            match result {
                Ok((fee, _)) => {
                    honest_success += 1;
                    honest_fees += fee as u128;
                    owner_fees[sender] += fee as u128;
                    payments_sent[sender] += payment as u128;
                    payments_received[recipient] += payment as u128;
                    block_fees += fee;
                    *fee_histogram.entry(fee.to_string()).or_default() += 1;
                }
                Err(e) => *failures.entry(format!("honest:{e}")).or_default() += 1,
            }
        }
        let refresh = strategy == "refresh_two"
            && offset > 0
            && offset % cfg["refresh_interval"].as_u64().unwrap() == 0;
        if (offset == 0 && strategy != "idle") || refresh {
            refresh_attempts += 1;
            let count = if strategy == "hold_one" { 1 } else { 2 };
            match model.transfer(&position, ATTACKER, ATTACKER, None, count, height, seed) {
                Ok((fee, ids)) => {
                    refresh_success += 1;
                    position = ids;
                    attacker_fees += fee as u128;
                    owner_fees[ATTACKER] += fee as u128;
                    block_fees += fee;
                }
                Err(e) => *failures.entry(format!("attacker:{e}")).or_default() += 1,
            }
        }
        let mut hash = [0; 32];
        hash[..8].copy_from_slice(&(seed ^ height).to_le_bytes());
        let candidates = model.candidates(height, &hash);
        let rho = count_eligible(&candidates, height, &lottery);
        assert_eq!(rho as usize, candidates.len());
        let spent = candidates
            .iter()
            .filter(|c| model.coins[&c.id].spent)
            .count();
        let payouts = candidates
            .iter()
            .filter(|c| model.coins[&c.id].payout)
            .count();
        let attacker = candidates
            .iter()
            .filter(|c| model.coins[&c.id].owner == ATTACKER)
            .count();
        max_eligible = max_eligible.max(candidates.len());
        max_spent_eligible = max_spent_eligible.max(spent);
        max_payout_eligible = max_payout_eligible.max(payouts);
        let reward = botho::block::calculate_block_reward(height, 0);
        let emission = botho::monetary::mainnet_policy().lottery_emission_share(height, reward);
        assert_eq!(
            emission, 0,
            "this finite non-mining workload assumes zero lottery emission in its chosen heights"
        );
        let cap = reward_cap(&candidates, height, reward, &fees_cfg);
        let accounting = compute_pool_accounting(block_fees, emission, reserve, cap, &fees_cfg);
        if accounting.available > accounting.payout as u128 {
            cap_blocks += 1;
        }
        if rho > 0 {
            expected += accounting.payout as f64 * attacker as f64 / rho as f64;
        }
        let draw = draw_winners(&candidates, accounting.payout, height, &hash, &lottery);
        let mut distributed = 0u64;
        if let Some(draw) = draw {
            for winner in draw.winners {
                let owner = model.coins[&winner.utxo_id].owner;
                distributed += winner.payout;
                owner_capture[owner] += winner.payout as u128;
                if owner == ATTACKER {
                    capture += winner.payout as u128;
                }
                let inherited_key = model.coins[&winner.utxo_id].target_key;
                let id = model.add(owner, winner.payout, height, true);
                model.coins.get_mut(&id).unwrap().target_key = inherited_key;
            }
        }
        assert_eq!(distributed, accounting.payout);
        reserve = accounting.carryover_after(distributed);
        burn += accounting.fee_burn as u128;
        gross += block_fees as u128;
        awarded += distributed as u128;
        assert_eq!(gross, awarded + reserve + burn);
        assert!(model.coins.values().filter(|c| c.payout).all(|c| !c.spent));
        let private_total: u128 = model
            .coins
            .values()
            .filter(|c| !c.spent)
            .map(|c| c.value as u128)
            .sum();
        assert_eq!(private_total + reserve + burn, initial as u128 * 101);
        assert_eq!(gross, honest_fees + attacker_fees);
        assert!(model.coins.len() <= cfg["max_public_outputs"].as_u64().unwrap() as usize);
        transcript.update(height.to_le_bytes());
        transcript.update(block_fees.to_le_bytes());
        transcript.update(distributed.to_le_bytes());
        transcript.update((rho as u64).to_le_bytes());
        transcript.update(model.accounted(ATTACKER).to_le_bytes());
        if [0, 719, 720, 9000, 9001, 10000, 10001, blocks - 1].contains(&offset) {
            snapshots.push(json!({"offset":offset,"eligible":rho,"spent_public_eligible":spent,"payout_eligible":payouts,"attacker_eligible":attacker,"attacker_spendable":model.spendable(ATTACKER).to_string(),"attacker_locked_payouts":model.locked(ATTACKER).to_string()}));
        }
    }
    assert_eq!(
        honest_attempts,
        honest_success
            + failures
                .iter()
                .filter(|(k, _)| k.starts_with("honest:"))
                .map(|(_, n)| n)
                .sum::<u64>()
    );
    assert_eq!(
        refresh_attempts,
        refresh_success
            + failures
                .iter()
                .filter(|(k, _)| k.starts_with("attacker:"))
                .map(|(_, n)| n)
                .sum::<u64>()
    );
    assert_eq!(
        model.accounted(ATTACKER) as i128 - initial as i128,
        capture as i128 - attacker_fees as i128
    );
    let owners: Vec<Value> = (0..=ATTACKER)
        .map(|owner| {
            let spendable = model.spendable(owner);
            let locked = model.locked(owner);
            let accounted = model.accounted(owner);
            assert_eq!(accounted, spendable + locked);
            assert_eq!(locked, owner_capture[owner]);
            assert_eq!(
                accounted + owner_fees[owner] + payments_sent[owner],
                initial as u128 + payments_received[owner] + owner_capture[owner]
            );
            json!({"owner":owner,"fees":owner_fees[owner].to_string(),
            "capture":owner_capture[owner].to_string(),
            "payments_sent":payments_sent[owner].to_string(),
            "payments_received":payments_received[owner].to_string(),
            "spendable":spendable.to_string(),"locked":locked.to_string(),
            "accounted":accounted.to_string()})
        })
        .collect();
    assert_eq!(owner_fees[..ATTACKER].iter().sum::<u128>(), honest_fees);
    assert_eq!(owner_fees[ATTACKER], attacker_fees);
    assert_eq!(owner_capture.iter().sum::<u128>(), awarded);
    assert_eq!(owner_capture[ATTACKER], capture);
    assert_eq!(
        payments_sent.iter().sum::<u128>(),
        payments_received.iter().sum::<u128>()
    );
    json!({"owners":owners,"seed":seed,"honest_cadence":cadence,"strategy":strategy,"honest_attempts":honest_attempts,"honest_success":honest_success,"attacker_attempts":refresh_attempts,"attacker_success":refresh_success,"failures":failures,"honest_fee_histogram":fee_histogram,"honest_fees":honest_fees.to_string(),"attacker_fees":attacker_fees.to_string(),"attacker_capture":capture.to_string(),"conditional_uniform_capture":format!("{expected:.3}"),"attacker_accounted_value":model.accounted(ATTACKER).to_string(),"attacker_spendable":model.spendable(ATTACKER).to_string(),"attacker_locked_payouts":model.locked(ATTACKER).to_string(),"max_eligible":max_eligible,"max_spent_public_eligible":max_spent_eligible,"max_payout_eligible":max_payout_eligible,"public_outputs":model.coins.len(),"honest_accounted_value":(0..100).map(|owner|model.accounted(owner)).sum::<u128>().to_string(),"honest_spendable":(0..100).map(|owner|model.spendable(owner)).sum::<u128>().to_string(),"honest_locked_payouts":(0..100).map(|owner|model.locked(owner)).sum::<u128>().to_string(),"awarded":awarded.to_string(),"burn":burn.to_string(),"reserve":reserve.to_string(),"cap_bound_blocks":cap_blocks,"snapshots":snapshots,"transcript_sha256":hex::encode(transcript.finalize())})
}
#[test]
fn funded_payment_workloads() {
    let cfg = config();
    let mut rows = vec![];
    assert!(
        cfg["seeds"].as_array().unwrap().len()
            * cfg["honest_cadences"].as_array().unwrap().len()
            * cfg["strategies"].as_array().unwrap().len()
            <= 16
    );
    for seed in cfg["seeds"].as_array().unwrap() {
        for cadence in cfg["honest_cadences"].as_array().unwrap() {
            for strategy in cfg["strategies"].as_array().unwrap() {
                rows.push(history(
                    seed.as_u64().unwrap(),
                    cadence.as_u64().unwrap(),
                    strategy.as_str().unwrap(),
                ));
            }
        }
    }
    assert_eq!(rows.len(), 16);
    let replay = history(902, 500, "hold_one");
    assert_eq!(
        rows.iter()
            .find(|r| r["seed"] == 902 && r["honest_cadence"] == 500 && r["strategy"] == "hold_one")
            .unwrap(),
        &replay
    );
    if std::env::var_os("CT_ECONOMICS_WRITE_REPORT").is_some() {
        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/research/ct-economics/workload-results.json"),
            serde_json::to_vec_pretty(&json!({"config":cfg,"rows":rows})).unwrap(),
        )
        .unwrap();
    }
}
#[test]
fn failed_payment_leaves_all_inventory_unchanged() {
    let mut model = Model::default();
    for owner in 0..24 {
        model.add(owner, BTH, 0, false);
    }
    let input = model.largest(0, 20000).unwrap();
    let before = model.coins.clone();
    assert_eq!(
        model.transfer(&[input], 0, 1, Some(BTH), 2, 20000, 1306),
        Err("unaffordable")
    );
    assert_eq!(model.coins, before);
}
#[test]
fn scarce_decoy_failure_leaves_inventory_unchanged() {
    let mut model = Model::default();
    for owner in 0..19 {
        model.add(owner, BTH, 0, false);
    }
    let input = model.largest(0, 20000).unwrap();
    let before = model.coins.clone();
    assert_eq!(
        model.transfer(&[input], 0, 1, Some(BTH / 10), 2, 20000, 902),
        Err("selection_failure")
    );
    assert_eq!(model.coins, before);
}
#[test]
fn payout_claims_are_locked_from_payment_funding() {
    let mut model = Model::default();
    let payout = model.add(0, BTH, 0, true);
    assert_eq!(model.accounted(0), BTH as u128);
    assert_eq!(model.spendable(0), 0);
    assert_eq!(model.locked(0), BTH as u128);
    assert_eq!(model.largest(0, 20000), None);
    let before = model.coins.clone();
    assert_eq!(
        model.transfer(&[payout], 0, 1, Some(BTH / 10), 2, 20000, 1306),
        Err("locked_payout")
    );
    assert_eq!(model.coins, before);
}
#[test]
fn actual_ledger_public_inventory_and_boundary_parity() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let mut model = Model::default();
    let old = model.add(0, BTH, 1000, false);
    model.coins.get_mut(&old).unwrap().spent = true;
    let payout = model.add(0, BTH / 10, 2000, true);
    model.coins.get_mut(&payout).unwrap().target_key = model.coins[&old].target_key;
    assert_eq!(
        model.coins[&payout].utxo().output.target_key,
        model.coins[&old].utxo().output.target_key
    );
    assert_ne!(old, payout);
    let image = [9; 32];
    // Actual snapshot loader and candidate API; this is NOT a signed-spend or
    // accepted-block rehearsal. The private spent oracle is deliberately absent
    // from the public snapshot except for an unlinkable key-image record.
    for height in [1719, 1720, 2719, 2720, 11000, 11001, 12000, 12001] {
        let present: Vec<_> = model
            .coins
            .values()
            .filter(|c| c.created <= height)
            .map(Coin::utxo)
            .collect();
        let snapshot = UtxoSnapshot::new(
            height,
            [3; 32],
            ChainState {
                height,
                ..Default::default()
            },
            present,
            vec![(image, 1500)],
            vec![],
        )
        .unwrap();
        ledger
            .load_from_snapshot(&snapshot, Some(&[3; 32]))
            .unwrap();
        assert_eq!(ledger.is_key_image_spent(&image).unwrap(), Some(1500));
        for hash in [[1; 32], [2; 32]] {
            let actual = ledger
                .get_lottery_validation_candidates(height, &hash, &LotteryDrawConfig::default())
                .unwrap();
            let expected = model.candidates(height, &hash);
            assert_eq!(
                actual.iter().map(|c| c.id).collect::<Vec<_>>(),
                expected.iter().map(|c| c.id).collect::<Vec<_>>()
            );
            if height == 1719 {
                assert!(!actual.iter().any(|c| c.id == old));
            }
            if height == 1720 {
                assert!(actual.iter().any(|c| c.id == old));
            }
            if height == 2719 {
                assert!(!actual.iter().any(|c| c.id == payout));
            }
            if height == 2720 {
                assert!(actual.iter().any(|c| c.id == payout));
            }
            if height == 11000 {
                assert!(actual.iter().any(|c| c.id == old));
            }
            if height == 11001 {
                assert!(!actual.iter().any(|c| c.id == old));
            }
            if height == 12000 {
                assert!(actual.iter().any(|c| c.id == payout));
            }
            if height == 12001 {
                assert!(actual.is_empty());
            }
        }
    }
    assert_eq!(model.accounted(0), (BTH / 10) as u128);
    assert!(ledger
        .get_utxo(&model.coins[&old].utxo().id)
        .unwrap()
        .is_some());
    assert!(ledger
        .get_utxo(&model.coins[&payout].utxo().id)
        .unwrap()
        .is_some());
}
