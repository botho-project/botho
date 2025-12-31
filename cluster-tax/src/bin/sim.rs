//! Cluster tax simulation CLI.
//!
//! Run economic scenarios to validate the cluster taxation model.

#[cfg(feature = "cli")]
mod cli {
    use bth_cluster_tax::{
        analysis::{
            analyze_fee_curve, analyze_structuring, analyze_wash_trading, hops_to_reach,
            tag_after_hops,
        },
        execute_transfer, mint, Account, AndDecayConfig, AndTagVector, BlockAwareTagVector,
        BlockDecayConfig, ClusterId, ClusterWealth, FeeCurve, RateLimitedDecayConfig,
        RateLimitedTagVector, TransferConfig, TAG_WEIGHT_SCALE,
    };
    use clap::{Parser, Subcommand};
    use rand::prelude::*;

    #[derive(Parser)]
    #[command(name = "cluster-tax-sim")]
    #[command(about = "Simulate cluster-based progressive transaction fees")]
    pub struct Cli {
        #[command(subcommand)]
        pub command: Command,
    }

    #[derive(Subcommand)]
    pub enum Command {
        /// Analyze tag decay over multiple hops
        Decay {
            /// Decay rate in percent (e.g., 5 for 5%)
            #[arg(short, long, default_value = "5")]
            rate: f64,

            /// Number of hops to simulate
            #[arg(short = 'n', long, default_value = "50")]
            hops: u32,
        },

        /// Analyze fee curve behavior
        FeeCurve {
            /// Number of sample points
            #[arg(short = 'n', long, default_value = "20")]
            samples: usize,
        },

        /// Analyze wash trading profitability
        WashTrading {
            /// Cluster wealth
            #[arg(short, long, default_value = "100000000")]
            wealth: u64,

            /// Decay rate in percent
            #[arg(short, long, default_value = "5")]
            decay: f64,

            /// Maximum hops to analyze
            #[arg(short = 'n', long, default_value = "50")]
            max_hops: u32,
        },

        /// Analyze structuring attack
        Structuring {
            /// Transfer amount
            #[arg(short, long, default_value = "1000000")]
            amount: u64,

            /// Cluster wealth
            #[arg(short, long, default_value = "50000000")]
            wealth: u64,
        },

        /// Run a whale diffusion scenario
        WhaleDiffusion {
            /// Initial whale wealth
            #[arg(short, long, default_value = "100000000")]
            wealth: u64,

            /// Number of economy participants
            #[arg(short, long, default_value = "100")]
            participants: usize,

            /// Number of simulation rounds
            #[arg(short, long, default_value = "1000")]
            rounds: usize,
        },

        /// Run a mixer scenario
        Mixer {
            /// Number of depositors
            #[arg(short, long, default_value = "10")]
            depositors: usize,

            /// Deposit amount per depositor
            #[arg(short, long, default_value = "1000000")]
            amount: u64,

            /// Number of mixing cycles
            #[arg(short, long, default_value = "100")]
            cycles: usize,
        },

        /// Scenario A: Baseline economy simulation with diverse agents
        ScenarioBaseline {
            /// Number of retail users
            #[arg(long, default_value = "100")]
            retail_users: usize,

            /// Number of merchants
            #[arg(long, default_value = "10")]
            merchants: usize,

            /// Whale wealth as fraction of total supply (0.0 to 1.0)
            #[arg(long, default_value = "0.1")]
            whale_fraction: f64,

            /// Number of simulation rounds
            #[arg(short, long, default_value = "10000")]
            rounds: u64,

            /// Verbose output
            #[arg(short, long)]
            verbose: bool,

            /// Show progress bar
            #[arg(long)]
            progress: bool,
        },

        /// Scenario B: Compare whale fee minimization strategies
        ScenarioWhale {
            /// Initial whale wealth
            #[arg(long, default_value = "10000000")]
            whale_wealth: u64,

            /// Number of other participants
            #[arg(long, default_value = "50")]
            participants: usize,

            /// Number of simulation rounds
            #[arg(short, long, default_value = "5000")]
            rounds: u64,
        },

        /// Scenario C: Mixer equilibrium with competing mixers
        ScenarioMixers {
            /// Number of competing mixers
            #[arg(long, default_value = "3")]
            num_mixers: usize,

            /// Number of whale users
            #[arg(long, default_value = "10")]
            whales: usize,

            /// Number of simulation rounds
            #[arg(short, long, default_value = "5000")]
            rounds: u64,
        },

        /// Scenario D: Velocity variation comparison
        ScenarioVelocity {
            /// Number of agents
            #[arg(long, default_value = "50")]
            agents: usize,

            /// Number of simulation rounds
            #[arg(short, long, default_value = "5000")]
            rounds: u64,
        },

        /// Scenario E: Parameter sensitivity analysis
        ScenarioParams {
            /// Number of agents
            #[arg(long, default_value = "50")]
            agents: usize,

            /// Number of simulation rounds per config
            #[arg(short, long, default_value = "2000")]
            rounds: u64,
        },

        /// Compare Gini coefficient evolution with and without progressive fees
        Compare {
            /// Number of retail users
            #[arg(long, default_value = "100")]
            retail_users: usize,

            /// Number of merchants
            #[arg(long, default_value = "10")]
            merchants: usize,

            /// Number of whales
            #[arg(long, default_value = "5")]
            whales: usize,

            /// Whale wealth as fraction of total (0.0 to 1.0)
            #[arg(long, default_value = "0.4")]
            whale_fraction: f64,

            /// Number of simulation rounds
            #[arg(short, long, default_value = "10000")]
            rounds: u64,

            /// Output directory for CSV files
            #[arg(short, long, default_value = ".")]
            output: String,

            /// Flat fee rate in basis points for comparison (default: average
            /// of progressive)
            #[arg(long, default_value = "100")]
            flat_rate: u32,
        },

        /// Analyze ring size cost/benefit tradeoffs
        RingSize {
            /// Ring sizes to analyze (comma-separated)
            #[arg(long, default_value = "5,7,9,11,13")]
            sizes: String,

            /// Run privacy simulations for each ring size
            #[arg(long)]
            simulate: bool,

            /// Number of simulations per ring size (if --simulate)
            #[arg(short = 'n', long, default_value = "1000")]
            simulations: usize,

            /// UTXO pool size for simulations
            #[arg(long, default_value = "50000")]
            pool_size: usize,
        },

        /// Simulate ring signature privacy under various adversary models
        Privacy {
            /// Number of ring simulations to run
            #[arg(short = 'n', long, default_value = "10000")]
            simulations: usize,

            /// Size of the UTXO pool
            #[arg(long, default_value = "100000")]
            pool_size: usize,

            /// Fraction of standard transactions (0.0 to 1.0)
            #[arg(long, default_value = "0.50")]
            standard_fraction: f64,

            /// Fraction of exchange transactions
            #[arg(long, default_value = "0.25")]
            exchange_fraction: f64,

            /// Fraction of whale transactions
            #[arg(long, default_value = "0.10")]
            whale_fraction: f64,

            /// Cluster decay rate per hop (percent)
            #[arg(long, default_value = "5.0")]
            decay_rate: f64,

            /// Enable cluster-aware decoy selection
            #[arg(long, default_value = "true")]
            cluster_aware: bool,

            /// Minimum cluster similarity threshold
            #[arg(long, default_value = "0.70")]
            min_similarity: f64,

            /// Disable parallel execution (use single-threaded mode)
            #[arg(long)]
            no_parallel: bool,

            /// Hide progress bar
            #[arg(long)]
            quiet: bool,
        },

        /// Compare block-based vs hop-based decay for wash trading resistance
        DecayCompare {
            /// Initial cluster wealth for simulation
            #[arg(long, default_value = "100000000")]
            wealth: u64,

            /// Hop-based decay rate in percent
            #[arg(long, default_value = "5.0")]
            hop_decay: f64,

            /// Block-based half-life in blocks
            #[arg(long, default_value = "60480")]
            half_life: u64,

            /// Number of wash trading transactions to simulate
            #[arg(long, default_value = "100")]
            wash_txs: usize,

            /// Blocks elapsed during wash trading period
            #[arg(long, default_value = "100")]
            blocks: u64,
        },

        /// Compare all three decay mechanisms: hop-based, block-based, rate-limited hybrid
        DecayCompareAll {
            /// Initial cluster wealth for simulation
            #[arg(long, default_value = "100000000")]
            wealth: u64,

            /// Hop-based decay rate in percent
            #[arg(long, default_value = "5.0")]
            hop_decay: f64,

            /// Block-based half-life in blocks
            #[arg(long, default_value = "60480")]
            half_life: u64,

            /// Rate-limited: minimum blocks between decays
            #[arg(long, default_value = "360")]
            min_blocks: u64,

            /// Number of wash trading transactions to simulate
            #[arg(long, default_value = "100")]
            wash_txs: usize,

            /// Blocks elapsed during wash trading period
            #[arg(long, default_value = "100")]
            blocks: u64,
        },

        /// Compare all four decay mechanisms including AND-based (time AND hop required)
        DecayCompareFour {
            /// Initial cluster wealth for simulation
            #[arg(long, default_value = "100000000")]
            wealth: u64,

            /// Hop-based decay rate in percent
            #[arg(long, default_value = "5.0")]
            hop_decay: f64,

            /// Block-based half-life in blocks
            #[arg(long, default_value = "60480")]
            half_life: u64,

            /// Minimum blocks between eligible decays (for rate-limited and AND models)
            #[arg(long, default_value = "360")]
            min_blocks: u64,

            /// Maximum decays per day (for AND model epoch cap)
            #[arg(long, default_value = "24")]
            max_per_day: u32,

            /// Number of wash trading transactions to simulate
            #[arg(long, default_value = "100")]
            wash_txs: usize,

            /// Blocks elapsed during simulation
            #[arg(long, default_value = "8640")]
            blocks: u64,
        },
    }

