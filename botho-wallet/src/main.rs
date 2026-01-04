//! Botho Thin Wallet CLI
//!
//! A standalone wallet for the Botho cryptocurrency network.

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod commands;
mod discovery;
mod keys;
mod rpc_pool;
mod secmem;
mod storage;
mod transaction;

#[derive(Parser)]
#[command(name = "botho-wallet")]
#[command(about = "Botho thin wallet - manage your BTH securely")]
#[command(version)]
struct Cli {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Custom wallet file path
    #[arg(short, long, global = true)]
    wallet: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new wallet
    Init {
        /// Recover from existing mnemonic
        #[arg(long)]
        recover: bool,
    },

    /// Show wallet receive address
    Address {
        /// Show quantum-safe (post-quantum) address
        #[arg(long)]
        pq: bool,
    },

    /// Check wallet balance
    Balance {
        /// Show detailed UTXO breakdown
        #[arg(long)]
        detailed: bool,
    },

    /// Send BTH to an address
    Send {
        /// Recipient address
        address: String,

        /// Amount to send in CAD
        amount: f64,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,

        /// Use quantum-private transaction (larger, higher fee, post-quantum
        /// secure)
        #[arg(long)]
        quantum_private: bool,
    },

    /// Sync wallet with the network
    Sync {
        /// Force full rescan from genesis
        #[arg(long)]
        full: bool,
    },

    /// Show transaction history
    History {
        /// Maximum number of transactions to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },

    /// Export wallet backup
    Export {
        /// Output file path
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Show connected nodes
    Nodes {
        /// Discover new nodes
        #[arg(long)]
        discover: bool,
    },

    /// Migrate funds to quantum-safe address
    ///
    /// Sweeps all classical UTXOs to your quantum-safe address, protecting
    /// funds against future quantum computer attacks. Uses ML-KEM-768 and
    /// ML-DSA-65 (NIST post-quantum standards).
    MigrateToPq {
        /// Preview migration without making changes
        #[arg(long)]
        dry_run: bool,

        /// Show current migration status only
        #[arg(long)]
        status: bool,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    // Determine wallet path
    let wallet_path = cli.wallet.map(std::path::PathBuf::from).unwrap_or_else(|| {
        dirs::home_dir()
            .expect("Could not find home directory")
            .join(".botho-wallet")
            .join("wallet.dat")
    });

    match cli.command {
        Commands::Init { recover } => commands::init::run(&wallet_path, recover).await,
        Commands::Address { pq } => commands::address::run(&wallet_path, pq).await,
        Commands::Balance { detailed } => commands::balance::run(&wallet_path, detailed).await,
        Commands::Send {
            address,
            amount,
            yes,
            quantum_private,
        } => commands::send::run(&wallet_path, &address, amount, yes, quantum_private).await,
        Commands::Sync { full } => commands::sync::run(&wallet_path, full).await,
        Commands::History { limit } => commands::history::run(&wallet_path, limit).await,
        Commands::Export { output } => commands::export::run(&wallet_path, output).await,
        Commands::Nodes { discover } => commands::nodes::run(&wallet_path, discover).await,
        Commands::MigrateToPq {
            dry_run,
            status,
            yes,
        } => commands::migrate_to_pq::run(&wallet_path, dry_run, status, yes).await,
    }
}
