use anyhow::Result;
use bth_transaction_types::constants::Network;
use clap::{Parser, Subcommand};

use botho::{commands, config};

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
        /// Enable mining
        #[arg(long)]
        mine: bool,
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

    /// Send credits to an address
    Send {
        /// Recipient address
        address: String,

        /// Amount to send (in credits)
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Determine network (defaults to testnet)
    let network = cli.network();

    // Validate network is enabled
    config::validate_network(network)?;

    // Initialize simple logging
    let level = if cli.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .init();

    // Show network indicator
    if network.is_production() {
        eprintln!("[MAINNET] Using production network - transactions have real value!");
    } else {
        eprintln!("[TESTNET] Using test network - coins have no real value");
    }

    // Determine config path (network-specific by default)
    let config_path = cli.config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| config::config_path(network));

    // Execute command
    match cli.command {
        Commands::Init { recover, relay } => {
            commands::init::run(&config_path, recover, relay, network)
        }
        Commands::Run { mine } => {
            commands::run::run(&config_path, mine)
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
    }
}