    pub fn run(cli: Cli) {
        match cli.command {
            Command::Decay { rate, hops } => run_decay_analysis(rate, hops),
            Command::FeeCurve { samples } => run_fee_curve_analysis(samples),
            Command::WashTrading {
                wealth,
                decay,
                max_hops,
            } => run_wash_trading_analysis(wealth, decay, max_hops),
            Command::Structuring { amount, wealth } => run_structuring_analysis(amount, wealth),
            Command::WhaleDiffusion {
                wealth,
                participants,
                rounds,
            } => run_whale_diffusion(wealth, participants, rounds),
            Command::Mixer {
                depositors,
                amount,
                cycles,
            } => run_mixer_scenario(depositors, amount, cycles),
            Command::ScenarioBaseline {
                retail_users,
                merchants,
                whale_fraction,
                rounds,
                verbose,
                progress,
            } => run_scenario_baseline(retail_users, merchants, whale_fraction, rounds, verbose, progress),
            Command::ScenarioWhale {
                whale_wealth,
                participants,
                rounds,
            } => run_scenario_whale(whale_wealth, participants, rounds),
            Command::ScenarioMixers {
                num_mixers,
                whales,
                rounds,
            } => run_scenario_mixers(num_mixers, whales, rounds),
            Command::ScenarioVelocity { agents, rounds } => run_scenario_velocity(agents, rounds),
            Command::ScenarioParams { agents, rounds } => run_scenario_params(agents, rounds),
            Command::Compare {
                retail_users,
                merchants,
                whales,
                whale_fraction,
                rounds,
                output,
                flat_rate,
            } => run_compare(
                retail_users,
                merchants,
                whales,
                whale_fraction,
                rounds,
                output,
                flat_rate,
            ),
            Command::RingSize {
                sizes,
                simulate,
                simulations,
                pool_size,
            } => run_ring_size_analysis(&sizes, simulate, simulations, pool_size),
            Command::Privacy {
                simulations,
                pool_size,
                standard_fraction,
                exchange_fraction,
                whale_fraction,
                decay_rate,
                cluster_aware,
                min_similarity,
                no_parallel,
                quiet,
            } => run_privacy_simulation(
                simulations,
                pool_size,
                standard_fraction,
                exchange_fraction,
                whale_fraction,
                decay_rate,
                cluster_aware,
                min_similarity,
                !no_parallel,
                !quiet,
            ),
            Command::DecayCompare {
                wealth,
                hop_decay,
                half_life,
                wash_txs,
                blocks,
            } => run_decay_comparison(wealth, hop_decay, half_life, wash_txs, blocks),
            Command::DecayCompareAll {
                wealth,
                hop_decay,
                half_life,
                min_blocks,
                wash_txs,
                blocks,
            } => run_decay_comparison_all(wealth, hop_decay, half_life, min_blocks, wash_txs, blocks),
            Command::DecayCompareFour {
                wealth,
                hop_decay,
                half_life,
                min_blocks,
                max_per_day,
                wash_txs,
                blocks,
            } => run_decay_comparison_four(wealth, hop_decay, half_life, min_blocks, max_per_day, wash_txs, blocks),
        }
    }

    fn run_decay_analysis(rate_pct: f64, max_hops: u32) {
        let decay_rate = (rate_pct / 100.0 * TAG_WEIGHT_SCALE as f64) as u32;

        println!("Tag Decay Analysis");
        println!("==================");
        println!("Decay rate: {rate_pct}% per hop\n");

        println!("{:>6} {:>12} {:>12}", "Hops", "Remaining", "Lost");
        println!("{:-<6} {:-<12} {:-<12}", "", "", "");

        for hops in (0..=max_hops).step_by(5.max(max_hops as usize / 10)) {
            let remaining = tag_after_hops(decay_rate, hops);
            println!(
                "{:>6} {:>11.2}% {:>11.2}%",
                hops,
                remaining * 100.0,
                (1.0 - remaining) * 100.0
            );
        }

        println!();
        if let Some(half_life) = hops_to_reach(decay_rate, 0.5) {
            println!("Hops to halve: {half_life}");
        }
        if let Some(tenth_life) = hops_to_reach(decay_rate, 0.1) {
            println!("Hops to 10%:   {tenth_life}");
        }
        if let Some(hundredth_life) = hops_to_reach(decay_rate, 0.01) {
            println!("Hops to 1%:    {hundredth_life}");
        }
    }

    fn run_fee_curve_analysis(samples: usize) {
        let fee_curve = FeeCurve::default_params();
        let analysis = analyze_fee_curve(&fee_curve, samples);

        println!("Fee Curve Analysis");
        println!("==================");
        println!(
            "r_min: {:.2}%  r_max: {:.2}%  w_mid: {}",
            fee_curve.r_min_bps as f64 / 100.0,
            fee_curve.r_max_bps as f64 / 100.0,
            fee_curve.w_mid
        );
        println!();

        println!(
            "{:>15} {:>10} {:>12}",
            "Cluster Wealth", "Fee Rate", "Marginal"
        );
        println!("{:-<15} {:-<10} {:-<12}", "", "", "");

        for i in 0..samples {
            println!(
                "{:>15} {:>9.2}% {:>12.6}",
                analysis.wealth_levels[i],
                analysis.fee_rates[i] as f64 / 100.0,
                analysis.marginal_rates[i] * 10000.0 // bps per unit wealth
            );
        }
    }

    fn run_wash_trading_analysis(wealth: u64, decay_pct: f64, max_hops: u32) {
        let fee_curve = FeeCurve::default_params();
        let decay_rate = (decay_pct / 100.0 * TAG_WEIGHT_SCALE as f64) as u32;

        println!("Wash Trading Analysis");
        println!("=====================");
        println!("Cluster wealth: {wealth}");
        println!("Decay rate: {decay_pct}% per hop\n");

        println!(
            "{:>6} {:>10} {:>10} {:>12} {:>12} {:>15}",
            "Hops", "Init Rate", "Final Rate", "Total Fees", "Savings/Tx", "Break-Even"
        );
        println!(
            "{:-<6} {:-<10} {:-<10} {:-<12} {:-<12} {:-<15}",
            "", "", "", "", "", ""
        );

        for hops in [5, 10, 15, 20, 30, 40, 50]
            .iter()
            .filter(|&&h| h <= max_hops)
        {
            let analysis = analyze_wash_trading(wealth, decay_rate, *hops, &fee_curve);

            let break_even_str = match analysis.break_even_transactions {
                Some(n) => format!("{n} txs"),
                None => "Never".to_string(),
            };

            println!(
                "{:>6} {:>9.2}% {:>9.2}% {:>11.2}% {:>11.4}% {:>15}",
                hops,
                analysis.initial_rate_bps as f64 / 100.0,
                analysis.final_rate_bps as f64 / 100.0,
                analysis.total_fees_fraction * 100.0,
                analysis.fee_savings_per_tx * 100.0,
                break_even_str
            );
        }
    }

    fn run_structuring_analysis(amount: u64, wealth: u64) {
        let fee_curve = FeeCurve::default_params();

        println!("Structuring Attack Analysis");
        println!("===========================");
        println!("Transfer amount: {amount}");
        println!("Cluster wealth: {wealth}\n");

        println!(
            "{:>8} {:>12} {:>12} {:>12} {:>12}",
            "Splits", "Single Fee", "Split Fees", "Difference", "Savings %"
        );
        println!("{:-<8} {:-<12} {:-<12} {:-<12} {:-<12}", "", "", "", "", "");

        for splits in [1, 2, 5, 10, 20, 50, 100] {
            let analysis = analyze_structuring(amount, wealth, splits, &fee_curve);

            let savings_pct = if analysis.single_fee > 0 {
                analysis.savings as f64 / analysis.single_fee as f64 * 100.0
            } else {
                0.0
            };

            println!(
                "{:>8} {:>12} {:>12} {:>12} {:>11.2}%",
                splits,
                analysis.single_fee,
                analysis.total_split_fees,
                analysis.savings,
                savings_pct
            );
        }
    }

    fn run_whale_diffusion(initial_wealth: u64, num_participants: usize, rounds: usize) {
        let mut rng = rand::thread_rng();
        let config = TransferConfig::default();
        let mut cluster_wealth = ClusterWealth::new();

        // Create whale account
        let whale_cluster = ClusterId::new(0);
        let mut whale = Account::new(0);
        mint(
            &mut whale,
            initial_wealth,
            whale_cluster,
            &mut cluster_wealth,
        );

        // Create participant accounts with small initial balances
        let mut participants: Vec<Account> = (1..=num_participants)
            .map(|id| {
                let mut acc = Account::new(id as u64);
                let cluster = ClusterId::new(id as u64);
                mint(&mut acc, 10_000, cluster, &mut cluster_wealth);
                acc
            })
            .collect();

        println!("Whale Diffusion Simulation");
        println!("==========================");
        println!("Initial whale wealth: {initial_wealth}");
        println!("Participants: {num_participants}");
        println!("Rounds: {rounds}\n");

        let fee_curve = FeeCurve::default_params();
        let initial_rate = whale.effective_fee_rate(&cluster_wealth, &fee_curve);

        println!(
            "{:>8} {:>15} {:>12} {:>12} {:>15}",
            "Round", "Whale Balance", "Whale Rate", "Avg P Rate", "Whale Cluster W"
        );
        println!("{:-<8} {:-<15} {:-<12} {:-<12} {:-<15}", "", "", "", "", "");

        let whale_cluster_wealth = cluster_wealth.get(whale_cluster);
        println!(
            "{:>8} {:>15} {:>11.2}% {:>11.2}% {:>15}",
            0,
            whale.balance,
            initial_rate as f64 / 100.0,
            fee_curve.background_rate_bps as f64 / 100.0,
            whale_cluster_wealth
        );

        let mut total_fees = 0u64;

        for round in 1..=rounds {
            // Whale sends to random participant
            if whale.balance > 1000 {
                let amount = rng.gen_range(100..=whale.balance.min(10000));
                let recipient_idx = rng.gen_range(0..participants.len());

                if let Ok(result) = execute_transfer(
                    &mut whale,
                    &mut participants[recipient_idx],
                    amount,
                    &config,
                    &mut cluster_wealth,
                ) {
                    total_fees += result.fee;
                }
            }

            // Participants trade among themselves
            for _ in 0..5 {
                let sender_idx = rng.gen_range(0..participants.len());
                let receiver_idx = rng.gen_range(0..participants.len());
                if sender_idx != receiver_idx && participants[sender_idx].balance > 100 {
                    let amount = rng.gen_range(10..=participants[sender_idx].balance.min(1000));
                    // Use split_at_mut to get two mutable references
                    let (lo, hi) = if sender_idx < receiver_idx {
                        let (left, right) = participants.split_at_mut(receiver_idx);
                        (&mut left[sender_idx], &mut right[0])
                    } else {
                        let (left, right) = participants.split_at_mut(sender_idx);
                        (&mut right[0], &mut left[receiver_idx])
                    };
                    let _ = execute_transfer(lo, hi, amount, &config, &mut cluster_wealth);
                }
            }

            // Print status every 100 rounds
            if round % 100 == 0 || round == rounds {
                let whale_rate = whale.effective_fee_rate(&cluster_wealth, &fee_curve);
                let avg_participant_rate: f64 = participants
                    .iter()
                    .map(|p| p.effective_fee_rate(&cluster_wealth, &fee_curve) as f64)
                    .sum::<f64>()
                    / participants.len() as f64;
                let whale_cluster_wealth = cluster_wealth.get(whale_cluster);

                println!(
                    "{:>8} {:>15} {:>11.2}% {:>11.2}% {:>15}",
                    round,
                    whale.balance,
                    whale_rate as f64 / 100.0,
                    avg_participant_rate / 100.0,
                    whale_cluster_wealth
                );
            }
        }

        println!("\nTotal fees collected: {total_fees}");
        let final_rate = whale.effective_fee_rate(&cluster_wealth, &fee_curve);
        println!(
            "Whale rate change: {:.2}% -> {:.2}%",
            initial_rate as f64 / 100.0,
            final_rate as f64 / 100.0
        );
    }

