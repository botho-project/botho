use anyhow::Result;
use bth_transaction_types::constants::Network;
use clap::{Parser, Subcommand};

use botho::{commands, config, telemetry};

#[derive(Parser)]
#[command(name = "botho")]
#[command(about = "A privacy-preserving mined cryptocurrency", long_about = None)]
struct Cli {
    /// Use testnet (default during beta)
    #[arg(long, global = true, conflicts_with = "mainnet")]
    testnet: bool,

    /// Use mainnet (requires BOTHO_ENABLE_MAINNET=1)
    #[arg(long, global = true, conflicts_with = "testnet")]
    mainnet: bool,

    /// Path to config file (default: ~/.botho/{network}/config.toml)
    #[arg(short, long, global = true)]
    config: Option<String>,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

impl Cli {
    /// Determine the network from CLI flags (defaults to testnet)
    fn network(&self) -> Network {
        if self.mainnet {
            Network::Mainnet
        } else {
            Network::Testnet
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new wallet or relay node
    Init {
        /// Recover wallet from existing mnemonic
        #[arg(long)]
        recover: bool,

        /// Create a relay node config (no wallet, for seed/infrastructure nodes)
        #[arg(long)]
        relay: bool,
    },

    /// Run the node (sync, scan wallet, optionally mine)
    Run {
        /// Enable minting
        #[arg(long)]
        mint: bool,

        /// Port for Prometheus metrics endpoint (overrides config, 0 to disable)
        #[arg(long)]
        metrics_port: Option<u16>,
    },

    /// Show node and wallet status
    Status,

    /// Show wallet balance
    Balance,

    /// Show receiving address
    Address {
        /// Save address to a file (use .botho for classical, .pq for quantum)
        #[arg(long)]
        save: Option<String>,
    },

    /// Send BTH to an address
    Send {
        /// Recipient address
        address: String,

        /// Amount to send (in BTH)
        amount: String,

        /// Use ring signatures for sender privacy (hides which UTXO you spent)
        #[arg(long)]
        private: bool,

        /// Use quantum-safe cryptography for post-quantum security
        /// Outputs use ML-KEM-768, inputs use Schnorr + ML-DSA-65 signatures
        #[arg(long)]
        quantum: bool,

        /// Attach an encrypted memo to the transaction (max 62 bytes)
        /// Only the recipient can decrypt and read this message
        #[arg(long)]
        memo: Option<String>,
    },

    /// Manage UTXO snapshots for fast initial sync
    Snapshot {
        #[command(subcommand)]
        action: SnapshotAction,
    },
}

#[derive(Subcommand)]
enum SnapshotAction {
    /// Create a snapshot of the current UTXO set
    Create {
        /// Output file path for the snapshot
        #[arg(short, long)]
        output: String,
    },

    /// Load a snapshot from a file
    Load {
        /// Input file path for the snapshot
        #[arg(short, long)]
        input: String,

        /// Expected block hash (hex) for verification
        #[arg(long)]
        verify_hash: Option<String>,
    },

    /// Show information about a snapshot file
    Info {
        /// Snapshot file path
        file: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Determine network (defaults to testnet)
    let network = cli.network();

    // Validate network is enabled
    config::validate_network(network)?;

    // Determine config path (network-specific by default)
    let config_path = cli.config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config::config_path(network));

    // For commands that run with an existing config, try to use telemetry settings.
    // For init command (no config yet), use basic logging.
    let _telemetry_guard = match &cli.command {
        Commands::Run { .. } => {
            // Try to load config for telemetry settings
            if let Ok(config) = config::Config::load(&config_path) {
                let telemetry_config = telemetry::TelemetryConfig {
                    enabled: config.telemetry.enabled,
                    endpoint: config.telemetry.endpoint.clone(),
                    service_name: config.telemetry.service_name.clone(),
                    sampling_rate: config.telemetry.sampling_rate,
                };
                telemetry::init_tracing(&telemetry_config, cli.verbose)?
            } else {
                // Config doesn't exist yet, use basic logging
                init_basic_tracing(cli.verbose);
                None
            }
        }
        _ => {
            // Other commands use basic logging
            init_basic_tracing(cli.verbose);
            None
        }
    };

    // Show network indicator
    if network.is_production() {
        eprintln!("[MAINNET] Using production network - transactions have real value!");
    } else {
        eprintln!("[TESTNET] Using test network - coins have no real value");
    }

    // Execute command
    match cli.command {
        Commands::Init { recover, relay } => {
            commands::init::run(&config_path, recover, relay, network)
        }
        Commands::Run { mint, metrics_port } => {
            commands::run::run(&config_path, mint, metrics_port)
        }
        Commands::Status => {
            commands::status::run(&config_path)
        }
        Commands::Balance => {
            commands::balance::run(&config_path)
        }
        Commands::Address { save } => {
            commands::address::run(&config_path, save.as_deref())
        }
        Commands::Send { address, amount, private, quantum, memo } => {
            commands::send::run(&config_path, &address, &amount, private, quantum, memo.as_deref())
        }
        Commands::Snapshot { action } => match action {
            SnapshotAction::Create { output } => {
                commands::snapshot::create(&config_path, &output)
            }
            SnapshotAction::Load { input, verify_hash } => {
                commands::snapshot::load(&config_path, &input, verify_hash.as_deref())
            }
            SnapshotAction::Info { file } => {
                commands::snapshot::info(&file)
            }
        }
    }
}

/// Initialize basic tracing without OpenTelemetry
fn init_basic_tracing(verbose: bool) {
    let level = if verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .init();
}
