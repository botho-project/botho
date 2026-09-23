//! Separate candidate model sharing only the non-test funded model helpers.
#[path = "common/funded_model.rs"]
mod funded_model;
#[path = "../../scripts/research/ct-economics/reference.rs"]
mod reference;
#[path = "common/reinvestment_model.rs"]
mod reinvestment_model;
mod reinvestment {
    use super::{
        funded_model::{Model, ATTACKER, BTH},
        reference,
    };
    use botho::consensus::lottery::{compute_pool_accounting, reward_cap, LotteryFeeConfig};
    use bth_cluster_tax::{count_eligible, draw_winners, LotteryDrawConfig};
    use serde_json::{json, Value};
    use std::collections::{BTreeMap, BTreeSet};

    use super::reinvestment_model::{award_inputs, awards, history};
    #[test]
    fn fee_funded_awards_consolidate_without_retiring_public_tickets() {
        let mut model = Model::default();
        let funding: Vec<_> = (0..25)
            .map(|_| model.add(0, 32 * BTH, 19000, false))
            .collect();
        let (mut pool, mut burned, mut gross) = (0u128, 0u128, 0u128);
        // Separate positive control: larger ordinary output count funds larger awards.
        // The eight-history matrix retains its two-output payments unchanged.
        for (step, id) in funding[..1].iter().enumerate() {
            let height = 20000 + step as u64;
            let (fee, _) = model
                .transfer(&[*id], 0, 0, None, 16, height, 1306)
                .unwrap();
            assert_eq!(fee, reference::fee(&[0], 16, 2).unwrap());
            let (reserve, burn, _, _, _, _, _, _) =
                awards(&mut model, height, 1306, fee, pool, "candidate_age720");
            pool = reserve;
            burned += burn;
            gross += fee as u128;
        }
        let ids = award_inputs(&model, 0, 20721, 720);
        assert!(ids.len() >= 3 && ids.len() <= 16);
        let before = model.clone();
        assert_eq!(
            model.transfer_with_award_policy(
                &[ids[0], ids[0]],
                0,
                0,
                None,
                1,
                20721,
                1306,
                Some(720)
            ),
            Err("duplicate_input")
        );
        assert_eq!(model, before);
        let total: u128 = ids.iter().map(|id| model.coins[id].value as u128).sum();
        let (fee, outputs) = model
            .transfer_with_award_policy(&ids, 0, 0, None, 1, 20721, 1306, Some(720))
            .unwrap();
        assert_eq!(fee, reference::fee(&vec![0; ids.len()], 1, 2).unwrap());
        assert_eq!(total, model.coins[&outputs[0]].value as u128 + fee as u128);
        for id in &ids {
            assert!(model.coins[id].spent);
            assert_eq!(model.coins[id].created, before.coins[id].created);
            assert!(model.coins[id]
                .candidate()
                .is_eligible(20721, &LotteryDrawConfig::default()));
        }
        assert!(!model.coins[&outputs[0]]
            .candidate()
            .is_eligible(20721, &LotteryDrawConfig::default()));
        let (reserve, burn, _, _, _, _, _, _) =
            awards(&mut model, 20721, 1306, fee, pool, "candidate_age720");
        burned += burn;
        gross += fee as u128;
        assert_eq!(model.accounted(0) + reserve + burned, 25 * 32 * BTH as u128);
        assert_eq!(
            gross,
            burned
                + reserve
                + model
                    .coins
                    .values()
                    .filter(|c| c.payout)
                    .map(|c| c.value as u128)
                    .sum::<u128>()
        );
    }
    #[test]
    fn policy_boundaries_and_failed_attempts_preserve_inventory() {
        let mut model = Model::default();
        for _ in 0..25 {
            model.add(0, BTH, 0, false);
        }
        let id = model.add(0, BTH, 1000, true);
        for age in [9, 10, 719, 720, 10000, 10001] {
            assert_eq!(
                model.coins[&id]
                    .candidate()
                    .is_eligible(1000 + age, &LotteryDrawConfig::default()),
                (720..=10000).contains(&age)
            );
            assert_eq!(
                !award_inputs(&model, 0, 1000 + age, 720).is_empty(),
                age >= 720
            );
            assert_eq!(
                !award_inputs(&model, 0, 1000 + age, 10001).is_empty(),
                age >= 10001
            );
        }
        let mut confirmed = model.clone();
        let before = confirmed.clone();
        assert_eq!(
            confirmed.transfer_with_award_policy(&[id], 0, 0, None, 1, 1009, 1306, Some(10)),
            Err("policy_deferred")
        );
        assert_eq!(confirmed, before);
        assert!(confirmed
            .transfer_with_award_policy(&[id], 0, 0, None, 1, 1010, 1306, Some(10))
            .is_ok());
        let before = model.clone();
        assert_eq!(
            model.transfer_with_award_policy(&[id], 0, 0, None, 1, 1719, 1306, Some(720)),
            Err("policy_deferred")
        );
        assert_eq!(model, before);
        assert_eq!(
            model.transfer(&[id], 0, 0, None, 1, 11001, 1306),
            Err("locked_payout")
        );
        assert_eq!(model, before);
        // Expired awards still own principal and may be recycled under this strategy.
        let mut expired = model.clone();
        assert!(expired
            .transfer_with_award_policy(&[id], 0, 0, None, 1, 11001, 1306, Some(10001))
            .is_ok());
        let small = model.add(0, 1, 1000, true);
        let before = model.clone();
        assert_eq!(
            model.transfer_with_award_policy(&[small], 0, 0, None, 1, 1720, 1306, Some(720)),
            Err("unaffordable")
        );
        assert_eq!(model, before);
        let mut scarce = Model::default();
        let id = scarce.add(0, BTH, 0, true);
        let before = scarce.clone();
        assert_eq!(
            scarce.transfer_with_award_policy(&[id], 0, 0, None, 1, 720, 1306, Some(720)),
            Err("selection_failure")
        );
        assert_eq!(scarce, before);
    }
    #[test]
    fn eight_histories_and_one_replay() {
        let config: Value = serde_json::from_str(include_str!(
            "../../scripts/research/ct-reinvestment/config.json"
        ))
        .unwrap();
        assert_eq!(config["schema"], 2);
        for (field, value) in [
            ("start_height", 20000),
            ("blocks", 11232),
            ("honest_owners", 100),
            ("owners", 101),
            ("initial_bth_per_owner", 32),
            ("honest_cadence", 100),
            ("consolidation_cadence", 100),
            ("max_award_inputs", 16),
            ("factor", 1000),
            ("rate_bps", 200),
            ("bucket_bits", 2),
            ("max_public_outputs", 5000),
            ("max_eligible_candidates", 10000),
            ("compile_seconds", 900),
            ("matrix_seconds", 180),
        ] {
            assert_eq!(config[field], value);
        }
        assert_eq!(config["seeds"], json!([1306, 902]));
        assert_eq!(
            config["modes"],
            json!([
                "legacy_locked",
                "candidate_locked",
                "candidate_age720",
                "candidate_age10001"
            ])
        );
        assert_eq!(
            config["payment_pico_cycle"],
            json!([BTH / 10, BTH, 10 * BTH])
        );
        let mut rows = vec![];
        for seed in [1306, 902] {
            for mode in [
                "legacy_locked",
                "candidate_locked",
                "candidate_age720",
                "candidate_age10001",
            ] {
                eprintln!("REINVEST_BEGIN seed={seed} mode={mode}");
                rows.push(history(seed, mode));
                eprintln!("REINVEST_DONE seed={seed} mode={mode}");
            }
        }
        assert_eq!(history(1306, "candidate_age720"), rows[2]);
        let result = json!({"schema":2,"config":config,"rows":rows});
        let out = std::env::var_os("CT_REINVESTMENT_OUTPUT").expect("collector output required");
        std::fs::write(out, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    }
}