    fn run_mixer_scenario(num_depositors: usize, deposit_amount: u64, cycles: usize) {
        let config = TransferConfig::default();
        let mut cluster_wealth = ClusterWealth::new();
        let fee_curve = FeeCurve::default_params();

        // Create depositors with high-tag wealth
        let mut depositors: Vec<Account> = (0..num_depositors)
            .map(|id| {
                let cluster = ClusterId::new(id as u64);
                let mut acc = Account::new(id as u64);
                // Each depositor has large cluster wealth (simulating whales)
                let cluster_total = deposit_amount * 1000; // Their cluster is much larger
                cluster_wealth.set(cluster, cluster_total);
                mint(&mut acc, deposit_amount, cluster, &mut cluster_wealth);
                acc
            })
            .collect();

        // Create mixer account
        let mixer_cluster = ClusterId::new(1000);
        let mut mixer = Account::new(1000);
        mint(&mut mixer, 1000, mixer_cluster, &mut cluster_wealth);

        println!("Mixer Scenario Simulation");
        println!("=========================");
        println!("Depositors: {num_depositors}");
        println!("Deposit amount: {deposit_amount}");
        println!("Cycles: {cycles}\n");

        // Initial deposits
        println!("Phase 1: Deposits");
        for depositor in &mut depositors {
            let initial_rate = depositor.effective_fee_rate(&cluster_wealth, &fee_curve);
            if let Ok(result) = execute_transfer(
                depositor,
                &mut mixer,
                deposit_amount / 2,
                &config,
                &mut cluster_wealth,
            ) {
                println!(
                    "  Depositor {} -> Mixer: {} (fee: {}, rate: {:.2}%)",
                    depositor.id,
                    result.net_amount,
                    result.fee,
                    initial_rate as f64 / 100.0
                );
            }
        }

        let mixer_rate_after_deposits = mixer.effective_fee_rate(&cluster_wealth, &fee_curve);
        println!("\nMixer balance after deposits: {}", mixer.balance);
        println!(
            "Mixer effective rate: {:.2}%",
            mixer_rate_after_deposits as f64 / 100.0
        );

        // Mixing cycles (internal shuffling)
        println!("\nPhase 2: Mixing ({cycles} internal cycles)");

        // Simulate by having depositors withdraw to each other
        for cycle in 0..cycles {
            let sender_idx = cycle % depositors.len();
            let receiver_idx = (cycle + 1) % depositors.len();

            if mixer.balance > 1000 {
                let amount = mixer.balance.min(deposit_amount / 10);
                let _ = execute_transfer(
                    &mut mixer,
                    &mut depositors[receiver_idx],
                    amount,
                    &config,
                    &mut cluster_wealth,
                );
            }

            // Redeposit
            if depositors[sender_idx].balance > 1000 {
                let amount = depositors[sender_idx].balance.min(deposit_amount / 20);
                let _ = execute_transfer(
                    &mut depositors[sender_idx],
                    &mut mixer,
                    amount,
                    &config,
                    &mut cluster_wealth,
                );
            }
        }

        // Final state
        println!("\nFinal State:");
        println!(
            "Mixer balance: {}, rate: {:.2}%",
            mixer.balance,
            mixer.effective_fee_rate(&cluster_wealth, &fee_curve) as f64 / 100.0
        );

        println!("\nDepositor states:");
        for depositor in &depositors {
            let rate = depositor.effective_fee_rate(&cluster_wealth, &fee_curve);
            println!(
                "  Depositor {}: balance = {}, rate = {:.2}%",
                depositor.id,
                depositor.balance,
                rate as f64 / 100.0
            );
        }
    }

    // ========== Agent-Based Scenarios ==========

    fn run_scenario_baseline(
        num_retail: usize,
        num_merchants: usize,
        whale_fraction: f64,
        rounds: u64,
        verbose: bool,
        show_progress: bool,
    ) {
        use bth_cluster_tax::simulation::{
            agents::whale::WhaleStrategy, run_simulation, run_simulation_with_progress, Agent,
            AgentId, MerchantAgent, MinterAgent, MixerServiceAgent, RetailUserAgent,
            SimulationConfig, WhaleAgent,
        };

        println!("Scenario A: Baseline Economy");
        println!("=============================");
        println!("Retail users: {num_retail}");
        println!("Merchants: {num_merchants}");
        println!("Whale wealth fraction: {:.1}%", whale_fraction * 100.0);
        println!("Rounds: {rounds}\n");

        // Calculate total supply
        let retail_balance = 1000u64;
        let merchant_balance = 5000u64;
        let minter_balance = 10000u64;
        let base_supply = (num_retail as u64 * retail_balance)
            + (num_merchants as u64 * merchant_balance)
            + minter_balance;
        let whale_wealth = (base_supply as f64 * whale_fraction / (1.0 - whale_fraction)) as u64;
        let total_supply = base_supply + whale_wealth;

        println!("Total supply: {total_supply}");
        println!(
            "Whale wealth: {whale_wealth} ({:.1}%)\n",
            whale_wealth as f64 / total_supply as f64 * 100.0
        );

        let mut agents: Vec<Box<dyn Agent>> = Vec::new();
        let mut next_id = 0u64;

        // Create merchants first (so retail can reference them)
        let merchant_ids: Vec<AgentId> = (0..num_merchants)
            .map(|_| {
                let id = AgentId(next_id);
                next_id += 1;
                id
            })
            .collect();

        for &id in &merchant_ids {
            let mut merchant = MerchantAgent::new(id)
                .with_payment_threshold(10000)
                .with_supplier_payment_fraction(0.3);
            merchant.account_mut_ref().balance = merchant_balance;
            agents.push(Box::new(merchant));
        }

        // Create retail users
        for _ in 0..num_retail {
            let id = AgentId(next_id);
            next_id += 1;
            let mut retail = RetailUserAgent::new(id)
                .with_merchants(merchant_ids.clone())
                .with_spending_probability(0.1)
                .with_avg_spend(50);
            retail.account_mut_ref().balance = retail_balance;
            agents.push(Box::new(retail));
        }

        // Create whale (passive strategy)
        let whale_id = AgentId(next_id);
        next_id += 1;
        let mut whale = WhaleAgent::new(whale_id, whale_wealth, WhaleStrategy::Passive)
            .with_spending_targets(merchant_ids.clone())
            .with_spending_rate(0.001);
        whale.account_mut_ref().balance = whale_wealth;
        agents.push(Box::new(whale));

        // Create minter
        let minter_id = AgentId(next_id);
        next_id += 1;
        let mut minter = MinterAgent::new(minter_id)
            .with_buyers(merchant_ids)
            .with_block_reward(100)
            .with_minting_interval(10);
        minter.account_mut_ref().balance = minter_balance;
        agents.push(Box::new(minter));

        // Create mixer
        let mixer_id = AgentId(next_id);
        let mixer = MixerServiceAgent::new(mixer_id)
            .with_fee_bps(100)
            .with_withdrawal_delay(5);
        agents.push(Box::new(mixer));

        // Run simulation
        let config = SimulationConfig {
            rounds,
            snapshot_frequency: rounds / 20,
            verbose,
            ..Default::default()
        };

        let result = if show_progress {
            run_simulation_with_progress(&mut agents, &config)
        } else {
            run_simulation(&mut agents, &config)
        };
        let summary = result.metrics.summary();

        // Print results
        println!("\n===== RESULTS =====\n");
        println!(
            "Gini coefficient: {:.4} -> {:.4} (change: {:+.4})",
            summary.initial_gini,
            summary.final_gini,
            summary.final_gini - summary.initial_gini
        );
        println!("Total fees collected: {}", summary.total_fees);
        println!("Total transactions: {}", summary.total_transactions);
        println!("\nFee rates by wealth quintile (poorest to richest):");
        for (i, rate) in summary.avg_fee_by_quintile.iter().enumerate() {
            println!("  Q{}: {:.2} bps", i + 1, rate);
        }
        println!(
            "\nWash trading: {} attempts, net savings: {}",
            summary.wash_trade_attempts, summary.wash_trade_net_savings
        );
    }

    fn run_scenario_whale(whale_wealth: u64, num_participants: usize, rounds: u64) {
        use bth_cluster_tax::simulation::{
            agents::whale::WhaleStrategy, run_simulation, Agent, AgentId, MerchantAgent,
            RetailUserAgent, SimulationConfig, WhaleAgent,
        };

        println!("Scenario B: Whale Fee Minimization Strategies");
        println!("==============================================");
        println!("Whale wealth: {whale_wealth}");
        println!("Participants: {num_participants}");
        println!("Rounds: {rounds}\n");

        let strategies = [
            ("Passive", WhaleStrategy::Passive),
            ("Wash Trading", WhaleStrategy::WashTrading),
            ("Structuring", WhaleStrategy::Structuring),
            ("Aggressive", WhaleStrategy::Aggressive),
        ];

        println!(
            "{:<15} {:>12} {:>12} {:>12} {:>15}",
            "Strategy", "Final Gini", "Total Fees", "Whale Fees", "Effectiveness"
        );
        println!(
            "{:-<15} {:-<12} {:-<12} {:-<12} {:-<15}",
            "", "", "", "", ""
        );

        let mut baseline_fees = 0u64;

        for (name, strategy) in strategies {
            let mut agents: Vec<Box<dyn Agent>> = Vec::new();
            let mut next_id = 0u64;

            // Create merchant targets
            let merchant_ids: Vec<AgentId> = (0..5)
                .map(|_| {
                    let id = AgentId(next_id);
                    next_id += 1;
                    id
                })
                .collect();

            for &id in &merchant_ids {
                let mut merchant = MerchantAgent::new(id);
                merchant.account_mut_ref().balance = 5000;
                agents.push(Box::new(merchant));
            }

            // Create participants
            for _ in 0..num_participants {
                let id = AgentId(next_id);
                next_id += 1;
                let mut retail = RetailUserAgent::new(id).with_merchants(merchant_ids.clone());
                retail.account_mut_ref().balance = 1000;
                agents.push(Box::new(retail));
            }

            // Create whale with this strategy
            let whale_id = AgentId(next_id);
            let mut whale = WhaleAgent::new(whale_id, whale_wealth, strategy)
                .with_spending_targets(merchant_ids)
                .with_spending_rate(0.002);
            whale.account_mut_ref().balance = whale_wealth;
            agents.push(Box::new(whale));

            // Run simulation
            let config = SimulationConfig {
                rounds,
                snapshot_frequency: rounds / 10,
                verbose: false,
                ..Default::default()
            };

            let result = run_simulation(&mut agents, &config);
            let summary = result.metrics.summary();

            let whale_fees = result
                .metrics
                .agent_fees
                .get(&whale_id)
                .copied()
                .unwrap_or(0);

            if name == "Passive" {
                baseline_fees = whale_fees;
            }

            let effectiveness = if baseline_fees > 0 {
                (baseline_fees as f64 - whale_fees as f64) / baseline_fees as f64 * 100.0
            } else {
                0.0
            };

            println!(
                "{:<15} {:>12.4} {:>12} {:>12} {:>14.1}%",
                name, summary.final_gini, summary.total_fees, whale_fees, effectiveness
            );
        }

        println!("\nNote: Effectiveness = reduction in whale fees vs passive strategy");
    }

