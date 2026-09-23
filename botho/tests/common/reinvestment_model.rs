//! Shared inactive history helpers; no tests are registered here.
use super::{
    funded_model::{Model, ATTACKER, BTH},
    reference,
};
use botho::consensus::lottery::{compute_pool_accounting, reward_cap, LotteryFeeConfig};
use bth_cluster_tax::{count_eligible, draw_winners, LotteryDrawConfig};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) type AwardResult = (
    u128,
    u128,
    u128,
    Vec<(usize, u64)>,
    usize,
    usize,
    usize,
    f64,
);
const OWNERS: usize = ATTACKER + 1;
const INITIAL: u128 = 32 * BTH as u128;
#[derive(Clone, Default)]
struct Account {
    fees: u128,
    capture: u128,
    sent: u128,
    received: u128,
    consumed: u128,
    attempts: u64,
    success: u64,
    failures: BTreeMap<String, u64>,
}
fn policy(mode: &str) -> Option<u64> {
    match mode {
        "legacy_locked" | "candidate_locked" => None,
        "candidate_age720" => Some(720),
        "candidate_age10001" => Some(10001),
        _ => panic!("unknown policy"),
    }
}
pub(crate) fn award_inputs(
    model: &Model,
    owner: usize,
    height: u64,
    minimum_age: u64,
) -> Vec<[u8; 36]> {
    let mut inputs: Vec<_> = model
        .coins
        .values()
        .filter(|c| {
            c.owner == owner
                && c.payout
                && !c.spent
                && height.saturating_sub(c.created) >= minimum_age
        })
        .collect();
    inputs.sort_by_key(|c| (c.created, c.id));
    inputs.into_iter().take(16).map(|c| c.id).collect()
}
fn fail(map: &mut BTreeMap<String, u64>, reason: &str) {
    *map.entry(reason.into()).or_default() += 1;
}
pub(crate) fn awards(
    model: &mut Model,
    height: u64,
    seed: u64,
    fees: u64,
    reserve: u128,
    mode: &str,
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
    let cfg = LotteryDrawConfig::default();
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
    let draw = draw_winners(&candidates, accounting.payout, height, &hash, &cfg);
    if eligible > 0 && accounting.payout > 0 {
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
        }
    }
    assert_eq!(distributed, accounting.payout);
    let expected = if eligible == 0 {
        0.0
    } else {
        accounting.payout as f64 * designated as f64 / eligible as f64
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
pub(crate) fn history(seed: u64, mode: &str) -> Value {
    history_with(seed, mode, awards)
}
pub(crate) fn history_with(
    seed: u64,
    mode: &str,
    mut draw: impl FnMut(&mut Model, u64, u64, u64, u128, &str) -> AwardResult,
) -> Value {
    let mut model = Model::default();
    for owner in 0..ATTACKER {
        model.add(owner, INITIAL as u64, 19000, false);
    }
    model.add(ATTACKER, INITIAL as u64, 0, false);
    let mut accounts = vec![Account::default(); OWNERS];
    let mut recycled = BTreeSet::new();
    let mut receipts = vec![];
    let mut payment_failures = BTreeMap::new();
    let (mut payment_attempts, mut payment_success) = (0u64, 0u64);
    let (mut opportunities, mut attempts, mut success) = (0u64, 0u64, 0u64);
    let (mut reserve, mut burned, mut gross, mut captured) = (0u128, 0u128, 0u128, 0u128);
    let mut expected = 0.0;
    let (mut max_eligible, mut max_spent, mut max_award, mut max_recycled) = (0, 0, 0, 0);
    let mut snapshots = vec![];
    for offset in 0..11232u64 {
        let height = 20000 + offset;
        let mut block_fees = 0u64;
        if offset % 100 == 0 {
            let sender = (payment_attempts as usize) % ATTACKER;
            let recipient = (sender + 1) % ATTACKER;
            let amount = [BTH / 10, BTH, 10 * BTH][payment_attempts as usize % 3];
            payment_attempts += 1;
            let result = if let Some(id) = model.largest(sender, height) {
                model.transfer(&[id], sender, recipient, Some(amount), 2, height, seed)
            } else {
                Err("no_private_input")
            };
            match result {
                Ok((fee, _)) => {
                    payment_success += 1;
                    accounts[sender].fees += fee as u128;
                    accounts[sender].sent += amount as u128;
                    accounts[recipient].received += amount as u128;
                    block_fees = block_fees.checked_add(fee).unwrap();
                }
                Err(reason) => fail(&mut payment_failures, reason),
            }
            for owner in 0..OWNERS {
                opportunities += 1;
                let Some(age) = policy(mode) else { continue };
                attempts += 1;
                accounts[owner].attempts += 1;
                let ids = award_inputs(&model, owner, height, age);
                let total: u128 = ids.iter().map(|id| model.coins[id].value as u128).sum();
                match model.transfer_with_award_policy(
                    &ids,
                    owner,
                    owner,
                    None,
                    1,
                    height,
                    seed,
                    Some(age),
                ) {
                    Ok((fee, outputs)) => {
                        assert_eq!(outputs.len(), 1);
                        assert_eq!(model.coins[&outputs[0]].value as u128 + fee as u128, total);
                        success += 1;
                        accounts[owner].success += 1;
                        accounts[owner].fees += fee as u128;
                        accounts[owner].consumed += total;
                        block_fees = block_fees.checked_add(fee).unwrap();
                        recycled.insert(outputs[0]);
                        receipts.push(json!({"height":height,"owner":owner,"inputs":ids.iter().map(hex::encode).collect::<Vec<_>>(),
                          "consumed":total.to_string(),"fee":fee.to_string(),"output":hex::encode(outputs[0]),
                          "value":model.coins[&outputs[0]].value.to_string()}));
                    }
                    Err(reason) => fail(&mut accounts[owner].failures, reason),
                }
            }
        }
        let (pool, burn, distribution, awarded, eligible, spent, payout, conditional) =
            draw(&mut model, height, seed, block_fees, reserve, mode);
        reserve = pool;
        burned += burn;
        captured += distribution;
        gross += block_fees as u128;
        expected += conditional;
        for (owner, value) in awarded {
            accounts[owner].capture += value as u128;
        }
        assert_eq!(gross, captured + reserve + burned);
        assert_eq!(
            model
                .coins
                .values()
                .filter(|c| !c.spent)
                .map(|c| c.value as u128)
                .sum::<u128>()
                + reserve
                + burned,
            INITIAL * OWNERS as u128
        );
        assert!(model.coins.len() <= 5000);
        if mode == "legacy_locked" || mode == "candidate_locked" {
            assert!(model.coins.values().filter(|c| c.payout).all(|c| !c.spent));
        }
        max_eligible = max_eligible.max(eligible);
        max_spent = max_spent.max(spent);
        max_award = max_award.max(payout);
        let eligible_recycled = recycled
            .iter()
            .filter(|id| {
                model.coins[*id]
                    .candidate()
                    .is_eligible(height, &LotteryDrawConfig::default())
            })
            .count();
        max_recycled = max_recycled.max(eligible_recycled);
        if [0, 719, 720, 10000, 10001, 11231].contains(&offset) {
            snapshots.push(json!({"height":height,"eligible":eligible,"spent_public_eligible":spent,"award_eligible":payout,"recycled_eligible":eligible_recycled}));
        }
    }
    let owners:Vec<_>=(0..OWNERS).map(|owner|{
        let a=&accounts[owner];let mut ordinary=0u128;let mut unspent=0u128;let mut consumed=0u128;
        let(mut immature,mut deferred,mut ready)=(0u128,0u128,0u128);
        for c in model.coins.values().filter(|c|c.owner==owner) {
            if c.payout {
                if c.spent {consumed+=c.value as u128;continue}
                unspent+=c.value as u128;let age=31231-c.created;
                if age<10 {immature+=c.value as u128}
                else if policy(mode).is_none_or(|min|age<min) {deferred+=c.value as u128}
                else {ready+=c.value as u128}
            } else if !c.spent {ordinary+=c.value as u128;}
        }
        assert_eq!(consumed,a.consumed);assert_eq!(a.capture,unspent+consumed);
        assert_eq!(ordinary+unspent+a.fees+a.sent,INITIAL+a.received+a.capture);
        assert_eq!(a.attempts,a.success+a.failures.values().sum::<u64>());
        json!({"owner":owner,"ordinary":ordinary.to_string(),"award_unspent":unspent.to_string(),"award_consumed":consumed.to_string(),
         "award_immature":immature.to_string(),"award_deferred":deferred.to_string(),"award_ready":ready.to_string(),
         "capture":a.capture.to_string(),"fees":a.fees.to_string(),"payments_sent":a.sent.to_string(),"payments_received":a.received.to_string(),
         "attempts":a.attempts,"success":a.success,"failures":a.failures})
    }).collect();
    assert_eq!(gross, accounts.iter().map(|a| a.fees).sum::<u128>());
    assert_eq!(
        payment_attempts,
        payment_success + payment_failures.values().sum::<u64>()
    );
    json!({"seed":seed,"mode":mode,"owners":owners,"payment_attempts":payment_attempts,"payment_success":payment_success,
     "payment_failures":payment_failures,"consolidation_opportunities":opportunities,"consolidation_attempts":attempts,"consolidation_success":success,
     "fees":gross.to_string(),"capture":captured.to_string(),"burn":burned.to_string(),"reserve":reserve.to_string(),
     "conditional_designated_capture":format!("{expected:.3}"),"public_outputs":model.coins.len(),"recycled_outputs":recycled.len(),
     "max_eligible":max_eligible,"max_spent_public_eligible":max_spent,"max_award_eligible":max_award,"max_recycled_eligible":max_recycled,
     "snapshots":snapshots,"receipts":receipts})
}
