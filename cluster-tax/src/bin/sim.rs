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

        /// Compare all three decay mechanisms: hop-based, block-based,
        /// rate-limited hybrid
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

        /// Compare all four decay mechanisms including AND-based (time AND hop
        /// required)
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

            /// Minimum blocks between eligible decays (for rate-limited and AND
            /// models)
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

        /// Compare entropy-weighted decay vs age-based decay for wash trading
        /// resistance
        DecayEntropyCompare {
            /// Initial cluster wealth
            #[arg(long, default_value = "100000000")]
            initial_wealth: u64,

            /// Initial cluster factor (1.0 to 6.0)
            #[arg(long, default_value = "6.0")]
            initial_factor: f64,

            /// Duration in blocks (60480 = 1 week)
            #[arg(long, default_value = "60480")]
            duration_blocks: u64,

            /// Output results as JSON
            #[arg(long)]
            json: bool,
        },

        /// Test attack resistance against various strategies
        AttackResistance {
            /// Initial cluster wealth
            #[arg(long, default_value = "100000000")]
            initial_wealth: u64,

            /// Attack strategy to test
            #[arg(long, default_value = "patient-wash")]
            strategy: String,

            /// Duration in blocks
            #[arg(long, default_value = "60480")]
            duration_blocks: u64,

            /// For patient-wash: blocks between transfers
            #[arg(long, default_value = "720")]
            interval_blocks: u64,

            /// For sybil-wash: number of fake counterparties
            #[arg(long, default_value = "100")]
            fake_counterparties: u32,

            /// For partial-commerce: ratio of legitimate transactions
            #[arg(long, default_value = "0.5")]
            legit_ratio: f64,

            /// Output results as JSON
            #[arg(long)]
            json: bool,
        },

        /// Parameter sensitivity analysis for entropy-weighted decay
        EntropyParameterSweep {
            /// Initial cluster wealth
            #[arg(long, default_value = "100000000")]
            initial_wealth: u64,

            /// Duration in blocks
            #[arg(long, default_value = "60480")]
            duration_blocks: u64,

            /// Output results as JSON
            #[arg(long)]
            json: bool,
        },

        /// Parameter sweep for the combined progressive mechanism
        /// (asymmetric-fees-simulation.md): value-weighted lottery with floor,
        /// eligibility decay, and asymmetric structure fees. Measures Gini
        /// reduction vs a burn-only baseline plus attack profitability.
        LotterySweep {
            /// Simulation duration in blocks (default ~30 days at 20s blocks)
            #[arg(long, default_value = "129600")]
            blocks: u64,

            /// Transactions per block
            #[arg(long, default_value = "10")]
            txs_per_block: u32,

            /// Quick mode: single recommended config instead of full grid
            #[arg(long)]
            quick: bool,
        },

        /// Structural Gini-reduction experiment: tail emission routed to the
        /// lottery, uniform-per-UTXO payouts, cluster demurrage, and a
        /// strategic splitting whale (gamed equilibrium). Seven scenarios
        /// isolate each lever.
        LotteryExperiment {
            /// Simulation duration in blocks (default ~1 year at 20s blocks)
            #[arg(long, default_value = "1576800")]
            blocks: u64,

            /// Transactions per block
            #[arg(long, default_value = "1")]
            txs_per_block: u32,

            /// Base fee in base units (250 = 0.25 BTH)
            #[arg(long, default_value = "250")]
            base_fee: u64,

            /// Block emission routed to the lottery (base units;
            /// 1600/block ~= 2.5% of 100M BTH supply per year)
            #[arg(long, default_value = "1600")]
            emission_per_block: u64,

            /// Strategic whale split count
            #[arg(long, default_value = "1000")]
            split: u32,

            /// Churn interval in days for the strategic whale
            #[arg(long, default_value = "7")]
            churn_days: u64,

            /// Annual demurrage in basis points at max cluster factor
            /// (200 = 2%/year on factor-6 clusters, 0 on factor-1)
            #[arg(long, default_value = "200")]
            demurrage_bps: u32,

            /// Quick mode: ~20-day horizon for sanity checking
            #[arg(long)]
            quick: bool,
        },

        /// Emission-schedule sweep (issue #350): run a fixed grid of candidate
        /// MonetaryPolicy schedules (S1..S5) through the agent-based sim plus
        /// an analytic monetary model, and emit a neutral comparison
        /// report (markdown + CSV). Presents data only; recommends
        /// nothing.
        EmissionSweep {
            /// Number of simulated rounds for the distribution track
            #[arg(long, default_value = "4000")]
            rounds: u64,

            /// Blocks processed per round
            #[arg(long, default_value = "4")]
            blocks_per_round: u64,

            /// Number of retail users in the fixed agent population
            #[arg(long, default_value = "120")]
            retail: usize,

            /// Number of merchants
            #[arg(long, default_value = "12")]
            merchants: usize,

            /// Number of minters
            #[arg(long, default_value = "4")]
            minters: usize,

            /// Number of whales
            #[arg(long, default_value = "4")]
            whales: usize,

            /// Directory to write the report (markdown) and CSV artifacts
            #[arg(long, default_value = "experiments/results")]
            output: String,

            /// Quick mode: smaller/faster run for sanity checking
            #[arg(long)]
            quick: bool,
        },

        /// Decoy-quantile demurrage sweep (empirical gate for #577 / H2-B1):
        /// compare the value-weighted-mean age kernel (centroid) against
        /// value-independent order statistics (quantile@p75/p90/max) under an
        /// ADVERSARIAL decoy population. Reports final Gini, Δgini, the
        /// adversary dilution ratio and the honest over-charge ratio per
        /// kernel. Does NOT wire anything into consensus.
        DecoyQuantileSweep {
            /// Number of factor-1 background holders
            #[arg(long, default_value = "100")]
            poor: usize,

            /// Number of honest factor-6 whales (age-similar decoys)
            #[arg(long, default_value = "10")]
            honest_whales: usize,

            /// Number of adversarial factor-6 whales (fresh high-value decoys)
            #[arg(long, default_value = "10")]
            adversary_whales: usize,

            /// Ring size (1 real input + ring_size-1 decoys)
            #[arg(long, default_value = "11")]
            ring_size: usize,

            /// Simulated rounds (each round ~= one year of holding)
            #[arg(long, default_value = "25")]
            rounds: u64,

            /// Annual demurrage rate in basis points at max cluster factor
            #[arg(long, default_value = "200")]
            rate_bps: u32,

            /// Honest decoy age jitter half-width in basis points (500 = ±5%)
            #[arg(long, default_value = "500")]
            honest_jitter_bps: u32,

            /// Base RNG seed for reproducibility
            #[arg(long, default_value = "2953379877")]
            seed: u64,
        },

        /// M2 (#605 / #626 §7) — RECALIBRATED-CUMULATIVE run: exercises the
        /// REAL production log-domain cluster-factor curve (not the
        /// #314 hardcoded factors). Population declared in BTH, factor
        /// derived per-epoch from each cluster's cumulative tagged
        /// volume through `ClusterFactorCurve::factor` at 611M-BTH
        /// realism. Emits Δgini vs the >0.05 criterion for the honest
        /// and gamed (split+churn) equilibria.
        ///
        /// Reproducible one-liners (see experiments/M2_RUNBOOK.md):
        ///   sim m2-cumulative --horizon-years 10
        ///   sim m2-cumulative --horizon-years 20 --gamed
        M2Cumulative {
            /// Horizon in years (10 and 20 are the #605 long-horizon runs)
            #[arg(long, default_value = "10")]
            horizon_years: u64,

            /// Gamed equilibrium: strategic whale splits + churns to game
            /// payout
            #[arg(long)]
            gamed: bool,

            /// Deterministic RNG seed
            #[arg(long, default_value = "626626626")]
            seed: u64,

            /// Smoke mode: tiny horizon (days, not years) for end-to-end tests
            #[arg(long)]
            smoke: bool,
        },

        /// M2 (#605 / #626 §7) — EPOCH-HALVING DECAY variant: same cumulative
        /// harness on the REAL curve, plus deterministic `w >>= 1` halving of
        /// each cluster's cumulative wealth at epoch boundaries (pure function
        /// of height; never per-access — M3 lesson). Sweeps half-lives
        /// {2,5,10}yr and additionally emits the WASH-TRADING evasion
        /// metric (pass <20%, prior art 94–99% at aggressive per-hop
        /// decay) and the privacy ring-IDENTIFICATION rate (pass <50%,
        /// prior art 78.7% at 20% decay) from experiments/ANALYSIS.md.
        ///
        /// Reproducible one-liners (see experiments/M2_RUNBOOK.md):
        ///   sim m2-decay --horizon-years 10 --half-life-years 5
        ///   sim m2-decay --horizon-years 20 --half-life-years 2 --gamed
        M2Decay {
            /// Horizon in years
            #[arg(long, default_value = "10")]
            horizon_years: u64,

            /// Epoch-halving half-life in years ({2,5,10} is the #605 sweep)
            #[arg(long, default_value = "5")]
            half_life_years: u64,

            /// Gamed equilibrium: strategic whale splits + churns
            #[arg(long)]
            gamed: bool,

            /// Deterministic RNG seed
            #[arg(long, default_value = "626626626")]
            seed: u64,

            /// Smoke mode: tiny horizon (days, not years) for end-to-end tests
            #[arg(long)]
            smoke: bool,
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
            } => run_scenario_baseline(
                retail_users,
                merchants,
                whale_fraction,
                rounds,
                verbose,
                progress,
            ),
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
            } => {
                run_decay_comparison_all(wealth, hop_decay, half_life, min_blocks, wash_txs, blocks)
            }
            Command::DecayCompareFour {
                wealth,
                hop_decay,
                half_life,
                min_blocks,
                max_per_day,
                wash_txs,
                blocks,
            } => run_decay_comparison_four(
                wealth,
                hop_decay,
                half_life,
                min_blocks,
                max_per_day,
                wash_txs,
                blocks,
            ),
            Command::DecayEntropyCompare {
                initial_wealth,
                initial_factor,
                duration_blocks,
                json,
            } => run_decay_entropy_compare(initial_wealth, initial_factor, duration_blocks, json),
            Command::AttackResistance {
                initial_wealth,
                strategy,
                duration_blocks,
                interval_blocks,
                fake_counterparties,
                legit_ratio,
                json,
            } => run_attack_resistance(
                initial_wealth,
                &strategy,
                duration_blocks,
                interval_blocks,
                fake_counterparties,
                legit_ratio,
                json,
            ),
            Command::EntropyParameterSweep {
                initial_wealth,
                duration_blocks,
                json,
            } => run_entropy_parameter_sweep(initial_wealth, duration_blocks, json),
            Command::LotterySweep {
                blocks,
                txs_per_block,
                quick,
            } => run_lottery_sweep(blocks, txs_per_block, quick),
            Command::LotteryExperiment {
                blocks,
                txs_per_block,
                base_fee,
                emission_per_block,
                split,
                churn_days,
                demurrage_bps,
                quick,
            } => run_lottery_experiment(
                if quick { 86_400 } else { blocks },
                txs_per_block,
                base_fee,
                emission_per_block,
                split,
                churn_days,
                demurrage_bps,
            ),
            Command::EmissionSweep {
                rounds,
                blocks_per_round,
                retail,
                merchants,
                minters,
                whales,
                output,
                quick,
            } => run_emission_sweep(
                rounds,
                blocks_per_round,
                retail,
                merchants,
                minters,
                whales,
                &output,
                quick,
            ),
            Command::DecoyQuantileSweep {
                poor,
                honest_whales,
                adversary_whales,
                ring_size,
                rounds,
                rate_bps,
                honest_jitter_bps,
                seed,
            } => run_decoy_quantile_sweep(
                poor,
                honest_whales,
                adversary_whales,
                ring_size,
                rounds,
                rate_bps,
                honest_jitter_bps,
                seed,
            ),
            Command::M2Cumulative {
                horizon_years,
                gamed,
                seed,
                smoke,
            } => {
                run_m2_cumulative(horizon_years, gamed, seed, smoke);
            }
            Command::M2Decay {
                horizon_years,
                half_life_years,
                gamed,
                seed,
                smoke,
            } => {
                run_m2_decay(horizon_years, half_life_years, gamed, seed, smoke);
            }
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
            agents::whale::WhaleStrategy, run_simulation, Agent, AgentId, MerchantAgent,
            MinterAgent, MixerServiceAgent, RetailUserAgent, SimulationConfig, WhaleAgent,
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

        if show_progress {
            eprintln!(
                "(progress display not available; running {} rounds...)",
                rounds
            );
        }
        let result = run_simulation(&mut agents, &config);
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
            format_monte_carlo_report, run_monte_carlo, MonteCarloConfig, PoolConfig,
            RingSimConfig, RING_SIZE,
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
        println!("  Min similarity threshold: {:.0}%", min_similarity * 100.0);
        if use_parallel {
            println!("  Parallel execution: {} threads", num_threads);
        } else {
            println!("  Parallel execution: disabled");
        }
        println!();

        if use_parallel || show_progress {
            eprintln!("(parallel execution not available; running single-threaded...)");
        }
        let results = {
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
            analyze_ring_sizes, format_ring_size_analysis, run_monte_carlo, MonteCarloConfig,
            PoolConfig, RingSimConfig,
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
            println!(
                "\nRUNNING PRIVACY SIMULATIONS (parallel, {} threads)\n",
                num_threads
            );
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

                let results = {
                    let mut rng = rand::thread_rng();
                    run_monte_carlo(&config, &mut rng)
                };

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
        println!(
            "║  Cluster Wealth: {:>12}                                    ║",
            wealth
        );
        println!(
            "║  Hop Decay Rate: {:>5.1}% per transfer                           ║",
            hop_decay_pct
        );
        println!(
            "║  Block Half-Life: {:>6} blocks (~{:.1} days @ 10s/block)        ║",
            half_life_blocks,
            half_life_blocks as f64 / 8640.0
        );
        println!(
            "║  Wash Trading Simulation: {} txs in {} blocks                   ║",
            wash_txs, blocks_elapsed
        );
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
        println!(
            "{:<25} {:>14.1}% {:>14.1}%",
            "Cluster Tag Remaining",
            hop_remaining * 100.0,
            block_remaining * 100.0
        );
        println!(
            "{:<25} {:>14.1}% {:>14.1}%",
            "Background (Anonymous)",
            hop_background * 100.0,
            block_background * 100.0
        );
        println!(
            "{:<25} {:>13} bps {:>12} bps",
            "Initial Fee Rate", initial_rate, initial_rate
        );
        println!(
            "{:<25} {:>13} bps {:>12} bps",
            "Final Fee Rate", hop_final_rate, block_final_rate
        );
        println!(
            "{:<25} {:>14.1}% {:>14.1}%",
            "Fee Rate Reduction",
            (1.0 - hop_final_rate as f64 / initial_rate as f64) * 100.0,
            (1.0 - block_final_rate as f64 / initial_rate as f64) * 100.0
        );
        println!("─────────────────────────────────────────────────────────────────────");

        // Economic analysis
        let hop_savings_pct = (initial_rate - hop_final_rate) as f64 / initial_rate as f64 * 100.0;
        let block_savings_pct =
            (initial_rate - block_final_rate) as f64 / initial_rate as f64 * 100.0;

        println!();
        println!("ECONOMIC ANALYSIS");
        println!("─────────────────────────────────────────────────────────────────────");

        if hop_savings_pct > 1.0 {
            println!(
                "⚠  HOP-BASED: Wash trading reduces fees by {:.1}%",
                hop_savings_pct
            );
            println!(
                "   After {} self-transfers, whale pays {:.1}x less in fees",
                wash_txs,
                initial_rate as f64 / hop_final_rate as f64
            );
        } else {
            println!(
                "✓  HOP-BASED: Wash trading ineffective ({:.1}% savings)",
                hop_savings_pct
            );
        }

        if block_savings_pct > 1.0 {
            println!(
                "⚠  BLOCK-BASED: Time decay reduces fees by {:.1}%",
                block_savings_pct
            );
        } else {
            println!("✓  BLOCK-BASED: Wash trading completely ineffective");
            println!(
                "   {} self-transfers provide {:.2}% fee reduction",
                wash_txs, block_savings_pct
            );
        }

        println!();
        println!("RECOMMENDATION");
        println!("─────────────────────────────────────────────────────────────────────");

        if block_savings_pct < hop_savings_pct / 10.0 {
            println!(
                "✓  Block-based decay is {:.0}x more resistant to wash trading",
                hop_savings_pct / block_savings_pct.max(0.01)
            );
            println!(
                "   Switch to block-based decay with half-life of {} blocks",
                half_life_blocks
            );
        } else {
            println!("   Both mechanisms show similar wash trading resistance");
            println!("   Consider increasing block half-life for better protection");
        }

        // Sweep analysis
        println!();
        println!("SENSITIVITY ANALYSIS: Wash Trading at Different Scales");
        println!("─────────────────────────────────────────────────────────────────────");
        println!(
            "{:>6} {:>12} {:>12} {:>12} {:>12}",
            "TXs", "Hop Remain", "Hop Fee", "Block Remain", "Block Fee"
        );
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

            println!(
                "{:>6} {:>11.1}% {:>10} bps {:>11.1}% {:>10} bps",
                n_txs,
                h_remain * 100.0,
                h_rate,
                b_remain * 100.0,
                b_rate
            );
        }

        println!("─────────────────────────────────────────────────────────────────────");
        println!();
        println!(
            "Note: Block-based decay resists wash trading because time passes at a fixed rate."
        );
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

        println!(
            "╔══════════════════════════════════════════════════════════════════════════════╗"
        );
        println!(
            "║            THREE-WAY DECAY MECHANISM COMPARISON                               ║"
        );
        println!(
            "║         Hop-Based vs Block-Based vs Rate-Limited Hybrid                       ║"
        );
        println!(
            "╠══════════════════════════════════════════════════════════════════════════════╣"
        );
        println!(
            "║  Cluster Wealth:    {:>12}                                               ║",
            wealth
        );
        println!(
            "║  Hop Decay Rate:    {:>5.1}% per transfer                                      ║",
            hop_decay_pct
        );
        println!(
            "║  Block Half-Life:   {:>6} blocks (~{:.1} days @ 10s/block)                   ║",
            half_life_blocks,
            half_life_blocks as f64 / 8640.0
        );
        println!(
            "║  Rate Limit:        {:>6} blocks (~{:.1} hours between eligible decays)      ║",
            min_blocks_between,
            min_blocks_between as f64 / 360.0
        );
        println!(
            "║  Wash Trading Sim:  {} txs in {} blocks                                      ║",
            wash_txs, blocks_elapsed
        );
        println!(
            "╚══════════════════════════════════════════════════════════════════════════════╝"
        );
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
        println!(
            "────────────────────────────────────────────────────────────────────────────────"
        );
        println!(
            "{:<30} {:>15} {:>15} {:>15}",
            "Metric", "Hop-Based", "Block-Based", "Rate-Limited"
        );
        println!(
            "────────────────────────────────────────────────────────────────────────────────"
        );
        println!(
            "{:<30} {:>14.2}% {:>14.2}% {:>14.2}%",
            "Cluster Tag Remaining",
            hop_remaining * 100.0,
            block_remaining * 100.0,
            rate_remaining * 100.0
        );
        println!(
            "{:<30} {:>13} bps {:>12} bps {:>12} bps",
            "Initial Fee Rate", initial_rate, initial_rate, initial_rate
        );
        println!(
            "{:<30} {:>13} bps {:>12} bps {:>12} bps",
            "Final Fee Rate", hop_final_rate, block_final_rate, rate_final_rate
        );

        let hop_reduction = (1.0 - hop_final_rate as f64 / initial_rate as f64) * 100.0;
        let block_reduction = (1.0 - block_final_rate as f64 / initial_rate as f64) * 100.0;
        let rate_reduction = (1.0 - rate_final_rate as f64 / initial_rate as f64) * 100.0;

        println!(
            "{:<30} {:>14.1}% {:>14.1}% {:>14.1}%",
            "Fee Rate Reduction", hop_reduction, block_reduction, rate_reduction
        );
        println!(
            "{:<30} {:>15} {:>15} {:>15}",
            "Eligible Decay Events", wash_txs, "N/A (time)", eligible_decays
        );
        println!(
            "────────────────────────────────────────────────────────────────────────────────"
        );

        // Interpretation
        println!();
        println!("ANALYSIS");
        println!(
            "────────────────────────────────────────────────────────────────────────────────"
        );

        // Hop-based
        if hop_reduction > 10.0 {
            println!(
                "⚠  HOP-BASED: Vulnerable to wash trading ({:.1}% fee reduction)",
                hop_reduction
            );
            println!(
                "   {} self-transfers reduce fees by {:.1}x",
                wash_txs,
                initial_rate as f64 / hop_final_rate.max(1) as f64
            );
        } else {
            println!(
                "✓  HOP-BASED: Wash trading ineffective ({:.1}% reduction)",
                hop_reduction
            );
        }

        // Block-based
        if block_reduction < 1.0 {
            println!("✓  BLOCK-BASED: Completely wash-trading resistant");
            println!("   Only time affects decay, not transaction count");
        } else {
            println!(
                "○  BLOCK-BASED: {:.1}% natural decay over {} blocks",
                block_reduction, blocks_elapsed
            );
        }

        // Rate-limited
        let max_possible_decays = blocks_elapsed / min_blocks_between.max(1);
        println!(
            "○  RATE-LIMITED: {} of {} possible decay events triggered",
            eligible_decays, max_possible_decays
        );
        if rate_reduction < hop_reduction / 2.0 {
            println!(
                "✓  Rate limiting reduced attack effectiveness by {:.1}x",
                hop_reduction / rate_reduction.max(0.01)
            );
        }

        // Sweep: Different wash trading intensities
        println!();
        println!("SENSITIVITY: Wash Trading at Different Intensities");
        println!(
            "────────────────────────────────────────────────────────────────────────────────"
        );
        println!(
            "{:>8} {:>12} {:>12} {:>12} {:>12}",
            "TXs", "Hop Remain", "Block Remain", "Rate Remain", "Rate Decays"
        );
        println!(
            "────────────────────────────────────────────────────────────────────────────────"
        );

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

            println!(
                "{:>8} {:>11.2}% {:>11.2}% {:>11.2}% {:>12}",
                n_txs,
                h_remain * 100.0,
                b_remain * 100.0,
                r_remain * 100.0,
                decays
            );
        }

        // Recommendation
        println!();
        println!("RECOMMENDATION");
        println!(
            "────────────────────────────────────────────────────────────────────────────────"
        );

        if block_reduction < rate_reduction && block_reduction < hop_reduction {
            println!("✓  BLOCK-BASED decay is most resistant to wash trading");
            println!("   No transaction can accelerate decay - only time matters");
        } else if rate_reduction < hop_reduction {
            println!("✓  RATE-LIMITED HYBRID is a good compromise:");
            println!("   • Keeps intuitive 'decay per hop' semantics");
            println!(
                "   • Limits max decay rate to 1 per {} blocks (~{:.1} hours)",
                min_blocks_between,
                min_blocks_between as f64 / 360.0
            );
            println!(
                "   • {:.0}x more wash-trading resistant than pure hop-based",
                hop_reduction / rate_reduction.max(0.01)
            );
        } else {
            println!("⚠  All mechanisms show similar behavior in this scenario");
            println!("   Consider adjusting parameters for better differentiation");
        }

        println!();
        println!("Trade-offs:");
        println!("  • Block-based: Simplest, most resistant, but tags decay even without trading");
        println!(
            "  • Rate-limited: Keeps hop semantics, resistant to wash trading, slightly complex"
        );
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
        println!(
            "SCENARIO 1: RAPID WASH TRADING ({} txs in {} blocks)",
            wash_txs,
            blocks_elapsed.min(100)
        );
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

        println!(
            "{:<20} {:>12} {:>12} {:>12} {:>12}",
            "Metric", "Hop-Based", "Block-Based", "Rate-Ltd", "AND-Based"
        );
        println!("────────────────────────────────────────────────────────────────────────────────────────");
        println!(
            "{:<20} {:>11.2}% {:>11.2}% {:>11.2}% {:>11.2}%",
            "Tag Remaining",
            hop_remain * 100.0,
            block_remain * 100.0,
            rate_remain * 100.0,
            and_remain * 100.0
        );
        println!(
            "{:<20} {:>12} {:>12} {:>12} {:>12}",
            "Decay Events", wash_txs, "N/A", rate_decays, and_decays
        );

        // ============================================================
        // SCENARIO 2: PATIENT WASH TRADING (spaced out over time)
        // ============================================================
        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!(
            "SCENARIO 2: PATIENT WASH TRADING ({} txs over {} blocks = {:.1} days)",
            wash_txs,
            blocks_elapsed,
            blocks_elapsed as f64 / 8640.0
        );
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

        println!(
            "{:<20} {:>12} {:>12} {:>12} {:>12}",
            "Metric", "Hop-Based", "Block-Based", "Rate-Ltd", "AND-Based"
        );
        println!("────────────────────────────────────────────────────────────────────────────────────────");
        println!(
            "{:<20} {:>11.2}% {:>11.2}% {:>11.2}% {:>11.2}%",
            "Tag Remaining",
            hop_remain2 * 100.0,
            block_remain2 * 100.0,
            rate_remain2 * 100.0,
            and_remain2 * 100.0
        );
        println!(
            "{:<20} {:>12} {:>12} {:>12} {:>12}",
            "Decay Events", wash_txs, "N/A", rate_decays2, and_decays2
        );

        // ============================================================
        // SCENARIO 3: HOLDING WITHOUT TRADING (key differentiator!)
        // ============================================================
        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!(
            "SCENARIO 3: HOLDING WITHOUT TRADING (0 txs over {} blocks = {:.1} days)",
            blocks_elapsed,
            blocks_elapsed as f64 / 8640.0
        );
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

        println!(
            "{:<20} {:>12} {:>12} {:>12} {:>12}",
            "Metric", "Hop-Based", "Block-Based", "Rate-Ltd", "AND-Based"
        );
        println!("────────────────────────────────────────────────────────────────────────────────────────");
        println!(
            "{:<20} {:>11.2}% {:>11.2}% {:>11.2}% {:>11.2}%",
            "Tag Remaining",
            hop_remain3,
            block_remain3 * 100.0,
            rate_remain3,
            and_remain3
        );
        println!(
            "{:<20} {:>12} {:>12} {:>12} {:>12}",
            "Passive Decay?", "NO", "YES", "NO", "NO"
        );

        // ============================================================
        // ANALYSIS & RECOMMENDATION
        // ============================================================
        println!();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("ANALYSIS");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();

        println!("Model Comparison:");
        println!(
            "  • HOP-BASED:   Rapid={:.1}%, Patient={:.1}%, Holding=100%",
            hop_remain * 100.0,
            hop_remain2 * 100.0
        );
        println!(
            "  • BLOCK-BASED: Rapid={:.1}%, Patient={:.1}%, Holding={:.1}%",
            block_remain * 100.0,
            block_remain2 * 100.0,
            block_remain3 * 100.0
        );
        println!(
            "  • RATE-LTD:    Rapid={:.1}%, Patient={:.1}%, Holding=100%",
            rate_remain * 100.0,
            rate_remain2 * 100.0
        );
        println!(
            "  • AND-BASED:   Rapid={:.1}%, Patient={:.1}%, Holding=100%",
            and_remain * 100.0,
            and_remain2 * 100.0
        );

        println!();
        println!("Key Insights:");
        if hop_remain < 10.0 {
            println!(
                "  ❌ HOP-BASED: Vulnerable to rapid wash trading ({:.1}% remaining)",
                hop_remain
            );
        }
        if block_remain3 < 50.0 {
            println!("  ⚠️  BLOCK-BASED: Passive decay gives 'free' tax reduction over time");
        }
        if rate_remain < 50.0 && rate_remain2 < 10.0 {
            println!(
                "  ⚠️  RATE-LIMITED: Patient attackers can still decay to {:.1}%",
                rate_remain2
            );
        }

        // Check AND-based with epoch cap
        let max_decays_possible = max_per_day as u64 * (blocks_elapsed / 8640 + 1);
        let decay_with_cap =
            (1.0 - hop_decay_pct / 100.0).powi(max_decays_possible.min(and_decays2 as u64) as i32);
        println!();
        println!("  ✓ AND-BASED advantages:");
        println!("    • Requires BOTH time AND transfers for decay");
        println!("    • Holding without trading: NO decay (wealthy must wash trade)");
        println!(
            "    • Epoch cap limits max decay to {} per day",
            max_per_day
        );
        println!(
            "    • Over {:.1} days: max {} decays = {:.1}% remaining",
            blocks_elapsed as f64 / 8640.0,
            max_decays_possible.min(and_decays2 as u64),
            decay_with_cap * 100.0
        );

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
        println!(
            "This gives: Max decay of {:.1}% per day, {:.1}% per week, {:.1}% per month",
            (1.0 - 0.05_f64.powi(12)) * 100.0,
            (1.0 - 0.95_f64.powi(84)) * 100.0,
            (1.0 - 0.95_f64.powi(360)) * 100.0
        );
    }

    // ========== Entropy-Weighted Decay Commands ==========

    fn run_decay_entropy_compare(
        initial_wealth: u64,
        initial_factor: f64,
        duration_blocks: u64,
        _json_output: bool,
    ) {
        use bth_cluster_tax::{compare_decay_modes, AttackStrategy, DecayMode};

        let strategies = vec![
            ("rapid-wash", AttackStrategy::RapidWash { transfers: 100 }),
            (
                "patient-wash-720",
                AttackStrategy::PatientWash {
                    interval_blocks: 720,
                    duration_blocks,
                },
            ),
            (
                "patient-wash-1440",
                AttackStrategy::PatientWash {
                    interval_blocks: 1440,
                    duration_blocks,
                },
            ),
            (
                "sybil-wash-100",
                AttackStrategy::SybilWash {
                    fake_counterparties: 100,
                    transfers_per_counterparty: 10,
                },
            ),
            (
                "partial-commerce-50",
                AttackStrategy::PartialCommerce {
                    legit_ratio: 0.5,
                    total_transactions: 100,
                },
            ),
            (
                "legitimate-commerce",
                AttackStrategy::PartialCommerce {
                    legit_ratio: 1.0,
                    total_transactions: 100,
                },
            ),
        ];

        println!("╔════════════════════════════════════════════════════════════════════════════════════════╗");
        println!("║              ENTROPY-WEIGHTED vs AGE-BASED DECAY COMPARISON                            ║");
        println!("╠════════════════════════════════════════════════════════════════════════════════════════╣");
        println!("║  Initial Wealth:     {:>12}                                                         ║", initial_wealth);
        println!("║  Initial Factor:     {:>5.1}x (cluster factor)                                           ║", initial_factor);
        println!("║  Duration:           {:>6} blocks (~{:.1} days)                                          ║",
            duration_blocks, duration_blocks as f64 / 8640.0);
        println!("╚════════════════════════════════════════════════════════════════════════════════════════╝");
        println!();

        println!("┌───────────────────────┬─────────────────────────┬─────────────────────────┐");
        println!("│      STRATEGY         │     AGE-BASED DECAY     │  ENTROPY-WEIGHTED DECAY │");
        println!("├───────────────────────┼─────────────────────────┼─────────────────────────┤");
        println!("│                       │  Tag %  │ Decays│ Resist│  Tag %  │ Decays│ Resist│");
        println!("├───────────────────────┼─────────┼───────┼───────┼─────────┼───────┼───────┤");

        for (name, strategy) in &strategies {
            let comparison =
                compare_decay_modes(strategy, initial_wealth, initial_factor, duration_blocks);

            let age_result = comparison
                .iter()
                .find(|(m, _)| *m == DecayMode::AgeBased)
                .map(|(_, r)| r);
            let entropy_result = comparison
                .iter()
                .find(|(m, _)| *m == DecayMode::EntropyWeighted)
                .map(|(_, r)| r);

            if let (Some(age), Some(entropy)) = (age_result, entropy_result) {
                let age_resist = if age.tag_remaining_fraction > 0.5 {
                    "✓"
                } else {
                    "✗"
                };
                let entropy_resist = if entropy.tag_remaining_fraction > 0.5 {
                    "✓"
                } else {
                    "✗"
                };

                println!(
                    "│ {:<21} │ {:>6.1}% │ {:>5} │   {}   │ {:>6.1}% │ {:>5} │   {}   │",
                    name,
                    age.tag_remaining_fraction * 100.0,
                    age.decay_events,
                    age_resist,
                    entropy.tag_remaining_fraction * 100.0,
                    entropy.decay_events,
                    entropy_resist
                );
            }
        }

        println!("└───────────────────────┴─────────┴───────┴───────┴─────────┴───────┴───────┘");
        println!();
        println!("Legend: Resist = ✓ if >50% tag remains (attack resisted), ✗ if ≤50% (attack succeeded)");
        println!();

        // Analysis
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("ANALYSIS");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();
        println!(
            "Key Insight: Entropy-weighted decay only applies when cluster_entropy() increases."
        );
        println!();
        println!("  • SELF-TRANSFERS: Same tags in, same tags out → entropy unchanged → NO decay");
        println!("  • SYBIL ATTACK: Fake counterparties have attacker's tags → entropy unchanged → NO decay");
        println!(
            "  • REAL COMMERCE: Different cluster tags mix → entropy increases → decay applies"
        );
        println!();
        println!("Advantages of entropy-weighted decay:");
        println!("  1. ✓ Resistant to rapid wash trading (no entropy change)");
        println!(
            "  2. ✓ Resistant to patient wash trading (no entropy change regardless of timing)"
        );
        println!("  3. ✓ Resistant to sybil wash trading (fake identities don't add entropy)");
        println!("  4. ✓ Allows natural decay through legitimate commerce");
        println!("  5. ✓ Privacy preserved (uses existing cluster_entropy() calculation)");
    }

    fn run_attack_resistance(
        initial_wealth: u64,
        strategy_name: &str,
        duration_blocks: u64,
        interval_blocks: u64,
        fake_counterparties: u32,
        legit_ratio: f64,
        _json_output: bool,
    ) {
        use bth_cluster_tax::{compare_decay_modes, AttackStrategy, DecayMode};

        let strategy = match strategy_name {
            "rapid-wash" | "rapid" => AttackStrategy::RapidWash { transfers: 100 },
            "patient-wash" | "patient" => AttackStrategy::PatientWash {
                interval_blocks,
                duration_blocks,
            },
            "sybil-wash" | "sybil" => AttackStrategy::SybilWash {
                fake_counterparties,
                transfers_per_counterparty: 10,
            },
            "partial-commerce" | "partial" => AttackStrategy::PartialCommerce {
                legit_ratio,
                total_transactions: 100,
            },
            "legitimate" | "legit" | "commerce" => AttackStrategy::PartialCommerce {
                legit_ratio: 1.0,
                total_transactions: 100,
            },
            _ => {
                eprintln!("Unknown strategy: {}", strategy_name);
                eprintln!("Valid options: rapid-wash, patient-wash, sybil-wash, partial-commerce, legitimate");
                return;
            }
        };

        let initial_factor = 6.0; // Maximum cluster factor
        let comparison =
            compare_decay_modes(&strategy, initial_wealth, initial_factor, duration_blocks);

        println!("╔════════════════════════════════════════════════════════════════════════════════════════╗");
        println!("║                     ATTACK RESISTANCE ANALYSIS                                         ║");
        println!("╠════════════════════════════════════════════════════════════════════════════════════════╣");
        println!(
            "║  Strategy:           {:<30}                                    ║",
            strategy_name
        );
        println!("║  Initial Wealth:     {:>12}                                                         ║", initial_wealth);
        println!("║  Duration:           {:>6} blocks (~{:.1} days)                                          ║",
            duration_blocks, duration_blocks as f64 / 8640.0);
        println!("╚════════════════════════════════════════════════════════════════════════════════════════╝");
        println!();

        for (mode, result) in &comparison {
            let mode_name = match mode {
                DecayMode::AgeBased => "AGE-BASED",
                DecayMode::EntropyWeighted => "ENTROPY-WEIGHTED",
                DecayMode::None => "NO DECAY",
            };

            let attack_success = result.tag_remaining_fraction < 0.5;
            let status = if attack_success {
                "VULNERABLE ✗"
            } else {
                "RESISTANT ✓"
            };

            println!("┌─────────────────────────────────────────────────┐");
            println!("│ {:^47} │", mode_name);
            println!("├─────────────────────────────────────────────────┤");
            println!("│ Initial tag weight:     {:>20}  │", result.initial_tag);
            println!("│ Final tag weight:       {:>20}  │", result.final_tag);
            println!(
                "│ Tag remaining:          {:>19.2}%  │",
                result.tag_remaining_fraction * 100.0
            );
            println!("│ Decay events:           {:>20}  │", result.decay_events);
            println!("│ Total attempts:         {:>20}  │", result.total_attempts);
            println!("│ Attack status:          {:>20}  │", status);
            println!("└─────────────────────────────────────────────────┘");
            println!();
        }

        // Summary
        let age_result = comparison
            .iter()
            .find(|(m, _)| *m == DecayMode::AgeBased)
            .map(|(_, r)| r);
        let entropy_result = comparison
            .iter()
            .find(|(m, _)| *m == DecayMode::EntropyWeighted)
            .map(|(_, r)| r);

        if let (Some(age), Some(entropy)) = (age_result, entropy_result) {
            println!("Summary:");
            println!(
                "  Age-based:        {:.1}% remaining ({} decay events)",
                age.tag_remaining_fraction * 100.0,
                age.decay_events
            );
            println!(
                "  Entropy-weighted: {:.1}% remaining ({} decay events)",
                entropy.tag_remaining_fraction * 100.0,
                entropy.decay_events
            );

            if entropy.tag_remaining_fraction > age.tag_remaining_fraction {
                let improvement =
                    (entropy.tag_remaining_fraction - age.tag_remaining_fraction) * 100.0;
                println!();
                println!(
                    "  ✓ Entropy-weighted decay provides {:.1}% better attack resistance!",
                    improvement
                );
            }
        }
    }

    fn run_entropy_parameter_sweep(initial_wealth: u64, duration_blocks: u64, _json_output: bool) {
        use bth_cluster_tax::{compare_decay_modes, AttackStrategy, DecayMode};

        // Sweep over different parameters
        let patient_intervals = [360, 720, 1440, 2880]; // 1h, 2h, 4h, 8h

        println!("╔════════════════════════════════════════════════════════════════════════════════════════╗");
        println!("║                  ENTROPY DECAY PARAMETER SENSITIVITY                                   ║");
        println!("╠════════════════════════════════════════════════════════════════════════════════════════╣");
        println!("║  Initial Wealth:     {:>12}                                                         ║", initial_wealth);
        println!("║  Duration:           {:>6} blocks (~{:.1} days)                                          ║",
            duration_blocks, duration_blocks as f64 / 8640.0);
        println!("╚════════════════════════════════════════════════════════════════════════════════════════╝");
        println!();

        // Test 1: Patient wash at different intervals
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("TEST 1: Patient Wash Trading at Different Intervals");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();
        println!("┌───────────┬─────────────────────────┬─────────────────────────┐");
        println!("│  Interval │     AGE-BASED DECAY     │  ENTROPY-WEIGHTED DECAY │");
        println!("├───────────┼───────────┬─────────────┼───────────┬─────────────┤");
        println!("│  (hours)  │  Tag %    │   Status    │  Tag %    │   Status    │");
        println!("├───────────┼───────────┼─────────────┼───────────┼─────────────┤");

        for &interval in &patient_intervals {
            let strategy = AttackStrategy::PatientWash {
                interval_blocks: interval,
                duration_blocks,
            };
            let comparison = compare_decay_modes(&strategy, initial_wealth, 6.0, duration_blocks);

            let age_result = comparison
                .iter()
                .find(|(m, _)| *m == DecayMode::AgeBased)
                .map(|(_, r)| r);
            let entropy_result = comparison
                .iter()
                .find(|(m, _)| *m == DecayMode::EntropyWeighted)
                .map(|(_, r)| r);

            if let (Some(age), Some(entropy)) = (age_result, entropy_result) {
                let age_status = if age.tag_remaining_fraction > 0.5 {
                    "Resistant"
                } else {
                    "Vulnerable"
                };
                let entropy_status = if entropy.tag_remaining_fraction > 0.5 {
                    "Resistant"
                } else {
                    "Vulnerable"
                };

                println!(
                    "│   {:>5.1}   │  {:>6.1}%  │ {:^11} │  {:>6.1}%  │ {:^11} │",
                    interval as f64 / 360.0,
                    age.tag_remaining_fraction * 100.0,
                    age_status,
                    entropy.tag_remaining_fraction * 100.0,
                    entropy_status
                );
            }
        }
        println!("└───────────┴───────────┴─────────────┴───────────┴─────────────┘");
        println!();

        // Test 2: Partial commerce at different ratios
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("TEST 2: Partial Commerce (Mixed Legitimate + Wash Trading)");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();
        println!("┌───────────────┬─────────────────────────┬─────────────────────────┐");
        println!("│  Legit Ratio  │     AGE-BASED DECAY     │  ENTROPY-WEIGHTED DECAY │");
        println!("├───────────────┼───────────┬─────────────┼───────────┬─────────────┤");
        println!("│               │  Tag %    │   Decays    │  Tag %    │   Decays    │");
        println!("├───────────────┼───────────┼─────────────┼───────────┼─────────────┤");

        for ratio in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let strategy = AttackStrategy::PartialCommerce {
                legit_ratio: ratio,
                total_transactions: 100,
            };
            let comparison = compare_decay_modes(&strategy, initial_wealth, 6.0, duration_blocks);

            let age_result = comparison
                .iter()
                .find(|(m, _)| *m == DecayMode::AgeBased)
                .map(|(_, r)| r);
            let entropy_result = comparison
                .iter()
                .find(|(m, _)| *m == DecayMode::EntropyWeighted)
                .map(|(_, r)| r);

            if let (Some(age), Some(entropy)) = (age_result, entropy_result) {
                println!(
                    "│     {:>4.0}%     │  {:>6.1}%  │    {:>5}    │  {:>6.1}%  │    {:>5}    │",
                    ratio * 100.0,
                    age.tag_remaining_fraction * 100.0,
                    age.decay_events,
                    entropy.tag_remaining_fraction * 100.0,
                    entropy.decay_events
                );
            }
        }
        println!("└───────────────┴───────────┴─────────────┴───────────┴─────────────┘");
        println!();

        // Analysis
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("ANALYSIS");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();
        println!("Observations:");
        println!();
        println!("  1. Patient Wash Trading:");
        println!("     • Age-based decay becomes vulnerable as attacker increases patience");
        println!("     • Entropy-weighted decay remains resistant regardless of timing");
        println!();
        println!("  2. Partial Commerce:");
        println!(
            "     • Age-based: All transactions (legit or not) cause decay after age requirement"
        );
        println!("     • Entropy-weighted: Only legitimate commerce causes decay (proportional to ratio)");
        println!();
        println!("  3. Key Insight:");
        println!("     Entropy-weighted decay provides resistance proportional to the attacker's");
        println!(
            "     legitimate commerce ratio. Pure wash trading = 0% decay, regardless of patience."
        );
    }

    /// Parameter sweep for the combined progressive mechanism.
    ///
    /// Implements the sweep from docs/design/asymmetric-fees-simulation.md.
    /// Engine dynamics depend on (decay, threshold); the structure-fee penalty
    /// is applied analytically to attack cost since the tx loop models fixed
    /// 2-output transactions.
    fn run_lottery_sweep(blocks: u64, txs_per_block: u32, quick: bool) {
        use bth_cluster_tax::simulation::lottery::{
            LotteryConfig, LotterySimulation, SelectionMode, SybilStrategy, TransactionModel,
        };

        // 1 BTH = 1_000 base units at this sim's scale
        // (combined_mechanism uses min_utxo_value = 100_000 for 100 BTH).
        const BTH: u64 = 1_000;
        const PARKER_SPLIT: u32 = 100;
        const SYBIL_ACCOUNTS: u32 = 10;
        let total_wealth: u64 = 100_000_000 * BTH;

        let decays: Vec<f64> = if quick {
            vec![0.03]
        } else {
            vec![0.01, 0.03, 0.10]
        };
        let thresholds_bth: Vec<u64> = if quick {
            vec![1_000]
        } else {
            vec![100, 1_000, 5_000]
        };
        let penalties: Vec<f64> = if quick {
            vec![1.0]
        } else {
            vec![0.5, 1.0, 2.0]
        };

        let make_config = |decay: f64, threshold_bth: u64| -> LotteryConfig {
            let mut config = LotteryConfig::combined_mechanism();
            config.selection_mode = SelectionMode::ValueWeightedWithFloor {
                ticket_threshold: threshold_bth * BTH,
                decay_rate_per_day: decay,
                min_eligibility: 0.10,
                blocks_per_day: 4_320,
            };
            config
        };

        // Population: 5% poor (80 owners), 25% middle (30), 60% honest whales
        // (8), 5% parking attacker, 5% Sybil. Initial Gini ~0.72.
        let build = |config: LotteryConfig| -> (LotterySimulation, u64, u64, Vec<u64>) {
            let mut sim = LotterySimulation::new(config, FeeCurve::default_params());
            for _ in 0..80 {
                sim.add_owner(total_wealth / 20 / 80, SybilStrategy::Normal);
            }
            for _ in 0..30 {
                sim.add_owner(total_wealth / 4 / 30, SybilStrategy::Normal);
            }
            let whales: Vec<u64> = (0..8)
                .map(|_| sim.add_owner(total_wealth * 60 / 100 / 8, SybilStrategy::Normal))
                .collect();
            let parker = sim.add_owner(
                total_wealth * 5 / 100,
                SybilStrategy::ParkingAttack {
                    split_target: PARKER_SPLIT,
                },
            );
            let sybil = sim.add_owner(
                total_wealth * 5 / 100,
                SybilStrategy::MultiAccount {
                    num_accounts: SYBIL_ACCOUNTS,
                },
            );
            (sim, parker, sybil, whales)
        };

        println!("Combined Progressive Mechanism: Parameter Sweep");
        println!("================================================");
        println!(
            "Economy: 100M BTH, 120 owners (5% poor / 25% middle / 60% whales / 10% attackers)"
        );
        println!(
            "Duration: {} blocks (~{} days), {} txs/block",
            blocks,
            blocks / 4_320,
            txs_per_block
        );
        println!();

        // Baseline: identical population and fees, but everything burned
        // (pool_fraction = 0) -> no redistribution. This reproduces the
        // Experiment 5 setup where progressive burn fees showed no Gini
        // improvement.
        let mut baseline_config = make_config(0.03, 1_000);
        baseline_config.pool_fraction = 0.0;
        let (mut baseline_sim, _, _, _) = build(baseline_config);
        let baseline_initial = baseline_sim.calculate_gini();
        baseline_sim.advance_blocks_immediate(
            blocks,
            txs_per_block,
            TransactionModel::ValueWeighted,
        );
        let baseline_final = baseline_sim.calculate_gini();
        println!(
            "Baseline (burn-only, no lottery): Gini {:.4} -> {:.4} (delta {:+.4})",
            baseline_initial,
            baseline_final,
            baseline_final - baseline_initial
        );
        println!();

        struct RunResult {
            decay: f64,
            threshold_bth: u64,
            initial_gini: f64,
            final_gini: f64,
            parking_adv: f64,
            sybil_adv: f64,
            parker_winnings: u64,
            honest_rate: f64,
            parker_cluster_factor: f64,
            final_utxos: usize,
        }

        let mut runs: Vec<RunResult> = Vec::new();
        for &decay in &decays {
            for &threshold_bth in &thresholds_bth {
                let config = make_config(decay, threshold_bth);
                let (mut sim, parker, sybil, whales) = build(config);
                let initial_gini = sim.calculate_gini();
                let parker_wealth = sim.owner_value(parker) as f64;
                let sybil_wealth = sim.owner_value(sybil) as f64;
                let whale_wealth: f64 = whales.iter().map(|id| sim.owner_value(*id) as f64).sum();

                sim.advance_blocks_immediate(
                    blocks,
                    txs_per_block,
                    TransactionModel::ValueWeighted,
                );

                let final_gini = sim.calculate_gini();
                let whale_winnings: u64 = whales
                    .iter()
                    .map(|id| sim.owners.get(id).map(|o| o.total_winnings).unwrap_or(0))
                    .sum();
                // Winnings per unit of initial wealth, honest whales = reference.
                let honest_rate = whale_winnings as f64 / whale_wealth;
                let parker_winnings = sim
                    .owners
                    .get(&parker)
                    .map(|o| o.total_winnings)
                    .unwrap_or(0);
                let sybil_winnings = sim
                    .owners
                    .get(&sybil)
                    .map(|o| o.total_winnings)
                    .unwrap_or(0);
                let parking_adv = if honest_rate > 0.0 {
                    (parker_winnings as f64 / parker_wealth) / honest_rate
                } else {
                    0.0
                };
                let sybil_adv = if honest_rate > 0.0 {
                    (sybil_winnings as f64 / sybil_wealth) / honest_rate
                } else {
                    0.0
                };
                let parker_cluster_factor = sim
                    .owners
                    .get(&parker)
                    .and_then(|o| o.utxo_ids.first())
                    .and_then(|id| sim.utxos.get(id))
                    .map(|u| u.cluster_factor)
                    .unwrap_or(1.0);

                runs.push(RunResult {
                    decay,
                    threshold_bth,
                    initial_gini,
                    final_gini,
                    parking_adv,
                    sybil_adv,
                    parker_winnings,
                    honest_rate,
                    parker_cluster_factor,
                    final_utxos: sim.utxos.len(),
                });
            }
        }

        println!("## Parameter Sweep Results");
        println!();
        println!("| Penalty | Decay | Threshold (BTH) | Gini0 | GiniF | dGini | vs Baseline | Park Adv | Park ROI | Sybil Adv | UTXOs |");
        println!("|---------|-------|-----------------|-------|-------|-------|-------------|----------|----------|-----------|-------|");

        let mut sweet_spot: Option<(f64, f64, u64, f64)> = None;
        for run in &runs {
            for &penalty in &penalties {
                // Analytic parking ROI: extra winnings over honest strategy
                // vs the one-time split cost under this structure-fee penalty.
                let mut cfg = make_config(run.decay, run.threshold_bth);
                cfg.split_penalty_multiplier = penalty;
                let split_factor = cfg.structure_factor(1, PARKER_SPLIT);
                let split_cost = cfg.base_fee as f64 * run.parker_cluster_factor * split_factor;
                let parker_wealth = (total_wealth * 5 / 100) as f64;
                let extra_winnings = run.parker_winnings as f64 - run.honest_rate * parker_wealth;
                let parking_roi = if split_cost > 0.0 {
                    extra_winnings / split_cost
                } else {
                    0.0
                };

                let gini_delta = run.initial_gini - run.final_gini;
                let vs_baseline = baseline_final - run.final_gini;
                println!(
                    "| {:.1} | {:.2} | {} | {:.4} | {:.4} | {:+.4} | {:+.4} | {:.2}x | {:.2} | {:.2}x | {} |",
                    penalty,
                    run.decay,
                    run.threshold_bth,
                    run.initial_gini,
                    run.final_gini,
                    gini_delta,
                    vs_baseline,
                    run.parking_adv,
                    parking_roi,
                    run.sybil_adv,
                    run.final_utxos
                );

                if penalty == 1.0 && run.decay == 0.03 && run.threshold_bth == 1_000 {
                    sweet_spot =
                        Some((gini_delta, vs_baseline, run.final_utxos as u64, parking_roi));
                }
            }
        }

        println!();
        println!("Note: engine dynamics are identical across penalty values (tx loop models");
        println!("fixed 2-output transactions); the penalty column affects only the analytic");
        println!("split-cost in Park ROI.");
        println!();

        if let Some((gini_delta, vs_baseline, _, parking_roi)) = sweet_spot {
            let park_ok = runs
                .iter()
                .find(|r| r.decay == 0.03 && r.threshold_bth == 1_000)
                .map(|r| r.parking_adv < 2.0)
                .unwrap_or(false);
            let sybil_ok = runs
                .iter()
                .find(|r| r.decay == 0.03 && r.threshold_bth == 1_000)
                .map(|r| r.sybil_adv < 2.0)
                .unwrap_or(false);
            println!("## Success Criteria (recommended config: penalty=1.0, decay=0.03, threshold=1000 BTH)");
            println!();
            println!(
                "- Gini reduction vs baseline > 0.05: {} ({:+.4})",
                if vs_baseline > 0.05 { "PASS" } else { "FAIL" },
                vs_baseline
            );
            println!("- Absolute Gini reduction: {:+.4}", gini_delta);
            println!(
                "- Parking attack defeated (ROI < 1.0): {} ({:.2})",
                if parking_roi < 1.0 { "PASS" } else { "FAIL" },
                parking_roi
            );
            println!(
                "- Parking advantage < 2.0x: {}",
                if park_ok { "PASS" } else { "FAIL" }
            );
            println!(
                "- Splitting advantage < 2.0x: {}",
                if sybil_ok { "PASS" } else { "FAIL" }
            );
        }
    }

    /// Structural Gini-reduction experiment.
    ///
    /// Seven scenarios isolating each redistribution lever:
    ///   A. Status quo: value-weighted payout, fees only (known: zero Gini
    /// effect)   B. Uniform-per-UTXO payout, fees only (payout
    /// progressivity alone)   C. Value-weighted payout + emission (control:
    /// proportional payout of      emission should be Gini-neutral)
    ///   D. Uniform payout + emission (the naive full proposal, honest whale)
    ///   E. D with a strategic whale: splits into N UTXOs and churns them to
    ///      stay lottery-eligible (gamed equilibrium for uniform payouts)
    ///   F. Value-weighted payout + emission + cluster demurrage (intake-side
    ///      progressivity; payout deliberately game-proof-proportional)
    ///   G. F with the strategic whale (demurrage robustness: splitting does
    ///      not change cluster factor, so gaming should be pure waste)
    ///
    /// Cluster factors are assigned explicitly per wealth class (poor 1x,
    /// middle 2x, whales 6x) so intake progressivity is controlled — the
    /// default FeeCurve's w_mid is far below sim balances and would pin
    /// everyone at max factor (this flaw also affected the original sweep).
    #[allow(clippy::too_many_arguments)]
    fn run_lottery_experiment(
        blocks: u64,
        txs_per_block: u32,
        base_fee: u64,
        emission_per_block: u64,
        split: u32,
        churn_days: u64,
        demurrage_bps: u32,
    ) {
        use bth_cluster_tax::simulation::lottery::{
            LotteryConfig, LotterySimulation, SelectionMode, SybilStrategy, TransactionModel,
        };

        const BTH: u64 = 1_000;
        const BLOCKS_PER_DAY: u64 = 4_320;
        let total_wealth: u64 = 100_000_000 * BTH;

        #[derive(Clone, Copy, PartialEq)]
        enum Whale {
            Honest,
            SplitChurn,
            /// Never transacts: escapes spend-time demurrage entirely,
            /// touched only by emission dilution (issue #314)
            Parker,
        }
        #[derive(Clone, Copy, PartialEq)]
        enum Demurrage {
            None,
            /// Daily balance charge (original validation model)
            Daily,
            /// Accrual charged when coins move (matches node implementation)
            AtSpend,
        }
        #[derive(Clone, Copy, PartialEq)]
        enum Payout {
            /// Value-weighted with floor: split-proof, proportional
            Vw,
            /// Uniform per UTXO: progressive, catastrophically gameable
            Uniform,
            /// Value x inverse cluster factor: progressive AND split-proof
            /// (factor inherits through splits); gaming requires tag decay,
            /// which is bounded by the AND/entropy decay mechanisms
            ClusterTilted,
        }
        struct Scenario {
            name: &'static str,
            payout: Payout,
            emission: bool,
            whale: Whale,
            demurrage: Demurrage,
        }
        let scenarios = [
            Scenario {
                name: "A: status quo (VW payout, fees only)",
                payout: Payout::Vw,
                emission: false,
                whale: Whale::Honest,
                demurrage: Demurrage::None,
            },
            Scenario {
                name: "B: uniform payout, fees only",
                payout: Payout::Uniform,
                emission: false,
                whale: Whale::Honest,
                demurrage: Demurrage::None,
            },
            Scenario {
                name: "C: VW payout + emission",
                payout: Payout::Vw,
                emission: true,
                whale: Whale::Honest,
                demurrage: Demurrage::None,
            },
            Scenario {
                name: "D: uniform payout + emission",
                payout: Payout::Uniform,
                emission: true,
                whale: Whale::Honest,
                demurrage: Demurrage::None,
            },
            Scenario {
                name: "E: D + whale split+churn (gamed)",
                payout: Payout::Uniform,
                emission: true,
                whale: Whale::SplitChurn,
                demurrage: Demurrage::None,
            },
            Scenario {
                name: "F: VW payout + emission + demurrage",
                payout: Payout::Vw,
                emission: true,
                whale: Whale::Honest,
                demurrage: Demurrage::Daily,
            },
            Scenario {
                name: "G: F + whale split+churn (gaming attempt)",
                payout: Payout::Vw,
                emission: true,
                whale: Whale::SplitChurn,
                demurrage: Demurrage::Daily,
            },
            Scenario {
                name: "H: cluster-tilted payout + emission",
                payout: Payout::ClusterTilted,
                emission: true,
                whale: Whale::Honest,
                demurrage: Demurrage::None,
            },
            Scenario {
                name: "I: H + whale split+churn (gaming attempt)",
                payout: Payout::ClusterTilted,
                emission: true,
                whale: Whale::SplitChurn,
                demurrage: Demurrage::None,
            },
            Scenario {
                name: "J: H + demurrage daily (original model)",
                payout: Payout::ClusterTilted,
                emission: true,
                whale: Whale::Honest,
                demurrage: Demurrage::Daily,
            },
            Scenario {
                name: "K: H + demurrage AT SPEND (as implemented)",
                payout: Payout::ClusterTilted,
                emission: true,
                whale: Whale::Honest,
                demurrage: Demurrage::AtSpend,
            },
            Scenario {
                name: "L: K + whale split+churn (gaming attempt)",
                payout: Payout::ClusterTilted,
                emission: true,
                whale: Whale::SplitChurn,
                demurrage: Demurrage::AtSpend,
            },
            Scenario {
                name: "M: K + whale PARKS FOREVER (escape attempt)",
                payout: Payout::ClusterTilted,
                emission: true,
                whale: Whale::Parker,
                demurrage: Demurrage::AtSpend,
            },
        ];

        println!("Structural Gini Reduction Experiment");
        println!("=====================================");
        println!(
            "Economy: 100M BTH; 80 poor (5%, factor 1x) / 30 middle (25%, 2x) / 9 whales (65%, 6x) / 1 strategic whale (5%, 6x)"
        );
        println!(
            "Duration: {} blocks (~{} days) | {} tx/block, base fee {} | emission {}/block (~{:.1}%/yr of supply) | demurrage {}bps/yr at factor 6 | whale split {} (churn every {}d)",
            blocks,
            blocks / BLOCKS_PER_DAY,
            txs_per_block,
            base_fee,
            emission_per_block,
            (emission_per_block as f64 * 365.0 * BLOCKS_PER_DAY as f64) / total_wealth as f64 * 100.0,
            demurrage_bps,
            split,
            churn_days,
        );
        println!();
        println!("| Scenario | Gini0 | GiniF | dGini | vs A | Whale 5%-> | Whale net (BTH) | Poor 5%-> |");
        println!("|----------|-------|-------|-------|------|------------|-----------------|-----------|");

        let churn_interval = churn_days.max(1) * BLOCKS_PER_DAY;
        let daily_demurrage = demurrage_bps as f64 / 10_000.0 / 365.0;
        let mut gini_delta_a: Option<f64> = None;

        for sc in &scenarios {
            let mut config = LotteryConfig::combined_mechanism();
            config.base_fee = base_fee;
            if sc.demurrage == Demurrage::AtSpend {
                config.demurrage_at_spend_bps = demurrage_bps;
                config.blocks_per_year = BLOCKS_PER_DAY * 365;
            }
            config.selection_mode = match sc.payout {
                Payout::Vw => SelectionMode::ValueWeightedWithFloor {
                    ticket_threshold: 1_000 * BTH,
                    decay_rate_per_day: 0.03,
                    min_eligibility: 0.10,
                    blocks_per_day: BLOCKS_PER_DAY,
                },
                // u64::MAX threshold => every UTXO gets exactly 1 ticket:
                // uniform-per-UTXO with eligibility decay
                Payout::Uniform => SelectionMode::ValueWeightedWithFloor {
                    ticket_threshold: u64::MAX,
                    decay_rate_per_day: 0.03,
                    min_eligibility: 0.10,
                    blocks_per_day: BLOCKS_PER_DAY,
                },
                // weight = value x (max_factor - factor + 1) / max_factor
                Payout::ClusterTilted => SelectionMode::ClusterWeighted,
            };

            let mut sim = LotterySimulation::new(config, FeeCurve::default_params());

            let mut poor_ids = Vec::new();
            for _ in 0..80 {
                poor_ids.push(sim.add_owner_with_factor(
                    total_wealth / 20 / 80,
                    SybilStrategy::Normal,
                    1.0,
                ));
            }
            for _ in 0..30 {
                sim.add_owner_with_factor(total_wealth / 4 / 30, SybilStrategy::Normal, 2.0);
            }
            for _ in 0..9 {
                sim.add_owner_with_factor(total_wealth * 65 / 100 / 9, SybilStrategy::Normal, 6.0);
            }
            let whale_strategy = match sc.whale {
                Whale::Honest => SybilStrategy::Normal,
                Whale::SplitChurn => SybilStrategy::MultiAccount {
                    num_accounts: split,
                },
                Whale::Parker => SybilStrategy::PermanentParker,
            };
            let whale_id = sim.add_owner_with_factor(total_wealth * 5 / 100, whale_strategy, 6.0);

            let owner_ids: Vec<u64> = sim.owners.keys().copied().collect();
            let total_value = |sim: &LotterySimulation| -> u64 {
                owner_ids.iter().map(|id| sim.owner_value(*id)).sum()
            };

            let gini0 = sim.calculate_gini();
            let whale_share0 = sim.owner_value(whale_id) as f64 / total_value(&sim) as f64;
            let poor_share0 = poor_ids.iter().map(|id| sim.owner_value(*id)).sum::<u64>() as f64
                / total_value(&sim) as f64;

            for b in 1..=blocks {
                sim.current_block += 1;

                for _ in 0..txs_per_block {
                    sim.simulate_transaction_immediate(
                        base_fee,
                        2,
                        TransactionModel::ValueWeighted,
                    );
                }

                if sc.emission {
                    sim.distribute_to_winners(emission_per_block, 4);
                }

                if sc.demurrage == Demurrage::Daily && b % BLOCKS_PER_DAY == 0 {
                    // Charge (factor-1)/5 x daily max rate on every UTXO,
                    // redistribute proceeds through the lottery
                    let mut total_charged = 0u64;
                    let charges: Vec<(u64, u64)> = sim
                        .utxos
                        .iter()
                        .filter_map(|(id, u)| {
                            let progressivity = ((u.cluster_factor - 1.0) / 5.0).clamp(0.0, 1.0);
                            let charge = (u.value as f64 * daily_demurrage * progressivity) as u64;
                            (charge > 0).then_some((*id, charge))
                        })
                        .collect();
                    for (id, charge) in charges {
                        if let Some(u) = sim.utxos.get_mut(&id) {
                            let charge = charge.min(u.value);
                            u.value -= charge;
                            total_charged += charge;
                            let owner_id = u.owner_id;
                            if let Some(o) = sim.owners.get_mut(&owner_id) {
                                o.total_fees_paid += charge;
                            }
                        }
                    }
                    sim.distribute_to_winners(total_charged, 16);
                }

                if sc.whale == Whale::SplitChurn && b % churn_interval == 0 {
                    sim.churn_owner(whale_id);
                }
            }

            let gini_f = sim.calculate_gini();
            let gini_delta = gini0 - gini_f; // positive = inequality reduced
            if gini_delta_a.is_none() {
                gini_delta_a = Some(gini_delta);
            }
            let vs_a = gini_delta - gini_delta_a.unwrap();

            let total_f = total_value(&sim) as f64;
            let whale_share_f = sim.owner_value(whale_id) as f64 / total_f;
            let poor_share_f =
                poor_ids.iter().map(|id| sim.owner_value(*id)).sum::<u64>() as f64 / total_f;
            let whale = sim.owners.get(&whale_id).unwrap();
            let whale_net = whale.total_winnings as i64 - whale.total_fees_paid as i64;

            println!(
                "| {} | {:.4} | {:.4} | {:+.4} | {:+.4} | {:.2}% -> {:.2}% | {:+} | {:.2}% -> {:.2}% |",
                sc.name,
                gini0,
                gini_f,
                gini_delta,
                vs_a,
                whale_share0 * 100.0,
                whale_share_f * 100.0,
                whale_net / BTH as i64,
                poor_share0 * 100.0,
                poor_share_f * 100.0,
            );
        }

        println!();
        println!("Reading the table:");
        println!("- dGini > 0 means inequality fell; the design criterion is dGini vs A > 0.05.");
        println!("- 'Whale net' is lottery winnings minus all fees/demurrage paid (BTH).");
        println!("- E vs D quantifies the gaming premium of uniform payouts.");
        println!("- G vs F tests whether demurrage-based redistribution is split-proof.");
        println!("- I vs H tests whether cluster-tilted payouts are split-proof.");
        println!("- J is the combined candidate: progressive intake (fees+demurrage) and");
        println!(
            "  progressive payout (cluster-tilted), both anchored to split-proof cluster tags."
        );
        println!("- K vs J compares spend-time demurrage (as implemented in the node) to the");
        println!("  daily balance charge the original validation assumed (issue #314).");
        println!("- M measures the permanent-parker escape: never spending avoids spend-time");
        println!("  demurrage entirely; only emission dilution touches parked wealth.");
    }

    // ========================================================================
    // M2 run matrix (#605 / #626 §7) — thin printing wrappers over the lib
    // harness `bth_cluster_tax::simulation::m2`, which exercises the REAL
    // production log-domain cluster-factor curve (not the #314 hardcoded
    // factors). The smoke tests live in that module and run under the default
    // `cargo test -p bth-cluster-tax`.
    // ========================================================================

    fn m2_pass(b: bool) -> &'static str {
        if b {
            "PASS"
        } else {
            "FAIL"
        }
    }
    fn m2_flag(b: bool) -> &'static str {
        if b {
            "FLAG"
        } else {
            "ok"
        }
    }

    fn print_m2_population() {
        use bth_cluster_tax::simulation::{lottery::LotterySimulation, m2::m2_population};
        for c in m2_population() {
            println!(
                "  cohort {:<9} n={:>3}  holdings={:>10} BTH  velocity={}x/yr  entry factor={:.3}x",
                c.name,
                c.count,
                c.holdings_bth,
                c.velocity_per_year,
                LotterySimulation::production_cluster_factor_bth(c.holdings_bth),
            );
        }
    }

    /// Run set 1: recalibrated-cumulative long-horizon run. Emits Δgini vs the
    /// >0.05 criterion plus merchant-mispricing and whale-progressivity health.
    fn run_m2_cumulative(horizon_years: u64, gamed: bool, seed: u64, smoke: bool) {
        use bth_cluster_tax::simulation::m2::{run_m2, M2Params};
        println!("M2 RECALIBRATED-CUMULATIVE (real log-domain curve, #626 §7 run set 1)");
        println!(
            "horizon={}yr  equilibrium={}  seed={}{}",
            horizon_years,
            if gamed { "gamed" } else { "honest" },
            seed,
            if smoke { "  [SMOKE]" } else { "" }
        );
        print_m2_population();
        let r = run_m2(&M2Params {
            horizon_years,
            half_life_years: None,
            gamed,
            seed,
            smoke,
        });
        println!(
            "gini0={:.4}  giniF={:.4}  dGini={:+.4}  (criterion >0.05: {})",
            r.gini0,
            r.gini_f,
            r.delta_gini,
            m2_pass(r.delta_gini > 0.05)
        );
        println!(
            "merchant mean factor={:.3}x  (mispricing flag >=3x: {})",
            r.merchant_mean_factor,
            m2_flag(r.merchant_mean_factor >= 3.0)
        );
        println!(
            "whale mean factor={:.3}x  (should stay >5x: {})",
            r.whale_mean_factor,
            m2_pass(r.whale_mean_factor > 5.0)
        );
    }

    /// Run set 2: epoch-halving decay variant. Emits Δgini plus the
    /// wash-trading evasion and privacy ring-identification metrics from
    /// experiments/ANALYSIS.md prior art.
    fn run_m2_decay(horizon_years: u64, half_life_years: u64, gamed: bool, seed: u64, smoke: bool) {
        use bth_cluster_tax::simulation::m2::{run_m2, M2Params};
        println!("M2 EPOCH-HALVING DECAY (real log-domain curve, #626 §7 run set 2)");
        println!(
            "horizon={}yr  half_life={}yr  equilibrium={}  seed={}{}",
            horizon_years,
            half_life_years,
            if gamed { "gamed" } else { "honest" },
            seed,
            if smoke { "  [SMOKE]" } else { "" }
        );
        print_m2_population();
        let r = run_m2(&M2Params {
            horizon_years,
            half_life_years: Some(half_life_years),
            gamed,
            seed,
            smoke,
        });
        let evasion = r.wash_evasion_pct.unwrap_or(0.0);
        let id_rate = r.ring_id_rate.unwrap_or(0.0);
        println!(
            "gini0={:.4}  giniF={:.4}  dGini={:+.4}  (criterion >0.05: {})",
            r.gini0,
            r.gini_f,
            r.delta_gini,
            m2_pass(r.delta_gini > 0.05)
        );
        println!(
            "wash-trading evasion={:.1}%  (criterion <20%: {})  [prior art 94-99% @ aggressive per-hop]",
            evasion,
            m2_pass(evasion < 20.0)
        );
        println!(
            "ring identification rate={:.1}%  (criterion <50%: {})  [prior art 78.7% @ 20% decay]",
            id_rate * 100.0,
            m2_pass(id_rate < 0.50)
        );
        println!(
            "merchant mean factor={:.3}x  whale mean factor={:.3}x",
            r.merchant_mean_factor, r.whale_mean_factor
        );
    }
    /// Emission-schedule sweep (issue #350).
    ///
    /// Runs the candidate schedule grid through both the analytic monetary
    /// model and the agent-based simulator, prints a comparison table, and
    /// writes a markdown report + CSV under `output`. Presents data and
    /// neutral observations only — recommends nothing.
    #[allow(clippy::too_many_arguments)]
    fn run_emission_sweep(
        rounds: u64,
        blocks_per_round: u64,
        retail: usize,
        merchants: usize,
        minters: usize,
        whales: usize,
        output: &str,
        quick: bool,
    ) {
        use bth_cluster_tax::simulation::emission_sweep::{
            run_sweep, to_csv, to_markdown, SweepParams,
        };
        use std::path::Path;

        let params = if quick {
            SweepParams {
                rounds: 400,
                blocks_per_round: 4,
                retail: 40,
                merchants: 6,
                minters: 2,
                whales: 2,
                snapshot_frequency: 50,
            }
        } else {
            SweepParams {
                rounds,
                blocks_per_round,
                retail,
                merchants,
                minters,
                whales,
                snapshot_frequency: (rounds / 20).max(1),
            }
        };

        println!("Emission-Schedule Sweep (issue #350)");
        println!("====================================");
        println!(
            "Distribution track: {} rounds x {} blocks = {} simulated blocks",
            params.rounds,
            params.blocks_per_round,
            params.total_blocks()
        );
        println!(
            "Population (fixed, deterministic): {} retail, {} merchants, {} whales, {} minters",
            params.retail, params.merchants, params.whales, params.minters
        );
        println!();

        let results = run_sweep(&params);

        // Print the markdown report to stdout for immediate inspection.
        let markdown = to_markdown(&results, &params);
        println!("{markdown}");

        // Write artifacts.
        let dir = Path::new(output);
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("Failed to create output dir {output}: {e}");
            return;
        }
        let md_path = dir.join("emission_sweep.md");
        let csv_path = dir.join("emission_sweep.csv");

        match std::fs::write(&md_path, &markdown) {
            Ok(()) => println!("Wrote report:  {}", md_path.display()),
            Err(e) => eprintln!("Failed to write {}: {e}", md_path.display()),
        }
        match std::fs::write(&csv_path, to_csv(&results)) {
            Ok(()) => println!("Wrote CSV:     {}", csv_path.display()),
            Err(e) => eprintln!("Failed to write {}: {e}", csv_path.display()),
        }
    }

    /// Decoy-quantile demurrage sweep (empirical gate for issue #577 / H2-B1).
    ///
    /// Compares the shipped value-weighted-mean age kernel against
    /// value-independent order statistics under an adversarial decoy
    /// population, and prints the gate table. Does NOT wire anything into
    /// consensus — #577's consensus wiring stays blocked under #323.
    #[allow(clippy::too_many_arguments)]
    fn run_decoy_quantile_sweep(
        poor: usize,
        honest_whales: usize,
        adversary_whales: usize,
        ring_size: usize,
        rounds: u64,
        rate_bps: u32,
        honest_jitter_bps: u32,
        seed: u64,
    ) {
        use bth_cluster_tax::simulation::decoy_quantile_sweep::{
            run_decoy_sweep, to_table, DecoySweepParams,
        };

        let defaults = DecoySweepParams::default();
        let params = DecoySweepParams {
            poor,
            honest_whales,
            adversary_whales,
            ring_size,
            rounds,
            rate_bps,
            honest_jitter_bps,
            seed,
            ..defaults
        };

        let report = run_decoy_sweep(&params);
        println!("{}", to_table(&report));
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