    fn run_scenario_mixers(num_mixers: usize, num_whales: usize, rounds: u64) {
        use bth_cluster_tax::simulation::{
            agents::whale::WhaleStrategy, run_simulation, Agent, AgentId, MixerServiceAgent,
            RetailUserAgent, SimulationConfig, WhaleAgent,
        };

        println!("Scenario C: Mixer Equilibrium");
        println!("=============================");
        println!("Competing mixers: {num_mixers}");
        println!("Whale users: {num_whales}");
        println!("Rounds: {rounds}\n");

        // Different fee levels for competing mixers
        let mixer_fees = [50, 100, 200]; // 0.5%, 1%, 2%

        let mut agents: Vec<Box<dyn Agent>> = Vec::new();
        let mut mixer_ids = Vec::new();
        let mut next_id = 0u64;

        // Create mixers with different fees
        for i in 0..num_mixers {
            let id = AgentId(next_id);
            next_id += 1;
            mixer_ids.push(id);

            let fee = mixer_fees[i % mixer_fees.len()];
            let mixer = MixerServiceAgent::new(id)
                .with_fee_bps(fee)
                .with_withdrawal_delay(3);
            agents.push(Box::new(mixer));
        }

        // Create whales that use mixers
        for _ in 0..num_whales {
            let id = AgentId(next_id);
            next_id += 1;

            let mut whale =
                WhaleAgent::new(id, 1_000_000, WhaleStrategy::UseMixers).with_spending_rate(0.001);
            whale.account_mut_ref().balance = 1_000_000;
            agents.push(Box::new(whale));
        }

        // Create retail users
        for _ in 0..20 {
            let id = AgentId(next_id);
            next_id += 1;
            let mut retail = RetailUserAgent::new(id);
            retail.account_mut_ref().balance = 1000;
            agents.push(Box::new(retail));
        }

        let config = SimulationConfig {
            rounds,
            snapshot_frequency: rounds / 10,
            verbose: false,
            ..Default::default()
        };

        let result = run_simulation(&mut agents, &config);
        let summary = result.metrics.summary();

        println!("Results:");
        println!("  Final Gini: {:.4}", summary.final_gini);
        println!("  Total fees: {}", summary.total_fees);
        println!(
            "  Mixer utilization: {:.2}%",
            summary.mixer_utilization * 100.0
        );

        println!("\nMixer statistics:");
        for (i, &mixer_id) in mixer_ids.iter().enumerate() {
            let balance = agents
                .iter()
                .find(|a| a.id() == mixer_id)
                .map(|a| a.balance())
                .unwrap_or(0);
            println!(
                "  Mixer {} ({}bps fee): balance = {}",
                i + 1,
                mixer_fees[i % mixer_fees.len()],
                balance
            );
        }
    }

    fn run_scenario_velocity(num_agents: usize, rounds: u64) {
        use bth_cluster_tax::simulation::{
            run_simulation, Agent, AgentId, MarketMakerAgent, RetailUserAgent, SimulationConfig,
        };

        println!("Scenario D: Velocity Variation");
        println!("===============================");
        println!("Agents: {num_agents}");
        println!("Rounds: {rounds}\n");

        let configs = [
            ("Low velocity", 0.05, 1),    // 5% spending prob, 1 trade/round
            ("Medium velocity", 0.15, 3), // 15% spending prob, 3 trades/round
            ("High velocity", 0.30, 5),   // 30% spending prob, 5 trades/round
        ];

        println!(
            "{:<20} {:>12} {:>12} {:>15} {:>12}",
            "Config", "Final Gini", "Total Fees", "Transactions", "Gini Change"
        );
        println!(
            "{:-<20} {:-<12} {:-<12} {:-<15} {:-<12}",
            "", "", "", "", ""
        );

        for (name, spending_prob, trades_per_round) in configs {
            let mut agents: Vec<Box<dyn Agent>> = Vec::new();

            // Half retail, half market makers
            for i in 0..num_agents / 2 {
                let id = AgentId(i as u64);
                let mut retail = RetailUserAgent::new(id)
                    .with_spending_probability(spending_prob)
                    .with_avg_spend(100);
                retail.account_mut_ref().balance = 10000;
                agents.push(Box::new(retail));
            }

            for i in num_agents / 2..num_agents {
                let id = AgentId(i as u64);
                let counterparties: Vec<AgentId> =
                    (0..num_agents as u64 / 2).map(AgentId).collect();
                let mut mm = MarketMakerAgent::new(id)
                    .with_counterparties(counterparties)
                    .with_trades_per_round(trades_per_round);
                mm.account_mut_ref().balance = 50000;
                agents.push(Box::new(mm));
            }

            let config = SimulationConfig {
                rounds,
                snapshot_frequency: rounds / 10,
                verbose: false,
                ..Default::default()
            };

            let result = run_simulation(&mut agents, &config);
            let summary = result.metrics.summary();
            let gini_change = summary.final_gini - summary.initial_gini;

            println!(
                "{:<20} {:>12.4} {:>12} {:>15} {:>+12.4}",
                name,
                summary.final_gini,
                summary.total_fees,
                summary.total_transactions,
                gini_change
            );
        }
    }

    fn run_scenario_params(num_agents: usize, rounds: u64) {
        use bth_cluster_tax::simulation::{
            agents::whale::WhaleStrategy, run_simulation, Agent, AgentId, RetailUserAgent,
            SimulationConfig, WhaleAgent,
        };

        println!("Scenario E: Parameter Sensitivity");
        println!("==================================");
        println!("Agents: {num_agents}");
        println!("Rounds per config: {rounds}\n");

        let decay_rates = [0.01, 0.05, 0.10, 0.20];

        println!("Decay Rate Sensitivity:");
        println!(
            "{:<12} {:>12} {:>12} {:>15} {:>12}",
            "Decay Rate", "Final Gini", "Total Fees", "Whale Fees", "Inequality Δ"
        );
        println!(
            "{:-<12} {:-<12} {:-<12} {:-<15} {:-<12}",
            "", "", "", "", ""
        );

        for &decay_rate in &decay_rates {
            let mut agents: Vec<Box<dyn Agent>> = Vec::new();

            // Create agents
            for i in 0..num_agents - 1 {
                let id = AgentId(i as u64);
                let mut retail = RetailUserAgent::new(id).with_spending_probability(0.1);
                retail.account_mut_ref().balance = 1000;
                agents.push(Box::new(retail));
            }

            // One whale
            let whale_id = AgentId(num_agents as u64 - 1);
            let targets: Vec<AgentId> = (0..5).map(|i| AgentId(i as u64)).collect();
            let mut whale = WhaleAgent::new(whale_id, 0, WhaleStrategy::Passive)
                .with_spending_targets(targets)
                .with_spending_rate(0.002);
            whale.account_mut_ref().balance = 100_000;
            agents.push(Box::new(whale));

            let mut config = SimulationConfig {
                rounds,
                snapshot_frequency: rounds / 5,
                verbose: false,
                ..Default::default()
            };
            config.transfer_config.decay_rate = (decay_rate * TAG_WEIGHT_SCALE as f64) as u32;

            let result = run_simulation(&mut agents, &config);
            let summary = result.metrics.summary();
            let whale_fees = result
                .metrics
                .agent_fees
                .get(&whale_id)
                .copied()
                .unwrap_or(0);
            let gini_change = summary.final_gini - summary.initial_gini;

            println!(
                "{:<12.0}% {:>12.4} {:>12} {:>15} {:>+12.4}",
                decay_rate * 100.0,
                summary.final_gini,
                summary.total_fees,
                whale_fees,
                gini_change
            );
        }

        println!("\nFee Curve Steepness Sensitivity:");
        let steepness_values = [1_000_000u64, 5_000_000, 10_000_000, 20_000_000];

        println!(
            "{:<15} {:>12} {:>12} {:>15}",
            "Steepness", "Final Gini", "Total Fees", "Whale Fees"
        );
        println!("{:-<15} {:-<12} {:-<12} {:-<15}", "", "", "", "");

        for &steepness in &steepness_values {
            let mut agents: Vec<Box<dyn Agent>> = Vec::new();

            for i in 0..num_agents - 1 {
                let id = AgentId(i as u64);
                let mut retail = RetailUserAgent::new(id).with_spending_probability(0.1);
                retail.account_mut_ref().balance = 1000;
                agents.push(Box::new(retail));
            }

            let whale_id = AgentId(num_agents as u64 - 1);
            let targets: Vec<AgentId> = (0..5).map(|i| AgentId(i as u64)).collect();
            let mut whale = WhaleAgent::new(whale_id, 0, WhaleStrategy::Passive)
                .with_spending_targets(targets)
                .with_spending_rate(0.002);
            whale.account_mut_ref().balance = 100_000;
            agents.push(Box::new(whale));

            let mut config = SimulationConfig {
                rounds,
                snapshot_frequency: rounds / 5,
                verbose: false,
                ..Default::default()
            };
            config.fee_curve.steepness = steepness;

            let result = run_simulation(&mut agents, &config);
            let summary = result.metrics.summary();
            let whale_fees = result
                .metrics
                .agent_fees
                .get(&whale_id)
                .copied()
                .unwrap_or(0);

            println!(
                "{:<15} {:>12.4} {:>12} {:>15}",
                steepness, summary.final_gini, summary.total_fees, whale_fees
            );
        }
    }

