use anyhow::Result;
use clap::{Parser, Subcommand};

use botho::{commands, config};

#[derive(Parser)]
#[command(name = "botho")]
#[command(about = "A privacy-preserving mined cryptocurrency", long_about = None)]
struct Cli {
    /// Path to config file (default: ~/.botho/config.toml)
    #[arg(short, long, global = true)]
    config: Option<String>,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
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
    Address,

    /// Send credits to an address
    Send {
        /// Recipient address
        address: String,

        /// Amount to send (in credits)
        amount: String,

        /// Use ring signatures for sender privacy (hides which UTXO you spent)
        #[arg(long)]
        private: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

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

    // Determine config path
    let config_path = cli.config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(config::default_config_path);

    // Execute command
    match cli.command {
        Commands::Init { recover, relay } => {
            commands::init::run(&config_path, recover, relay)
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
        Commands::Address => {
            commands::address::run(&config_path)
        }
        Commands::Send { address, amount, private } => {
            commands::send::run(&config_path, &address, &amount, private)
        }
    }
}