    fn run_compare(
        num_retail: usize,
        num_merchants: usize,
        num_whales: usize,
        whale_fraction: f64,
        rounds: u64,
        output_dir: String,
        flat_rate_bps: u32,
    ) {
        use bth_cluster_tax::simulation::{
            agents::whale::WhaleStrategy, run_simulation, Agent, AgentId, MerchantAgent,
            MinterAgent, RetailUserAgent, SimulationConfig, WhaleAgent,
        };
        use std::fs;

        println!("==============================================");
        println!("GINI COEFFICIENT COMPARISON");
        println!("Progressive vs Flat Transaction Fees");
        println!("==============================================\n");

        println!("Configuration:");
        println!("  Retail users:     {num_retail}");
        println!("  Merchants:        {num_merchants}");
        println!("  Whales:           {num_whales}");
        println!("  Whale fraction:   {:.1}%", whale_fraction * 100.0);
        println!("  Rounds:           {rounds}");
        println!(
            "  Flat rate:        {} bps ({:.2}%)",
            flat_rate_bps,
            flat_rate_bps as f64 / 100.0
        );
        println!("  Output dir:       {output_dir}\n");

        // Helper to create agents with given seed for reproducibility
        fn create_agents(
            num_retail: usize,
            num_merchants: usize,
            num_whales: usize,
            whale_fraction: f64,
        ) -> (Vec<Box<dyn Agent>>, u64) {
            let mut agents: Vec<Box<dyn Agent>> = Vec::new();
            let mut next_id = 0u64;

            // Calculate total supply
            let retail_balance = 1_000u64;
            let merchant_balance = 10_000u64;
            let minter_balance = 5_000u64;
            let base_supply = (num_retail as u64 * retail_balance)
                + (num_merchants as u64 * merchant_balance)
                + minter_balance;
            let whale_wealth_total =
                (base_supply as f64 * whale_fraction / (1.0 - whale_fraction)) as u64;
            let whale_wealth_each = whale_wealth_total / num_whales.max(1) as u64;

            // Create merchants first
            let merchant_ids: Vec<AgentId> = (0..num_merchants)
                .map(|_| {
                    let id = AgentId(next_id);
                    next_id += 1;
                    id
                })
                .collect();

            for &id in &merchant_ids {
                let mut merchant = MerchantAgent::new(id)
                    .with_payment_threshold(20000)
                    .with_supplier_payment_fraction(0.3);
                merchant.account_mut_ref().balance = merchant_balance;
                agents.push(Box::new(merchant));
            }

            // Create retail users
            for _ in 0..num_retail {
                let id = AgentId(next_id);
                next_id += 1;
                let mut retail = RetailUserAgent::new(id)
                    .with_merchants(merchant_ids.clone())
                    .with_spending_probability(0.15)
                    .with_avg_spend(50);
                retail.account_mut_ref().balance = retail_balance;
                agents.push(Box::new(retail));
            }

            // Create whales
            for _ in 0..num_whales {
                let whale_id = AgentId(next_id);
                next_id += 1;
                let mut whale =
                    WhaleAgent::new(whale_id, whale_wealth_each, WhaleStrategy::Passive)
                        .with_spending_targets(merchant_ids.clone())
                        .with_spending_rate(0.002);
                whale.account_mut_ref().balance = whale_wealth_each;
                agents.push(Box::new(whale));
            }

            // Create minter
            let minter_id = AgentId(next_id);
            let mut minter = MinterAgent::new(minter_id)
                .with_buyers(merchant_ids)
                .with_block_reward(100)
                .with_minting_interval(10);
            minter.account_mut_ref().balance = minter_balance;
            agents.push(Box::new(minter));

            let total_supply = base_supply + whale_wealth_total;
            (agents, total_supply)
        }

        // Run with progressive fees
        println!("Running simulation with PROGRESSIVE fees...");
        let (mut progressive_agents, total_supply) =
            create_agents(num_retail, num_merchants, num_whales, whale_fraction);

        // Scale the fee curve to match simulation wealth levels
        // w_mid should be set so whale clusters are in the high-fee region
        let whale_wealth_each =
            (total_supply as f64 * whale_fraction / num_whales.max(1) as f64) as u64;
        let progressive_fee_curve = FeeCurve {
            r_min_bps: 5,                     // 0.05% for small/diffused
            r_max_bps: 2000,                  // 20% for large concentrated clusters
            w_mid: whale_wealth_each / 2,     // Midpoint at half whale wealth
            steepness: whale_wealth_each / 4, // Gradual transition
            background_rate_bps: 10,          // 0.1% for diffused coins
        };

        println!(
            "  Fee curve: w_mid={}, whale_wealth={}",
            progressive_fee_curve.w_mid, whale_wealth_each
        );

        let progressive_config = SimulationConfig {
            rounds,
            fee_curve: progressive_fee_curve,
            snapshot_frequency: rounds / 100,
            verbose: false,
            ..Default::default()
        };
        let progressive_result = run_simulation(&mut progressive_agents, &progressive_config);
        let progressive_summary = progressive_result.metrics.summary();

        // Run with flat fees
        println!("Running simulation with FLAT fees...");
        let (mut flat_agents, _) =
            create_agents(num_retail, num_merchants, num_whales, whale_fraction);
        let flat_config = SimulationConfig {
            rounds,
            fee_curve: FeeCurve::flat(flat_rate_bps),
            snapshot_frequency: rounds / 100,
            verbose: false,
            ..Default::default()
        };
        let flat_result = run_simulation(&mut flat_agents, &flat_config);
        let flat_summary = flat_result.metrics.summary();

        // Print comparison
        println!("\n==============================================");
        println!("RESULTS");
        println!("==============================================\n");

        println!("Total supply: {total_supply}\n");

        println!("{:<25} {:>15} {:>15}", "", "Progressive", "Flat");
        println!("{:-<25} {:-<15} {:-<15}", "", "", "");
        println!(
            "{:<25} {:>15.4} {:>15.4}",
            "Initial Gini", progressive_summary.initial_gini, flat_summary.initial_gini
        );
        println!(
            "{:<25} {:>15.4} {:>15.4}",
            "Final Gini", progressive_summary.final_gini, flat_summary.final_gini
        );
        println!(
            "{:<25} {:>+15.4} {:>+15.4}",
            "Gini Change",
            progressive_summary.final_gini - progressive_summary.initial_gini,
            flat_summary.final_gini - flat_summary.initial_gini
        );
        println!(
            "{:<25} {:>15} {:>15}",
            "Total Fees", progressive_summary.total_fees, flat_summary.total_fees
        );
        println!(
            "{:<25} {:>15} {:>15}",
            "Transactions", progressive_summary.total_transactions, flat_summary.total_transactions
        );

        println!("\nFee rates by wealth quintile (bps):");
        println!("{:<25} {:>15} {:>15}", "", "Progressive", "Flat");
        for i in 0..5 {
            let label = format!(
                "Q{} ({} 20%)",
                i + 1,
                ["Poorest", "Lower", "Middle", "Upper", "Richest"][i]
            );
            println!(
                "{:<25} {:>15.1} {:>15.1}",
                label,
                progressive_summary.avg_fee_by_quintile[i],
                flat_summary.avg_fee_by_quintile[i]
            );
        }

        // Export CSVs
        // Create output directory if it doesn't exist
        fs::create_dir_all(&output_dir).expect("Failed to create output directory");

        let progressive_csv = progressive_result.metrics.to_csv();
        let flat_csv = flat_result.metrics.to_csv();

        let progressive_path = format!("{}/gini_progressive.csv", output_dir);
        let flat_path = format!("{}/gini_flat.csv", output_dir);

        fs::write(&progressive_path, &progressive_csv).expect("Failed to write progressive CSV");
        fs::write(&flat_path, &flat_csv).expect("Failed to write flat CSV");

        println!("\nCSV files exported:");
        println!("  {progressive_path}");
        println!("  {flat_path}");

        // Also export a combined comparison CSV
        let mut combined_csv = String::new();
        combined_csv.push_str("round,gini_progressive,gini_flat\n");

        let prog_snapshots = &progressive_result.metrics.snapshots;
        let flat_snapshots = &flat_result.metrics.snapshots;

        for i in 0..prog_snapshots.len().min(flat_snapshots.len()) {
            combined_csv.push_str(&format!(
                "{},{:.6},{:.6}\n",
                prog_snapshots[i].round,
                prog_snapshots[i].gini_coefficient,
                flat_snapshots[i].gini_coefficient,
            ));
        }

        let combined_path = format!("{}/gini_comparison.csv", output_dir);
        fs::write(&combined_path, &combined_csv).expect("Failed to write combined CSV");
        println!("  {combined_path}");

        println!("\nTo plot results, run:");
        println!("  python3 cluster-tax/scripts/plot_gini.py {output_dir}");
    }

    fn run_privacy_simulation(
        num_simulations: usize,
        pool_size: usize,
        standard_fraction: f64,
        exchange_fraction: f64,
        whale_fraction: f64,
        decay_rate_pct: f64,
        cluster_aware: bool,
        min_similarity: f64,
        use_parallel: bool,
        show_progress: bool,
    ) {
        use bth_cluster_tax::simulation::privacy::{
            format_monte_carlo_report, run_monte_carlo, run_monte_carlo_parallel,
            MonteCarloConfig, PoolConfig, RingSimConfig, RING_SIZE,
        };

        // Normalize fractions
        let total_specified = standard_fraction + exchange_fraction + whale_fraction;
        let coinbase_fraction = 0.10;
        let mixed_fraction = 1.0 - total_specified - coinbase_fraction;

        let pool_config = PoolConfig {
            pool_size,
            standard_fraction,
            exchange_fraction,
            whale_fraction,
            coinbase_fraction,
            mixed_fraction: mixed_fraction.max(0.0),
            num_clusters: 1_000,
            decay_rate: decay_rate_pct / 100.0,
            max_age_blocks: 525_600,
        };

        let ring_config = RingSimConfig {
            ring_size: RING_SIZE,
            min_cluster_similarity: min_similarity,
            cluster_aware_selection: cluster_aware,
        };

        let config = MonteCarloConfig {
            num_simulations,
            pool_config,
            ring_config,
        };

        let num_threads = rayon::current_num_threads();
        println!("Running privacy simulation...");
        println!("  Simulations: {num_simulations}");
        println!("  Pool size: {pool_size}");
        println!("  Standard tx fraction: {:.0}%", standard_fraction * 100.0);
        println!("  Decay rate: {decay_rate_pct}% per hop");
        println!("  Cluster-aware selection: {cluster_aware}");
        println!(
            "  Min similarity threshold: {:.0}%",
            min_similarity * 100.0
        );
        if use_parallel {
            println!("  Parallel execution: {} threads", num_threads);
        } else {
            println!("  Parallel execution: disabled");
        }
        println!();

        let results = if use_parallel {
            run_monte_carlo_parallel(&config, show_progress)
        } else {
            let mut rng = rand::thread_rng();
            run_monte_carlo(&config, &mut rng)
        };

        println!("{}", format_monte_carlo_report(&results));

        // Print interpretation
        println!("\nINTERPRETATION:");
        println!("───────────────────────────────────────────────────────────────────");

        if let Some(combined_stats) = results.bits_of_privacy_stats.get("Combined") {
            let mean_bits = combined_stats.mean;
            let median_bits = combined_stats.median;
            let worst_case = combined_stats.percentile_5;

            println!("Against a sophisticated adversary using both age and cluster heuristics:");
            println!();
            println!(
                "  • Average privacy:   {:.2} bits ({:.1} effective ring members)",
                mean_bits,
                2.0_f64.powf(mean_bits)
            );
            println!(
                "  • Median privacy:    {:.2} bits ({:.1} effective ring members)",
                median_bits,
                2.0_f64.powf(median_bits)
            );
            println!(
                "  • Worst case (5th%): {:.2} bits ({:.1} effective ring members)",
                worst_case,
                2.0_f64.powf(worst_case)
            );
            println!();

            let max_bits = (RING_SIZE as f64).log2();
            let efficiency = mean_bits / max_bits * 100.0;
            println!(
                "  • Privacy efficiency: {:.1}% of theoretical maximum ({:.2} bits)",
                efficiency, max_bits
            );

            if let Some(id_rate) = results.identified_rate.get("Combined") {
                println!(
                    "  • Identification rate: {:.1}% (adversary guesses correctly as #1 suspect)",
                    id_rate * 100.0
                );
            }

            println!();
            println!("For comparison:");
            if let Some(naive_stats) = results.bits_of_privacy_stats.get("Naive") {
                println!("  • Perfect (naive): {:.2} bits", naive_stats.mean);
            }
            if let Some(age_stats) = results.bits_of_privacy_stats.get("Age-Heuristic") {
                println!("  • Age-only attack: {:.2} bits", age_stats.mean);
            }
            if let Some(cluster_stats) = results.bits_of_privacy_stats.get("Cluster-Fingerprint") {
                println!("  • Cluster-only attack: {:.2} bits", cluster_stats.mean);
            }
        }

        println!();
        println!(
            "Note: Higher bits = better privacy. Theoretical max for ring size 7 is 2.81 bits."
        );
    }

    fn run_ring_size_analysis(sizes_str: &str, simulate: bool, num_sims: usize, pool_size: usize) {
        use bth_cluster_tax::simulation::privacy::{
            analyze_ring_sizes, format_ring_size_analysis, run_monte_carlo_parallel,
            MonteCarloConfig, PoolConfig, RingSimConfig,
        };

        // Parse ring sizes
        let sizes: Vec<usize> = sizes_str
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .filter(|&s| s >= 3 && s <= 31)
            .collect();

        if sizes.is_empty() {
            eprintln!("Error: No valid ring sizes provided. Use odd numbers between 3 and 31.");
            return;
        }

        // Run cost analysis
        let mut analyses = analyze_ring_sizes(&sizes);
        println!("{}", format_ring_size_analysis(&analyses));

        // Optionally run simulations for each ring size
        if simulate {
            let num_threads = rayon::current_num_threads();
            println!("\nRUNNING PRIVACY SIMULATIONS (parallel, {} threads)\n", num_threads);
            println!(
                "─────────────────────────────────────────────────────────────────────────────────"
            );
            println!("Ring   Theoretical   Measured    Efficiency   Cluster      ID Rate");
            println!("Size   Max (bits)    (bits)      (%)          Leakage      (Combined)");
            println!(
                "─────────────────────────────────────────────────────────────────────────────────"
            );

            for analysis in &mut analyses {
                let pool_config = PoolConfig {
                    pool_size,
                    ..Default::default()
                };

                let ring_config = RingSimConfig {
                    ring_size: analysis.ring_size,
                    min_cluster_similarity: 0.70,
                    cluster_aware_selection: true,
                };

                let config = MonteCarloConfig {
                    num_simulations: num_sims,
                    pool_config,
                    ring_config,
                };

                // Use parallel version with progress bar
                let results = run_monte_carlo_parallel(&config, true);

                if let Some(combined_stats) = results.bits_of_privacy_stats.get("Combined") {
                    let measured = combined_stats.mean;
                    let theoretical = analysis.theoretical_max_bits;
                    let efficiency = (measured / theoretical) * 100.0;
                    let leakage = theoretical - measured;
                    let id_rate = results
                        .identified_rate
                        .get("Combined")
                        .copied()
                        .unwrap_or(0.0);

                    analysis.measured_bits = Some(measured);
                    analysis.measured_efficiency = Some(efficiency);

                    println!(
                        "{:>4}   {:>6.2}        {:>6.2}      {:>5.1}%       {:>5.2}        {:>5.1}%",
                        analysis.ring_size,
                        theoretical,
                        measured,
                        efficiency,
                        leakage,
                        id_rate * 100.0
                    );
                }
            }

            // Summary and recommendation
            println!("\n");
            println!(
                "─────────────────────────────────────────────────────────────────────────────────"
            );
            println!("ANALYSIS SUMMARY");
            println!(
                "─────────────────────────────────────────────────────────────────────────────────"
            );

            // Find the sweet spot (best bits per KB)
            let best_efficiency = analyses
                .iter()
                .max_by(|a, b| a.bits_per_kb.partial_cmp(&b.bits_per_kb).unwrap())
                .unwrap();

            println!(
                "\nBest bits-per-KB efficiency: Ring size {} ({:.3} bits/KB)",
                best_efficiency.ring_size, best_efficiency.bits_per_kb
            );

            // Compare ring 7 to alternatives
            if let Some(ring7) = analyses.iter().find(|a| a.ring_size == 7) {
                println!("\nWhy ring size 7 is the sweet spot:");
                println!();

                // Compare to smaller
                if let Some(ring5) = analyses.iter().find(|a| a.ring_size == 5) {
                    let size_saved = ring7.signature_bytes - ring5.signature_bytes;
                    let privacy_lost = ring7.theoretical_max_bits - ring5.theoretical_max_bits;
                    println!(
                        "  vs Ring 5: +{:.1} KB (+{:.0}%) for +{:.2} bits (+{:.0}% privacy)",
                        size_saved as f64 / 1024.0,
                        (size_saved as f64 / ring5.signature_bytes as f64) * 100.0,
                        privacy_lost,
                        (privacy_lost / ring5.theoretical_max_bits) * 100.0
                    );
                }

                // Compare to larger
                for &compare_size in &[9, 11, 13] {
                    if let Some(larger) = analyses.iter().find(|a| a.ring_size == compare_size) {
                        let size_cost = larger.signature_bytes - ring7.signature_bytes;
                        let privacy_gain = larger.theoretical_max_bits - ring7.theoretical_max_bits;
                        println!("  vs Ring {}: +{:.1} KB (+{:.0}%) for only +{:.2} bits (+{:.0}% privacy)",
                            compare_size,
                            size_cost as f64 / 1024.0,
                            (size_cost as f64 / ring7.signature_bytes as f64) * 100.0,
                            privacy_gain,
                            (privacy_gain / ring7.theoretical_max_bits) * 100.0);
                    }
                }

                println!();
                println!(
                    "Ring 7 provides {} of {} theoretical bits ({:.1}% efficiency)",
                    ring7
                        .measured_bits
                        .map(|b| format!("{:.2}", b))
                        .unwrap_or("N/A".to_string()),
                    format!("{:.2}", ring7.theoretical_max_bits),
                    ring7.measured_efficiency.unwrap_or(0.0)
                );
            }
        } else {
            println!("\nRun with --simulate to measure actual privacy for each ring size.");
        }
    }

    fn run_decay_comparison(
        wealth: u64,
        hop_decay_pct: f64,
        half_life_blocks: u64,
        wash_txs: usize,
        blocks_elapsed: u64,
    ) {
        use bth_cluster_tax::{
            BlockAwareTagVector, BlockDecayConfig, ClusterId, FeeCurve, TagVector, TAG_WEIGHT_SCALE,
        };

        println!("╔══════════════════════════════════════════════════════════════════╗");
        println!("║        DECAY MECHANISM COMPARISON: Block vs Hop                  ║");
        println!("╠══════════════════════════════════════════════════════════════════╣");
        println!("║  Cluster Wealth: {:>12}                                    ║", wealth);
        println!("║  Hop Decay Rate: {:>5.1}% per transfer                           ║", hop_decay_pct);
        println!("║  Block Half-Life: {:>6} blocks (~{:.1} days @ 10s/block)        ║",
            half_life_blocks, half_life_blocks as f64 / 8640.0);
        println!("║  Wash Trading Simulation: {} txs in {} blocks                   ║",
            wash_txs, blocks_elapsed);
        println!("╚══════════════════════════════════════════════════════════════════╝");
        println!();

        let cluster = ClusterId::new(1);
        let fee_curve = FeeCurve::default_params();

        // Initial fee rate
        let initial_rate = fee_curve.rate_bps(wealth);

        // ============================================================
        // Scenario 1: Hop-based decay (current design)
        // ============================================================
        let hop_decay_rate = (hop_decay_pct / 100.0 * TAG_WEIGHT_SCALE as f64) as u32;
        let mut hop_tags = TagVector::single(cluster);

        // Simulate wash trading: N self-transfers
        for _ in 0..wash_txs {
            hop_tags.apply_decay(hop_decay_rate);
        }

        let hop_remaining = hop_tags.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;
        let hop_background = hop_tags.background() as f64 / TAG_WEIGHT_SCALE as f64;

        // Calculate effective cluster wealth after wash trading
        let hop_effective_wealth = (wealth as f64 * hop_remaining) as u64;
        let hop_final_rate = fee_curve.rate_bps(hop_effective_wealth);

        // ============================================================
        // Scenario 2: Block-based decay (new design)
        // ============================================================
        let block_config = BlockDecayConfig {
            half_life_blocks,
            min_decay_interval: 1,
            hop_decay_rate: 0,
        };

        let mut block_tags = BlockAwareTagVector::single(cluster, 0);

        // Simulate same wash trading: N self-transfers in `blocks_elapsed` blocks
        // With block decay, txs don't accelerate decay!
        block_tags.apply_block_decay(blocks_elapsed, &block_config);

        let block_remaining = block_tags.get_raw(cluster) as f64 / TAG_WEIGHT_SCALE as f64;
        let block_background = 1.0 - block_remaining;

        let block_effective_wealth = (wealth as f64 * block_remaining) as u64;
        let block_final_rate = fee_curve.rate_bps(block_effective_wealth);

        // ============================================================
        // Results
        // ============================================================
        println!("WASH TRADING RESISTANCE COMPARISON");
        println!("─────────────────────────────────────────────────────────────────────");
        println!("{:<25} {:>15} {:>15}", "Metric", "Hop-Based", "Block-Based");
        println!("─────────────────────────────────────────────────────────────────────");
        println!("{:<25} {:>14.1}% {:>14.1}%", "Cluster Tag Remaining",
            hop_remaining * 100.0, block_remaining * 100.0);
        println!("{:<25} {:>14.1}% {:>14.1}%", "Background (Anonymous)",
            hop_background * 100.0, block_background * 100.0);
        println!("{:<25} {:>13} bps {:>12} bps", "Initial Fee Rate",
            initial_rate, initial_rate);
        println!("{:<25} {:>13} bps {:>12} bps", "Final Fee Rate",
            hop_final_rate, block_final_rate);
        println!("{:<25} {:>14.1}% {:>14.1}%", "Fee Rate Reduction",
            (1.0 - hop_final_rate as f64 / initial_rate as f64) * 100.0,
            (1.0 - block_final_rate as f64 / initial_rate as f64) * 100.0);
        println!("─────────────────────────────────────────────────────────────────────");

        // Economic analysis
        let hop_savings_pct = (initial_rate - hop_final_rate) as f64 / initial_rate as f64 * 100.0;
        let block_savings_pct = (initial_rate - block_final_rate) as f64 / initial_rate as f64 * 100.0;

        println!();
        println!("ECONOMIC ANALYSIS");
        println!("─────────────────────────────────────────────────────────────────────");

        if hop_savings_pct > 1.0 {
            println!("⚠  HOP-BASED: Wash trading reduces fees by {:.1}%", hop_savings_pct);
            println!("   After {} self-transfers, whale pays {:.1}x less in fees",
                wash_txs, initial_rate as f64 / hop_final_rate as f64);
        } else {
            println!("✓  HOP-BASED: Wash trading ineffective ({:.1}% savings)", hop_savings_pct);
        }

        if block_savings_pct > 1.0 {
            println!("⚠  BLOCK-BASED: Time decay reduces fees by {:.1}%", block_savings_pct);
        } else {
            println!("✓  BLOCK-BASED: Wash trading completely ineffective");
            println!("   {} self-transfers provide {:.2}% fee reduction", wash_txs, block_savings_pct);
        }

        println!();
        println!("RECOMMENDATION");
        println!("─────────────────────────────────────────────────────────────────────");

        if block_savings_pct < hop_savings_pct / 10.0 {
            println!("✓  Block-based decay is {:.0}x more resistant to wash trading",
                hop_savings_pct / block_savings_pct.max(0.01));
            println!("   Switch to block-based decay with half-life of {} blocks", half_life_blocks);
        } else {
            println!("   Both mechanisms show similar wash trading resistance");
            println!("   Consider increasing block half-life for better protection");
        }

        // Sweep analysis
        println!();
        println!("SENSITIVITY ANALYSIS: Wash Trading at Different Scales");
        println!("─────────────────────────────────────────────────────────────────────");
        println!("{:>6} {:>12} {:>12} {:>12} {:>12}",
            "TXs", "Hop Remain", "Hop Fee", "Block Remain", "Block Fee");
        println!("─────────────────────────────────────────────────────────────────────");

        for &n_txs in &[10, 50, 100, 200, 500, 1000] {
            // Hop decay
            let mut tags = TagVector::single(cluster);
            for _ in 0..n_txs {
                tags.apply_decay(hop_decay_rate);
            }
            let h_remain = tags.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;
            let h_rate = fee_curve.rate_bps((wealth as f64 * h_remain) as u64);

            // Block decay (same time window, proportional blocks)
            let elapsed = blocks_elapsed * n_txs as u64 / wash_txs.max(1) as u64;
            let factor = block_config.decay_factor(elapsed);
            let b_remain = factor as f64 / TAG_WEIGHT_SCALE as f64;
            let b_rate = fee_curve.rate_bps((wealth as f64 * b_remain) as u64);

            println!("{:>6} {:>11.1}% {:>10} bps {:>11.1}% {:>10} bps",
                n_txs, h_remain * 100.0, h_rate, b_remain * 100.0, b_rate);
        }

        println!("─────────────────────────────────────────────────────────────────────");
        println!();
        println!("Note: Block-based decay resists wash trading because time passes at a fixed rate.");
        println!("      Making more transactions does NOT accelerate tag decay.");
    }

    fn run_decay_comparison_all(
        wealth: u64,
        hop_decay_pct: f64,
        half_life_blocks: u64,
        min_blocks_between: u64,
        wash_txs: usize,
        blocks_elapsed: u64,
    ) {
        use bth_cluster_tax::TagVector;

        println!("╔══════════════════════════════════════════════════════════════════════════════╗");
        println!("║            THREE-WAY DECAY MECHANISM COMPARISON                               ║");
        println!("║         Hop-Based vs Block-Based vs Rate-Limited Hybrid                       ║");
        println!("╠══════════════════════════════════════════════════════════════════════════════╣");
        println!("║  Cluster Wealth:    {:>12}                                               ║", wealth);
        println!("║  Hop Decay Rate:    {:>5.1}% per transfer                                      ║", hop_decay_pct);
        println!("║  Block Half-Life:   {:>6} blocks (~{:.1} days @ 10s/block)                   ║",
            half_life_blocks, half_life_blocks as f64 / 8640.0);
        println!("║  Rate Limit:        {:>6} blocks (~{:.1} hours between eligible decays)      ║",
            min_blocks_between, min_blocks_between as f64 / 360.0);
        println!("║  Wash Trading Sim:  {} txs in {} blocks                                      ║",
            wash_txs, blocks_elapsed);
        println!("╚══════════════════════════════════════════════════════════════════════════════╝");
        println!();

        let cluster = ClusterId::new(1);
        let fee_curve = FeeCurve::default_params();
        let initial_rate = fee_curve.rate_bps(wealth);
        let hop_decay_rate = (hop_decay_pct / 100.0 * TAG_WEIGHT_SCALE as f64) as u32;

        // ============================================================
        // Model 1: Pure Hop-Based Decay (current design)
        // ============================================================
        let mut hop_tags = TagVector::single(cluster);
        for _ in 0..wash_txs {
            hop_tags.apply_decay(hop_decay_rate);
        }
        let hop_remaining = hop_tags.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;
        let hop_effective_wealth = (wealth as f64 * hop_remaining) as u64;
        let hop_final_rate = fee_curve.rate_bps(hop_effective_wealth);

        // ============================================================
        // Model 2: Pure Block-Based Decay (time-only)
        // ============================================================
        let block_config = BlockDecayConfig {
            half_life_blocks,
            min_decay_interval: 1,
            hop_decay_rate: 0,
        };
        let mut block_tags = BlockAwareTagVector::single(cluster, 0);
        block_tags.apply_block_decay(blocks_elapsed, &block_config);
        let block_remaining = block_tags.get_raw(cluster) as f64 / TAG_WEIGHT_SCALE as f64;
        let block_effective_wealth = (wealth as f64 * block_remaining) as u64;
        let block_final_rate = fee_curve.rate_bps(block_effective_wealth);

        // ============================================================
        // Model 3: Rate-Limited Hop Decay (hybrid)
        // ============================================================
        let rate_config = RateLimitedDecayConfig {
            decay_rate_per_hop: hop_decay_rate,
            min_blocks_between_decays: min_blocks_between,
            passive_half_life_blocks: None,
        };
        let mut rate_tags = RateLimitedTagVector::single(cluster, 0);

        // Simulate wash trading: spread N txs over blocks_elapsed blocks
        // Each tx occurs at a proportional block number
        let mut eligible_decays = 0;
        for i in 0..wash_txs {
            let tx_block = (i as u64 * blocks_elapsed) / wash_txs.max(1) as u64;
            if rate_tags.try_apply_hop_decay(tx_block, &rate_config) {
                eligible_decays += 1;
            }
        }

        let rate_remaining = rate_tags.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;
        let rate_effective_wealth = (wealth as f64 * rate_remaining) as u64;
        let rate_final_rate = fee_curve.rate_bps(rate_effective_wealth);

        // ============================================================
        // Results Comparison
        // ============================================================
        println!("WASH TRADING RESISTANCE COMPARISON");
        println!("────────────────────────────────────────────────────────────────────────────────");
        println!("{:<30} {:>15} {:>15} {:>15}", "Metric", "Hop-Based", "Block-Based", "Rate-Limited");
        println!("────────────────────────────────────────────────────────────────────────────────");
        println!("{:<30} {:>14.2}% {:>14.2}% {:>14.2}%", "Cluster Tag Remaining",
            hop_remaining * 100.0, block_remaining * 100.0, rate_remaining * 100.0);
        println!("{:<30} {:>13} bps {:>12} bps {:>12} bps", "Initial Fee Rate",
            initial_rate, initial_rate, initial_rate);
        println!("{:<30} {:>13} bps {:>12} bps {:>12} bps", "Final Fee Rate",
            hop_final_rate, block_final_rate, rate_final_rate);

        let hop_reduction = (1.0 - hop_final_rate as f64 / initial_rate as f64) * 100.0;
        let block_reduction = (1.0 - block_final_rate as f64 / initial_rate as f64) * 100.0;
        let rate_reduction = (1.0 - rate_final_rate as f64 / initial_rate as f64) * 100.0;

        println!("{:<30} {:>14.1}% {:>14.1}% {:>14.1}%", "Fee Rate Reduction",
            hop_reduction, block_reduction, rate_reduction);
        println!("{:<30} {:>15} {:>15} {:>15}", "Eligible Decay Events",
            wash_txs, "N/A (time)", eligible_decays);
        println!("────────────────────────────────────────────────────────────────────────────────");

        // Interpretation
        println!();
        println!("ANALYSIS");
        println!("────────────────────────────────────────────────────────────────────────────────");

        // Hop-based
        if hop_reduction > 10.0 {
            println!("⚠  HOP-BASED: Vulnerable to wash trading ({:.1}% fee reduction)", hop_reduction);
            println!("   {} self-transfers reduce fees by {:.1}x", wash_txs,
                initial_rate as f64 / hop_final_rate.max(1) as f64);
        } else {
            println!("✓  HOP-BASED: Wash trading ineffective ({:.1}% reduction)", hop_reduction);
        }

        // Block-based
        if block_reduction < 1.0 {
            println!("✓  BLOCK-BASED: Completely wash-trading resistant");
            println!("   Only time affects decay, not transaction count");
        } else {
            println!("○  BLOCK-BASED: {:.1}% natural decay over {} blocks", block_reduction, blocks_elapsed);
        }

        // Rate-limited
        let max_possible_decays = blocks_elapsed / min_blocks_between.max(1);
        println!("○  RATE-LIMITED: {} of {} possible decay events triggered",
            eligible_decays, max_possible_decays);
        if rate_reduction < hop_reduction / 2.0 {
            println!("✓  Rate limiting reduced attack effectiveness by {:.1}x",
                hop_reduction / rate_reduction.max(0.01));
        }

        // Sweep: Different wash trading intensities
        println!();
        println!("SENSITIVITY: Wash Trading at Different Intensities");
        println!("────────────────────────────────────────────────────────────────────────────────");
        println!("{:>8} {:>12} {:>12} {:>12} {:>12}",
            "TXs", "Hop Remain", "Block Remain", "Rate Remain", "Rate Decays");
        println!("────────────────────────────────────────────────────────────────────────────────");

        for &n_txs in &[10, 50, 100, 500, 1000, 5000] {
            // Hop decay
            let mut hop_t = TagVector::single(cluster);
            for _ in 0..n_txs {
                hop_t.apply_decay(hop_decay_rate);
            }
            let h_remain = hop_t.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

            // Block decay (fixed time window)
            let b_remain = block_remaining; // Same for all - only depends on time

            // Rate-limited decay
            let mut rate_t = RateLimitedTagVector::single(cluster, 0);
            let mut decays = 0;
            for i in 0..n_txs {
                let tx_block = (i as u64 * blocks_elapsed) / n_txs.max(1) as u64;
                if rate_t.try_apply_hop_decay(tx_block, &rate_config) {
                    decays += 1;
                }
            }
            let r_remain = rate_t.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

            println!("{:>8} {:>11.2}% {:>11.2}% {:>11.2}% {:>12}",
                n_txs, h_remain * 100.0, b_remain * 100.0, r_remain * 100.0, decays);
        }

        // Recommendation
        println!();
        println!("RECOMMENDATION");
        println!("────────────────────────────────────────────────────────────────────────────────");

        if block_reduction < rate_reduction && block_reduction < hop_reduction {
            println!("✓  BLOCK-BASED decay is most resistant to wash trading");
            println!("   No transaction can accelerate decay - only time matters");
        } else if rate_reduction < hop_reduction {
            println!("✓  RATE-LIMITED HYBRID is a good compromise:");
            println!("   • Keeps intuitive 'decay per hop' semantics");
            println!("   • Limits max decay rate to 1 per {} blocks (~{:.1} hours)",
                min_blocks_between, min_blocks_between as f64 / 360.0);
            println!("   • {:.0}x more wash-trading resistant than pure hop-based",
                hop_reduction / rate_reduction.max(0.01));
        } else {
            println!("⚠  All mechanisms show similar behavior in this scenario");
            println!("   Consider adjusting parameters for better differentiation");
        }

        println!();
        println!("Trade-offs:");
        println!("  • Block-based: Simplest, most resistant, but tags decay even without trading");
        println!("  • Rate-limited: Keeps hop semantics, resistant to wash trading, slightly complex");
        println!("  • Hop-based: Most intuitive, but vulnerable to wash trading attacks");
    }

    fn run_decay_comparison_four(
        wealth: u64,
        hop_decay_pct: f64,
        half_life_blocks: u64,
        min_blocks_between: u64,
        max_per_day: u32,
        wash_txs: usize,
        blocks_elapsed: u64,
    ) {
        use bth_cluster_tax::TagVector;

        println!("╔══════════════════════════════════════════════════════════════════════════════════════╗");
        println!("║                    FOUR-WAY DECAY MECHANISM COMPARISON                                ║");
        println!("║          Hop-Based vs Block-Based vs Rate-Limited vs AND-Based                        ║");
        println!("╠══════════════════════════════════════════════════════════════════════════════════════╣");
        println!("║  Cluster Wealth:       {:>12}                                                      ║", wealth);
        println!("║  Hop Decay Rate:       {:>5.1}% per transfer                                            ║", hop_decay_pct);
        println!("║  Block Half-Life:      {:>6} blocks (~{:.1} days)                                       ║",
            half_life_blocks, half_life_blocks as f64 / 8640.0);
        println!("║  Min Blocks Between:   {:>6} blocks (~{:.1} hours)                                      ║",
            min_blocks_between, min_blocks_between as f64 / 360.0);
        println!("║  Max Decays/Day:       {:>6} (AND model epoch cap)                                     ║", max_per_day);
        println!("║  Simulation:           {} txs over {} blocks (~{:.1} days)                              ║",
            wash_txs, blocks_elapsed, blocks_elapsed as f64 / 8640.0);
        println!("╚══════════════════════════════════════════════════════════════════════════════════════╝");
        println!();

        let cluster = ClusterId::new(1);
        let fee_curve = FeeCurve::default_params();
        let initial_rate = fee_curve.rate_bps(wealth);
        let hop_decay_rate = (hop_decay_pct / 100.0 * TAG_WEIGHT_SCALE as f64) as u32;

        // ============================================================
        // SCENARIO 1: RAPID WASH TRADING (all txs in short time)
        // ============================================================
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("SCENARIO 1: RAPID WASH TRADING ({} txs in {} blocks)", wash_txs, blocks_elapsed.min(100));
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        let rapid_blocks = blocks_elapsed.min(100);

        // Model 1: Hop-based
        let mut hop_tags = TagVector::single(cluster);
        for _ in 0..wash_txs {
            hop_tags.apply_decay(hop_decay_rate);
        }
        let hop_remain = hop_tags.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

        // Model 2: Block-based
        let block_config = BlockDecayConfig {
            half_life_blocks,
            min_decay_interval: 1,
            hop_decay_rate: 0,
        };
        let mut block_tags = BlockAwareTagVector::single(cluster, 0);
        block_tags.apply_block_decay(rapid_blocks, &block_config);
        let block_remain = block_tags.get_raw(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

        // Model 3: Rate-limited
        let rate_config = RateLimitedDecayConfig {
            decay_rate_per_hop: hop_decay_rate,
            min_blocks_between_decays: min_blocks_between,
            passive_half_life_blocks: None,
        };
        let mut rate_tags = RateLimitedTagVector::single(cluster, 0);
        let mut rate_decays = 0;
        for i in 0..wash_txs {
            let tx_block = (i as u64 * rapid_blocks) / wash_txs.max(1) as u64;
            if rate_tags.try_apply_hop_decay(tx_block, &rate_config) {
                rate_decays += 1;
            }
        }
        let rate_remain = rate_tags.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

        // Model 4: AND-based
        let and_config = AndDecayConfig {
            decay_rate_per_hop: hop_decay_rate,
            min_blocks_between_decays: min_blocks_between,
            max_decays_per_epoch: max_per_day,
            epoch_blocks: 8_640,
        };
        let mut and_tags = AndTagVector::single(cluster, 0);
        let mut and_decays = 0;
        for i in 0..wash_txs {
            let tx_block = (i as u64 * rapid_blocks) / wash_txs.max(1) as u64;
            if and_tags.try_apply_decay_on_transfer(tx_block, &and_config) {
                and_decays += 1;
            }
        }
        let and_remain = and_tags.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

        println!("{:<20} {:>12} {:>12} {:>12} {:>12}", "Metric", "Hop-Based", "Block-Based", "Rate-Ltd", "AND-Based");
        println!("────────────────────────────────────────────────────────────────────────────────────────");
        println!("{:<20} {:>11.2}% {:>11.2}% {:>11.2}% {:>11.2}%", "Tag Remaining",
            hop_remain * 100.0, block_remain * 100.0, rate_remain * 100.0, and_remain * 100.0);
        println!("{:<20} {:>12} {:>12} {:>12} {:>12}", "Decay Events",
            wash_txs, "N/A", rate_decays, and_decays);

        // ============================================================
        // SCENARIO 2: PATIENT WASH TRADING (spaced out over time)
        // ============================================================
        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("SCENARIO 2: PATIENT WASH TRADING ({} txs over {} blocks = {:.1} days)",
            wash_txs, blocks_elapsed, blocks_elapsed as f64 / 8640.0);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        // Model 1: Hop-based (same result)
        let mut hop_tags2 = TagVector::single(cluster);
        for _ in 0..wash_txs {
            hop_tags2.apply_decay(hop_decay_rate);
        }
        let hop_remain2 = hop_tags2.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

        // Model 2: Block-based
        let mut block_tags2 = BlockAwareTagVector::single(cluster, 0);
        block_tags2.apply_block_decay(blocks_elapsed, &block_config);
        let block_remain2 = block_tags2.get_raw(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

        // Model 3: Rate-limited
        let mut rate_tags2 = RateLimitedTagVector::single(cluster, 0);
        let mut rate_decays2 = 0;
        for i in 0..wash_txs {
            let tx_block = (i as u64 * blocks_elapsed) / wash_txs.max(1) as u64;
            if rate_tags2.try_apply_hop_decay(tx_block, &rate_config) {
                rate_decays2 += 1;
            }
        }
        let rate_remain2 = rate_tags2.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

        // Model 4: AND-based
        let mut and_tags2 = AndTagVector::single(cluster, 0);
        let mut and_decays2 = 0;
        for i in 0..wash_txs {
            let tx_block = (i as u64 * blocks_elapsed) / wash_txs.max(1) as u64;
            if and_tags2.try_apply_decay_on_transfer(tx_block, &and_config) {
                and_decays2 += 1;
            }
        }
        let and_remain2 = and_tags2.get(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

        println!("{:<20} {:>12} {:>12} {:>12} {:>12}", "Metric", "Hop-Based", "Block-Based", "Rate-Ltd", "AND-Based");
        println!("────────────────────────────────────────────────────────────────────────────────────────");
        println!("{:<20} {:>11.2}% {:>11.2}% {:>11.2}% {:>11.2}%", "Tag Remaining",
            hop_remain2 * 100.0, block_remain2 * 100.0, rate_remain2 * 100.0, and_remain2 * 100.0);
        println!("{:<20} {:>12} {:>12} {:>12} {:>12}", "Decay Events",
            wash_txs, "N/A", rate_decays2, and_decays2);

        // ============================================================
        // SCENARIO 3: HOLDING WITHOUT TRADING (key differentiator!)
        // ============================================================
        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("SCENARIO 3: HOLDING WITHOUT TRADING (0 txs over {} blocks = {:.1} days)",
            blocks_elapsed, blocks_elapsed as f64 / 8640.0);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        // Model 1: Hop-based - NO transactions = NO decay
        let hop_remain3 = 100.0; // No decay without hops

        // Model 2: Block-based - TIME causes decay
        let mut block_tags3 = BlockAwareTagVector::single(cluster, 0);
        block_tags3.apply_block_decay(blocks_elapsed, &block_config);
        let block_remain3 = block_tags3.get_raw(cluster) as f64 / TAG_WEIGHT_SCALE as f64;

        // Model 3: Rate-limited - NO transactions = NO decay
        let rate_remain3 = 100.0; // No decay without hops

        // Model 4: AND-based - NO transactions = NO decay
        let and_remain3 = 100.0; // No decay without hops

        println!("{:<20} {:>12} {:>12} {:>12} {:>12}", "Metric", "Hop-Based", "Block-Based", "Rate-Ltd", "AND-Based");
        println!("────────────────────────────────────────────────────────────────────────────────────────");
        println!("{:<20} {:>11.2}% {:>11.2}% {:>11.2}% {:>11.2}%", "Tag Remaining",
            hop_remain3, block_remain3 * 100.0, rate_remain3, and_remain3);
        println!("{:<20} {:>12} {:>12} {:>12} {:>12}", "Passive Decay?",
            "NO", "YES", "NO", "NO");

        // ============================================================
        // ANALYSIS & RECOMMENDATION
        // ============================================================
        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("ANALYSIS");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();

        println!("Model Comparison:");
        println!("  • HOP-BASED:   Rapid={:.1}%, Patient={:.1}%, Holding=100%",
            hop_remain * 100.0, hop_remain2 * 100.0);
        println!("  • BLOCK-BASED: Rapid={:.1}%, Patient={:.1}%, Holding={:.1}%",
            block_remain * 100.0, block_remain2 * 100.0, block_remain3 * 100.0);
        println!("  • RATE-LTD:    Rapid={:.1}%, Patient={:.1}%, Holding=100%",
            rate_remain * 100.0, rate_remain2 * 100.0);
        println!("  • AND-BASED:   Rapid={:.1}%, Patient={:.1}%, Holding=100%",
            and_remain * 100.0, and_remain2 * 100.0);

        println!();
        println!("Key Insights:");
        if hop_remain < 10.0 {
            println!("  ❌ HOP-BASED: Vulnerable to rapid wash trading ({:.1}% remaining)", hop_remain);
        }
        if block_remain3 < 50.0 {
            println!("  ⚠️  BLOCK-BASED: Passive decay gives 'free' tax reduction over time");
        }
        if rate_remain < 50.0 && rate_remain2 < 10.0 {
            println!("  ⚠️  RATE-LIMITED: Patient attackers can still decay to {:.1}%", rate_remain2);
        }

        // Check AND-based with epoch cap
        let max_decays_possible = max_per_day as u64 * (blocks_elapsed / 8640 + 1);
        let decay_with_cap = (1.0 - hop_decay_pct / 100.0).powi(max_decays_possible.min(and_decays2 as u64) as i32);
        println!();
        println!("  ✓ AND-BASED advantages:");
        println!("    • Requires BOTH time AND transfers for decay");
        println!("    • Holding without trading: NO decay (wealthy must wash trade)");
        println!("    • Epoch cap limits max decay to {} per day", max_per_day);
        println!("    • Over {:.1} days: max {} decays = {:.1}% remaining",
            blocks_elapsed as f64 / 8640.0, max_decays_possible.min(and_decays2 as u64), decay_with_cap * 100.0);

        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("RECOMMENDATION: AND-BASED with Epoch Cap");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();
        println!("The AND-based model with epoch cap provides:");
        println!("  1. ✓ Rapid wash trading resistance (rate-limited)");
        println!("  2. ✓ Patient wash trading bounded (epoch cap)");
        println!("  3. ✓ Holding doesn't reduce tax (must transact)");
        println!("  4. ✓ Legitimate trading still enables privacy");
        println!();
        println!("Suggested parameters:");
        println!("  • decay_rate_per_hop: 5%");
        println!("  • min_blocks_between: 720 (~2 hours)");
        println!("  • max_decays_per_epoch: 12 per day");
        println!();
        println!("This gives: Max decay of {:.1}% per day, {:.1}% per week, {:.1}% per month",
            (1.0 - 0.05_f64.powi(12)) * 100.0,
            (1.0 - 0.95_f64.powi(84)) * 100.0,
            (1.0 - 0.95_f64.powi(360)) * 100.0);
    }
}

#[cfg(feature = "cli")]
fn main() {
    use clap::Parser;
    let cli = cli::Cli::parse();
    cli::run(cli);
}

#[cfg(not(feature = "cli"))]
fn main() {
    eprintln!("This binary requires the 'cli' feature. Build with:");
    eprintln!("  cargo build -p mc-cluster-tax --features cli --bin cluster-tax-sim");
}
