// Copyright (c) 2024 Botho Foundation

//! Local Testnet Harness
//!
//! This tool spawns and manages a local multi-node Botho network for testing.
//!
//! # Usage
//!
//! ```bash
//! # Start a 5-node local testnet
//! cargo run --release --bin botho-testnet -- start --nodes 5
//!
//! # Check status of running nodes
//! cargo run --release --bin botho-testnet -- status
//!
//! # Send test transaction between nodes
//! cargo run --release --bin botho-testnet -- send --from 0 --to 1 --amount 100
//!
//! # Stop all nodes
//! cargo run --release --bin botho-testnet -- stop
//! ```
//!
//! # Architecture
//!
//! The testnet harness spawns N independent botho node processes, each with:
//! - Unique gossip and RPC ports (base_port + N, base_port + 100 + N)
//! - Isolated ledger directory (/tmp/botho-testnet/node-N/)
//! - Pre-funded wallet from deterministic seed
//! - Auto-configured quorum set for BFT consensus
//!
//! All nodes connect to each other via localhost bootstrap peers.

use std::{
    fs::{self, File},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use bip39::{Language, Mnemonic, Seed};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Base directory for testnet data
const TESTNET_DIR: &str = "/tmp/botho-testnet";

/// Default number of nodes
const DEFAULT_NODES: usize = 5;

/// Default base port for gossip
const DEFAULT_BASE_PORT: u16 = 27100;

/// RPC port offset from gossip port
const RPC_PORT_OFFSET: u16 = 100;

/// Timeout for node startup (seconds)
const STARTUP_TIMEOUT_SECS: u64 = 30;

/// Timeout for RPC requests (seconds)
const RPC_TIMEOUT_SECS: u64 = 10;

#[derive(Parser)]
#[command(name = "botho-testnet")]
#[command(about = "Local multi-node testnet harness for Botho")]
struct Args {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a local testnet with N nodes
    Start {
        /// Number of nodes to spawn (2-10)
        #[arg(short, long, default_value_t = DEFAULT_NODES)]
        nodes: usize,

        /// Base port for gossip connections (RPC = base + 100)
        #[arg(long, default_value_t = DEFAULT_BASE_PORT)]
        base_port: u16,

        /// Clean existing data before starting
        #[arg(long)]
        clean: bool,

        /// Wait for consensus before returning
        #[arg(long)]
        wait_consensus: bool,
    },

    /// Stop all running testnet nodes
    Stop,

    /// Show status of testnet nodes
    Status,

    /// Send a test transaction between nodes
    Send {
        /// Source node index (0-based)
        #[arg(long)]
        from: usize,

        /// Destination node index (0-based)
        #[arg(long)]
        to: usize,

        /// Amount to send in BTH
        #[arg(long)]
        amount: u64,
    },

    /// Kill a specific node (for chaos testing)
    KillNode {
        /// Node index to kill (0-based)
        #[arg(long)]
        node: usize,
    },

    /// Restart a killed node
    RestartNode {
        /// Node index to restart (0-based)
        #[arg(long)]
        node: usize,
    },

    /// Clean all testnet data
    Clean,

    /// Provision bridge reserve + user wallet key material for the full-loop
    /// e2e (#999).
    ///
    /// Writes 32-byte-hex classical view/spend key files plus a 64-byte-hex
    /// ML-KEM/ML-DSA BIP39 seed file for two wallets and prints the shell
    /// `export` lines the bridge full-loop driver
    /// (`scripts/bridge-e2e-full-loop.sh`) consumes via `eval`. NO secret
    /// is committed: the reserve keys derive from the harness's own
    /// deterministic node mnemonic (already in-code and disposable) and the
    /// user keys are freshly random at runtime.
    ///
    /// The RESERVE is the node's own pre-funded mining wallet. NOTE (#1025): a
    /// freshly-mined node accrues ONLY 100%-cluster-tagged coinbases
    /// (`MintingTx::to_tx_output` tags each with a new cluster) and lottery
    /// EMISSION is zero in the bootstrap epoch — so the reserve does NOT
    /// naturally own the **factor-1** (zero-cluster-weight) outputs the CLSAG
    /// release path spends (ADR 0003). Run `fund-reserve` after this to settle
    /// a coinbase into a spendable factor-1 output. The USER wallet only
    /// receives the released stealth output (ADR 0004).
    GenBridgeKeys {
        /// Node index whose deterministic wallet becomes the bridge reserve.
        #[arg(long, default_value_t = 0)]
        node: usize,

        /// Directory to write the key files into (created if missing).
        #[arg(long)]
        out: PathBuf,
    },

    /// Fund the bridge reserve with a spendable **factor-1** (background)
    /// output by settling the node wallet's own coins (#1025).
    ///
    /// A freshly-mined node accrues ONLY 100%-cluster-tagged coinbases, never
    /// factor-1, so the bridge releaser (which spends only factor-1 reserve
    /// outputs, ADR 0003) would find nothing. This calls the node's
    /// `dev_settleToBackground` RPC (testnet-only) to emit an explicit
    /// demurrage-settlement that reclassifies a coinbase to factor-1, then
    /// waits for it to be mined — retrying while the chain is still warming up
    /// (insufficient ring decoys / no mature coinbase yet).
    FundReserve {
        /// Node index whose wallet is the reserve (matches `gen-bridge-keys`).
        #[arg(long, default_value_t = 0)]
        node: usize,

        /// Picocredits to settle to factor-1 (must cover the wrap amount +
        /// release fee). Default 1 BTH.
        #[arg(long, default_value_t = 1_000_000_000_000)]
        amount: u64,

        /// Base gossip port (RPC = base + 100 + node); locates the node RPC.
        #[arg(long, default_value_t = DEFAULT_BASE_PORT)]
        base_port: u16,

        /// Max seconds to wait for the settlement to succeed and be mined.
        #[arg(long, default_value_t = 900)]
        timeout_secs: u64,
    },
}

/// Testnet state persisted to disk
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestnetState {
    /// Number of nodes
    node_count: usize,
    /// Base gossip port
    base_port: u16,
    /// Node configurations
    nodes: Vec<NodeState>,
    /// Timestamp when started
    started_at: String,
}

/// Per-node state
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeState {
    /// Node index
    index: usize,
    /// Gossip port
    gossip_port: u16,
    /// RPC port
    rpc_port: u16,
    /// Data directory
    data_dir: PathBuf,
    /// PID file path
    pid_file: PathBuf,
    /// Log file path
    log_file: PathBuf,
    /// Node's public address (for receiving funds)
    address: Option<String>,
}

impl TestnetState {
    fn state_file() -> PathBuf {
        PathBuf::from(TESTNET_DIR).join("state.json")
    }

    fn load() -> Result<Option<Self>> {
        let path = Self::state_file();
        if !path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(&path)?;
        Ok(Some(serde_json::from_str(&contents)?))
    }

    fn save(&self) -> Result<()> {
        let path = Self::state_file();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(&path, contents)?;
        Ok(())
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Commands::Start {
            nodes,
            base_port,
            clean,
            wait_consensus,
        } => cmd_start(nodes, base_port, clean, wait_consensus, args.verbose),
        Commands::Stop => cmd_stop(args.verbose),
        Commands::Status => cmd_status(args.verbose),
        Commands::Send { from, to, amount } => cmd_send(from, to, amount, args.verbose),
        Commands::KillNode { node } => cmd_kill_node(node, args.verbose),
        Commands::RestartNode { node } => cmd_restart_node(node, args.verbose),
        Commands::Clean => cmd_clean(args.verbose),
        Commands::GenBridgeKeys { node, out } => cmd_gen_bridge_keys(node, &out),
        Commands::FundReserve {
            node,
            amount,
            base_port,
            timeout_secs,
        } => cmd_fund_reserve(node, amount, base_port, timeout_secs),
    }
}

/// Start the testnet
fn cmd_start(
    node_count: usize,
    base_port: u16,
    clean: bool,
    wait_consensus: bool,
    verbose: bool,
) -> Result<()> {
    // Validate node count
    if !(2..=10).contains(&node_count) {
        return Err(anyhow!("Node count must be between 2 and 10"));
    }

    // Check for existing testnet
    if let Some(state) = TestnetState::load()? {
        // Check if any nodes are still running
        let running = state.nodes.iter().filter(|n| is_node_running(n)).count();
        if running > 0 {
            return Err(anyhow!(
                "Testnet already running with {} nodes. Use 'stop' first.",
                running
            ));
        }
    }

    // Clean if requested
    if clean {
        cmd_clean(verbose)?;
    }

    println!("Starting {} node testnet...", node_count);
    println!(
        "  Base port: {} (gossip), {} (RPC)",
        base_port,
        base_port + RPC_PORT_OFFSET
    );
    println!("  Data dir: {}", TESTNET_DIR);

    // Create testnet directory
    fs::create_dir_all(TESTNET_DIR)?;

    // Generate node configurations
    let mut nodes = Vec::with_capacity(node_count);
    for i in 0..node_count {
        let gossip_port = base_port + i as u16;
        let rpc_port = base_port + RPC_PORT_OFFSET + i as u16;
        let data_dir = PathBuf::from(TESTNET_DIR).join(format!("node-{}", i));

        nodes.push(NodeState {
            index: i,
            gossip_port,
            rpc_port,
            data_dir: data_dir.clone(),
            pid_file: data_dir.join("botho.pid"),
            log_file: data_dir.join("botho.log"),
            address: None,
        });
    }

    // Initialize and start each node
    let mut children: Vec<Child> = Vec::new();

    for i in 0..nodes.len() {
        let node = &nodes[i];
        println!("\n[Node {}] Initializing...", node.index);

        // Create node data directory
        fs::create_dir_all(&node.data_dir)?;

        // Generate deterministic mnemonic for this node
        let mnemonic = generate_deterministic_mnemonic(node.index)?;

        // Create config file
        let config = create_node_config(node, &nodes, &mnemonic)?;
        let config_path = node.data_dir.join("config.toml");
        fs::write(&config_path, config)?;

        if verbose {
            println!("  Config: {}", config_path.display());
            println!("  Gossip: {}, RPC: {}", node.gossip_port, node.rpc_port);
        }

        // Start the node process
        let child = start_node_process(node, &config_path, verbose)?;
        let pid = child.id();

        // Save PID
        fs::write(&node.pid_file, pid.to_string())?;

        children.push(child);
        println!("[Node {}] Started (PID: {})", node.index, pid);
    }

    // Wait for nodes to be ready
    println!("\nWaiting for nodes to initialize...");
    let start = Instant::now();
    let mut ready_count = 0;

    while start.elapsed().as_secs() < STARTUP_TIMEOUT_SECS {
        ready_count = 0;
        for node in &nodes {
            if check_node_rpc(node) {
                ready_count += 1;
            }
        }
        if ready_count == node_count {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }

    if ready_count < node_count {
        println!(
            "Warning: Only {}/{} nodes responded to RPC within timeout",
            ready_count, node_count
        );
    } else {
        println!("All {} nodes ready!", node_count);
    }

    // Get addresses for each node
    for node in &mut nodes {
        if let Ok(addr) = get_node_address(node) {
            node.address = Some(addr.clone());
            if verbose {
                println!("[Node {}] Address: {}", node.index, addr);
            }
        }
    }

    // Save state
    let state = TestnetState {
        node_count,
        base_port,
        nodes: nodes.clone(),
        started_at: chrono::Utc::now().to_rfc3339(),
    };
    state.save()?;

    // Wait for consensus if requested
    if wait_consensus {
        println!("\nWaiting for consensus...");
        wait_for_consensus(&nodes, STARTUP_TIMEOUT_SECS)?;
    }

    println!("\nTestnet started successfully!");
    println!("\nUseful commands:");
    println!("  botho-testnet status              # Check node status");
    println!("  botho-testnet send --from 0 --to 1 --amount 100");
    println!("  botho-testnet stop                # Stop all nodes");

    Ok(())
}

/// Stop all testnet nodes
fn cmd_stop(verbose: bool) -> Result<()> {
    let state = TestnetState::load()?.ok_or_else(|| anyhow!("No testnet state found"))?;

    println!("Stopping {} nodes...", state.nodes.len());

    for node in &state.nodes {
        if let Some(pid) = read_pid(&node.pid_file) {
            if verbose {
                println!("[Node {}] Sending SIGTERM to PID {}", node.index, pid);
            }

            // Send SIGTERM
            #[cfg(unix)]
            {
                use nix::{
                    sys::signal::{kill, Signal},
                    unistd::Pid,
                };
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
            }

            #[cfg(not(unix))]
            {
                // On non-Unix, try to kill via taskkill
                let _ = Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output();
            }
        }
    }

    // Wait a moment for graceful shutdown
    thread::sleep(Duration::from_secs(2));

    // Force kill any remaining
    for node in &state.nodes {
        if let Some(pid) = read_pid(&node.pid_file) {
            if is_process_running(pid) {
                if verbose {
                    println!("[Node {}] Force killing PID {}", node.index, pid);
                }
                #[cfg(unix)]
                {
                    use nix::{
                        sys::signal::{kill, Signal},
                        unistd::Pid,
                    };
                    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
                }
            }
        }
        // Clean up PID file
        let _ = fs::remove_file(&node.pid_file);
    }

    println!("All nodes stopped.");
    Ok(())
}

/// Show testnet status
fn cmd_status(verbose: bool) -> Result<()> {
    let state = TestnetState::load()?;

    match state {
        None => {
            println!("No testnet running.");
            println!("Use 'botho-testnet start' to create one.");
            return Ok(());
        }
        Some(state) => {
            println!("Testnet Status");
            println!("==============");
            println!("Started: {}", state.started_at);
            println!("Nodes: {}", state.node_count);
            println!("Base port: {}", state.base_port);
            println!();

            for node in &state.nodes {
                let running = is_node_running(node);
                let rpc_ok = if running { check_node_rpc(node) } else { false };

                let status = if running && rpc_ok {
                    "✓ Running"
                } else if running {
                    "⚠ Running (RPC unresponsive)"
                } else {
                    "✗ Stopped"
                };

                println!(
                    "[Node {}] {} (gossip:{}, rpc:{})",
                    node.index, status, node.gossip_port, node.rpc_port
                );

                if verbose {
                    if let Some(addr) = &node.address {
                        println!("         Address: {}", addr);
                    }
                    if let Some(pid) = read_pid(&node.pid_file) {
                        println!("         PID: {}", pid);
                    }

                    // Try to get block height
                    if rpc_ok {
                        if let Ok(height) = get_block_height(node) {
                            println!("         Block height: {}", height);
                        }
                    }
                }
            }

            // Show consensus status
            println!();
            let running_count = state.nodes.iter().filter(|n| is_node_running(n)).count();
            let quorum_threshold = state.node_count.div_ceil(2) + 1; // BFT threshold
            if running_count >= quorum_threshold {
                println!(
                    "Quorum: {}/{} nodes running (threshold: {})",
                    running_count, state.node_count, quorum_threshold
                );
            } else {
                println!(
                    "Warning: Only {}/{} nodes running, need {} for quorum",
                    running_count, state.node_count, quorum_threshold
                );
            }
        }
    }

    Ok(())
}

/// Send a test transaction
fn cmd_send(from: usize, to: usize, amount: u64, verbose: bool) -> Result<()> {
    let state = TestnetState::load()?.ok_or_else(|| anyhow!("No testnet running"))?;

    if from >= state.nodes.len() {
        return Err(anyhow!("Invalid 'from' node index: {}", from));
    }
    if to >= state.nodes.len() {
        return Err(anyhow!("Invalid 'to' node index: {}", to));
    }
    if from == to {
        return Err(anyhow!("Cannot send to same node"));
    }

    let from_node = &state.nodes[from];
    let to_node = &state.nodes[to];

    // Get destination address
    let to_address = to_node
        .address
        .as_ref()
        .ok_or_else(|| anyhow!("Node {} has no address", to))?;

    println!(
        "Sending {} BTH from node {} to node {}...",
        amount, from, to
    );
    if verbose {
        println!("  To address: {}", to_address);
    }

    // Make RPC call to send
    let rpc_url = format!("http://127.0.0.1:{}", from_node.rpc_port);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "send_transaction",
        "params": {
            "to": to_address,
            "amount": amount,
            "private": false
        }
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(RPC_TIMEOUT_SECS))
        .build()?;

    let response = client.post(&rpc_url).json(&request).send()?;

    if response.status().is_success() {
        let result: serde_json::Value = response.json()?;
        if let Some(tx_hash) = result.get("result").and_then(|r| r.get("tx_hash")) {
            println!("Transaction submitted: {}", tx_hash);
        } else if let Some(error) = result.get("error") {
            return Err(anyhow!("RPC error: {}", error));
        } else {
            println!("Transaction submitted successfully");
        }
    } else {
        return Err(anyhow!("RPC request failed: {}", response.status()));
    }

    Ok(())
}

/// Kill a specific node (for chaos testing)
fn cmd_kill_node(node_idx: usize, verbose: bool) -> Result<()> {
    let state = TestnetState::load()?.ok_or_else(|| anyhow!("No testnet running"))?;

    if node_idx >= state.nodes.len() {
        return Err(anyhow!("Invalid node index: {}", node_idx));
    }

    let node = &state.nodes[node_idx];
    if let Some(pid) = read_pid(&node.pid_file) {
        if verbose {
            println!("Killing node {} (PID: {})", node_idx, pid);
        }

        #[cfg(unix)]
        {
            use nix::{
                sys::signal::{kill, Signal},
                unistd::Pid,
            };
            kill(Pid::from_raw(pid as i32), Signal::SIGKILL)?;
        }

        // Remove PID file
        let _ = fs::remove_file(&node.pid_file);
        println!("Node {} killed", node_idx);
    } else {
        println!("Node {} is not running", node_idx);
    }

    Ok(())
}

/// Restart a killed node
fn cmd_restart_node(node_idx: usize, verbose: bool) -> Result<()> {
    let mut state = TestnetState::load()?.ok_or_else(|| anyhow!("No testnet running"))?;

    if node_idx >= state.nodes.len() {
        return Err(anyhow!("Invalid node index: {}", node_idx));
    }

    let node = &state.nodes[node_idx];
    if is_node_running(node) {
        return Err(anyhow!("Node {} is already running", node_idx));
    }

    let config_path = node.data_dir.join("config.toml");
    if !config_path.exists() {
        return Err(anyhow!(
            "Config not found for node {}. Was it initialized?",
            node_idx
        ));
    }

    println!("Restarting node {}...", node_idx);

    let child = start_node_process(node, &config_path, verbose)?;
    let pid = child.id();

    // Save PID
    fs::write(&node.pid_file, pid.to_string())?;

    // Wait for node to be ready
    let start = Instant::now();
    while start.elapsed().as_secs() < STARTUP_TIMEOUT_SECS {
        if check_node_rpc(node) {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }

    if check_node_rpc(node) {
        println!("Node {} restarted (PID: {})", node_idx, pid);

        // Update address if needed
        if let Ok(addr) = get_node_address(node) {
            state.nodes[node_idx].address = Some(addr);
            state.save()?;
        }
    } else {
        println!(
            "Node {} started but RPC not responding (PID: {})",
            node_idx, pid
        );
    }

    Ok(())
}

/// Clean all testnet data
fn cmd_clean(verbose: bool) -> Result<()> {
    // First stop any running nodes
    if let Ok(Some(_)) = TestnetState::load() {
        let _ = cmd_stop(verbose);
    }

    // Remove testnet directory
    let testnet_dir = PathBuf::from(TESTNET_DIR);
    if testnet_dir.exists() {
        if verbose {
            println!("Removing {}", testnet_dir.display());
        }
        fs::remove_dir_all(&testnet_dir)?;
    }

    println!("Testnet data cleaned.");
    Ok(())
}

// ============================================================================
// Bridge key provisioning (#999)
// ============================================================================

/// One wallet's bridge key material: the on-disk file contents plus the
/// published v2 (`tbotho://2/…`) receive address.
struct BridgeKeyMaterial {
    /// 32-byte hex Ristretto view private scalar (`bth.view_key_file`).
    view_hex: String,
    /// 32-byte hex Ristretto spend private scalar (`bth.spend_key_file`).
    spend_hex: String,
    /// 64-byte hex BIP39 seed the reserve derives its ML-KEM-768 / ML-DSA-65
    /// keypairs from (`bth.pq_seed_file`, issue #972). Required on the
    /// protocol-6.0.0 hybrid chain so the scanner can decapsulate outputs paid
    /// to this wallet.
    pq_seed_hex: String,
    /// Published v2 address (carries the classical + ML-KEM + ML-DSA public
    /// keys), byte-identical to what `bth-bridge-service`'s `ReserveKeys`
    /// advertises from the same files.
    address: String,
}

/// Derive the bridge key material for a BIP39 mnemonic.
///
/// The classical view/spend private scalars come from the SAME SLIP-10 path
/// the node wallet uses (`botho::wallet::Wallet::from_mnemonic`), so the
/// derived reserve keys are exactly the node's own keys and detect the node's
/// coinbase/lottery outputs. The PQ seed is the 64-byte BIP39 seed, and the
/// published address is reconstructed the way `ReserveKeys::public_address`
/// does (`AccountKey::new(spend, view).default_subaddress().with_pq_keys(…)`),
/// so the printed `*_ADDRESS` matches what the reserve advertises.
fn derive_bridge_key_material(mnemonic_phrase: &str) -> Result<BridgeKeyMaterial> {
    use bth_account_keys::AccountKey;
    use bth_address_codec::{encode_address, Network};
    use bth_crypto_keys::RistrettoPrivate;
    use bth_crypto_pq::derive_pq_keys_from_seed;

    // Classical keys via the tested node-wallet derivation (SLIP-10 + the same
    // key material the running node uses to receive funds).
    let wallet = botho::wallet::Wallet::from_mnemonic(mnemonic_phrase)
        .map_err(|e| anyhow!("derive wallet from mnemonic: {e}"))?;
    let view_priv = wallet.account_key().view_private_key().to_bytes();
    let spend_priv = wallet.account_key().spend_private_key().to_bytes();

    // 64-byte BIP39 seed → ML-KEM-768 / ML-DSA-65 material (issue #972). Same
    // `Seed::new(&mnemonic, "")` the wallet feeds `derive_pq_keys_from_seed`,
    // so the reserve's published KEM key matches the secret it decapsulates
    // with.
    let mnemonic = Mnemonic::from_phrase(mnemonic_phrase, Language::English)
        .map_err(|e| anyhow!("invalid mnemonic: {e}"))?;
    let seed = Seed::new(&mnemonic, "");
    let seed_bytes: [u8; 64] = seed
        .as_bytes()
        .try_into()
        .map_err(|_| anyhow!("BIP39 seed must be 64 bytes"))?;
    let pq = derive_pq_keys_from_seed(&seed_bytes);
    let kem_pub = pq.kem_keypair.public_key().as_bytes().to_vec();
    let dsa_pub = pq.sig_keypair.public_key().as_bytes().to_vec();

    // Reconstruct the published v2 address exactly like `ReserveKeys` does from
    // the on-disk files, so config validation + the reconciler custody query
    // see the same address the reserve advertises.
    let account = AccountKey::new(
        &RistrettoPrivate::try_from(&spend_priv).map_err(|e| anyhow!("spend scalar: {e:?}"))?,
        &RistrettoPrivate::try_from(&view_priv).map_err(|e| anyhow!("view scalar: {e:?}"))?,
    );
    let address = encode_address(
        &account.default_subaddress().with_pq_keys(kem_pub, dsa_pub),
        Network::Testnet,
    )
    .map_err(|e| anyhow!("encode v2 address: {e}"))?;

    Ok(BridgeKeyMaterial {
        view_hex: hex::encode(view_priv),
        spend_hex: hex::encode(spend_priv),
        pq_seed_hex: hex::encode(seed_bytes),
        address,
    })
}

/// Write a key file with owner-only permissions (0600 on Unix).
fn write_key_file(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, format!("{contents}\n"))
        .with_context(|| format!("write key file {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 600 {}", path.display()))?;
    }
    Ok(())
}

/// Materialize one wallet's key files under `dir` with the given `role` prefix
/// and print the matching `export` lines for the bridge full-loop driver.
fn emit_bridge_wallet(dir: &Path, role: &str, upper: &str, keys: &BridgeKeyMaterial) -> Result<()> {
    let view_path = dir.join(format!("{role}.view.hex"));
    let spend_path = dir.join(format!("{role}.spend.hex"));
    let pq_seed_path = dir.join(format!("{role}.pq_seed.hex"));

    write_key_file(&view_path, &keys.view_hex)?;
    write_key_file(&spend_path, &keys.spend_hex)?;
    write_key_file(&pq_seed_path, &keys.pq_seed_hex)?;

    // Only the `export` lines go to stdout so the driver can `eval` them; all
    // human-facing logs go to stderr. Values are double-quoted so paths with
    // shell-special characters survive `eval`.
    println!(
        "export BRIDGE_BTH_{upper}_VIEW_KEY=\"{}\"",
        view_path.display()
    );
    println!(
        "export BRIDGE_BTH_{upper}_SPEND_KEY=\"{}\"",
        spend_path.display()
    );
    println!(
        "export BRIDGE_BTH_{upper}_PQ_SEED=\"{}\"",
        pq_seed_path.display()
    );
    println!("export BRIDGE_BTH_{upper}_ADDRESS=\"{}\"", keys.address);
    Ok(())
}

/// Provision bridge reserve + user wallet key material (#999).
fn cmd_gen_bridge_keys(node: usize, out: &Path) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("create key dir {}", out.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(out, fs::Permissions::from_mode(0o700));
    }

    // RESERVE = the node's own deterministic (pre-funded) wallet. It accrues
    // cluster-tagged coinbases; `fund-reserve` settles one into factor-1.
    // Deriving from the SAME mnemonic the harness boots node `node`
    // with means the reserve keys detect that node's coinbase + lottery
    // outputs — no secret is introduced.
    let reserve_mnemonic = generate_deterministic_mnemonic(node)?;
    let reserve = derive_bridge_key_material(&reserve_mnemonic)?;

    // USER = a freshly random wallet (only ever receives the released stealth
    // output). Never persisted anywhere but the disposable key dir.
    let mut entropy = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut entropy);
    let user_mnemonic = Mnemonic::from_entropy(&entropy, Language::English)
        .map_err(|e| anyhow!("generate user mnemonic: {e}"))?
        .phrase()
        .to_string();
    let user = derive_bridge_key_material(&user_mnemonic)?;

    eprintln!(
        "Provisioned bridge key material in {} (reserve = node {} wallet, user = fresh random)",
        out.display(),
        node
    );
    eprintln!("  reserve address: {}", reserve.address);
    eprintln!("  user address:    {}", user.address);

    emit_bridge_wallet(out, "reserve", "RESERVE", &reserve)?;
    emit_bridge_wallet(out, "user", "USER", &user)?;

    Ok(())
}

/// Fund the bridge reserve with a spendable factor-1 output (#1025).
///
/// Calls the node's testnet-only `dev_settleToBackground` RPC, which emits an
/// explicit demurrage-settlement reclassifying one of the node wallet's own
/// (cluster-tagged) coinbases to a factor-1/background output back to itself.
/// Retries while the chain is still warming up (no mature coinbase yet, or too
/// few decoys to form the ring), then waits for the settlement to be mined.
fn cmd_fund_reserve(node: usize, amount: u64, base_port: u16, timeout_secs: u64) -> Result<()> {
    let rpc_port = base_port + RPC_PORT_OFFSET + node as u16;
    let rpc_url = format!("http://127.0.0.1:{}", rpc_port);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(RPC_TIMEOUT_SECS))
        .build()?;

    eprintln!(
        "Funding reserve (node {}) with a factor-1 settlement of {} pc via {}",
        node, amount, rpc_url
    );

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    // 1) Retry the settlement until the chain has matured enough to build it (a
    //    mature coinbase to spend + DEFAULT_RING_SIZE-1 decoys for the ring).
    let tx_hash = loop {
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting to submit the reserve settlement (chain never \
                 warmed up enough: need a mature coinbase + a full decoy ring)"
            ));
        }

        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "dev_settleToBackground",
            "params": { "amount": amount },
        });
        match client.post(&rpc_url).json(&request).send() {
            Ok(resp) => {
                let body: serde_json::Value = resp.json().unwrap_or_else(|_| json!({}));
                if let Some(hash) = body
                    .get("result")
                    .and_then(|r| r.get("txHash"))
                    .and_then(|h| h.as_str())
                {
                    eprintln!("Reserve settlement submitted: tx {hash}");
                    break hash.to_string();
                }
                let msg = body
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                eprintln!("  settlement not ready yet ({msg}); retrying in 10s");
            }
            Err(e) => eprintln!("  RPC call failed ({e}); retrying in 10s"),
        }
        thread::sleep(Duration::from_secs(10));
    };

    // 2) Wait for the settlement to be mined (confirmed), so the reserve owns a
    //    spendable factor-1 output before the release leg runs.
    loop {
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "reserve settlement {tx_hash} submitted but not mined before timeout"
            ));
        }
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tx_get",
            "params": { "tx_hash": tx_hash },
        });
        if let Ok(resp) = client.post(&rpc_url).json(&request).send() {
            let body: serde_json::Value = resp.json().unwrap_or_else(|_| json!({}));
            if body
                .get("result")
                .and_then(|r| r.get("status"))
                .and_then(|s| s.as_str())
                == Some("confirmed")
            {
                let h = body
                    .get("result")
                    .and_then(|r| r.get("blockHeight"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                eprintln!("Reserve settlement {tx_hash} confirmed at block {h}");
                eprintln!("Reserve now owns a spendable factor-1 output.");
                return Ok(());
            }
        }
        thread::sleep(Duration::from_secs(5));
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Generate a deterministic mnemonic for a node index
fn generate_deterministic_mnemonic(node_index: usize) -> Result<String> {
    // Use a fixed seed phrase pattern for reproducibility
    let seed = format!("testnet-node-{}-seed-phrase-entropy", node_index);
    let entropy = sha256_hash(seed.as_bytes());

    // Create mnemonic from entropy (first 16 bytes = 128 bits = 12 words)
    let mnemonic = Mnemonic::from_entropy(&entropy[..16], Language::English)
        .map_err(|e| anyhow!("Failed to create mnemonic: {}", e))?;

    Ok(mnemonic.phrase().to_string())
}

/// Simple SHA256 hash
fn sha256_hash(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Create node config TOML
fn create_node_config(node: &NodeState, all_nodes: &[NodeState], mnemonic: &str) -> Result<String> {
    // Build bootstrap peers list (all other nodes)
    let bootstrap_peers: Vec<String> = all_nodes
        .iter()
        .filter(|n| n.index != node.index)
        .map(|n| format!("/ip4/127.0.0.1/tcp/{}", n.gossip_port))
        .collect();

    // Calculate quorum threshold (BFT optimal)
    let n = all_nodes.len();
    let threshold = n.div_ceil(2) + 1; // ceil((n+1)/2)

    let config = format!(
        r#"# Auto-generated config for testnet node {}
network_type = "testnet"

[wallet]
mnemonic = "{}"

[network]
gossip_port = {}
rpc_port = {}
cors_origins = ["*"]
bootstrap_peers = {:?}
max_connections_per_ip = 100

[network.quorum]
mode = "recommended"
threshold = {}
min_peers = 1

[minting]
enabled = true
threads = 1
"#,
        node.index, mnemonic, node.gossip_port, node.rpc_port, bootstrap_peers, threshold
    );

    Ok(config)
}

/// Start a node process
fn start_node_process(node: &NodeState, config_path: &Path, verbose: bool) -> Result<Child> {
    // Open log file
    let log_file = File::create(&node.log_file)?;

    // Find the botho binary
    let botho_bin = find_botho_binary()?;

    if verbose {
        println!("  Using binary: {}", botho_bin.display());
        println!("  Log file: {}", node.log_file.display());
    }

    let child = Command::new(&botho_bin)
        .args([
            "--testnet",
            "--config",
            config_path.to_str().unwrap(),
            "run",
            "--mint",
        ])
        // These are throwaway local harness nodes: opt in to the dev/test RPC
        // surface (`dev_settleToBackground` for reserve funding + the lifted
        // anonymous rate limit for the heavy e2e drive loops, #1025). A live
        // public testnet node does NOT set this, so it keeps its 100/min
        // anonymous limit and never exposes the mutating dev RPC (M1/L1).
        .env("BOTHO_ENABLE_DEV_RPC", "1")
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .with_context(|| format!("Failed to spawn botho process for node {}", node.index))?;

    Ok(child)
}

/// Find the botho binary (either in target/release or target/debug)
fn find_botho_binary() -> Result<PathBuf> {
    // Try release first
    let release_bin = PathBuf::from("target/release/botho");
    if release_bin.exists() {
        return Ok(release_bin);
    }

    // Try debug
    let debug_bin = PathBuf::from("target/debug/botho");
    if debug_bin.exists() {
        return Ok(debug_bin);
    }

    // Try system PATH
    if let Ok(path) = which::which("botho") {
        return Ok(path);
    }

    Err(anyhow!(
        "Could not find botho binary. Run 'cargo build --release' first."
    ))
}

/// Read PID from file
fn read_pid(pid_file: &Path) -> Option<u32> {
    fs::read_to_string(pid_file)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Check if a process is running
fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use nix::{sys::signal::kill, unistd::Pid};
        // Sending signal 0 checks if process exists
        kill(Pid::from_raw(pid as i32), None).is_ok()
    }

    #[cfg(not(unix))]
    {
        // On Windows, try to open the process
        false // Simplified for now
    }
}

/// Check if a node is running
fn is_node_running(node: &NodeState) -> bool {
    if let Some(pid) = read_pid(&node.pid_file) {
        is_process_running(pid)
    } else {
        false
    }
}

/// Check if node RPC is responding
fn check_node_rpc(node: &NodeState) -> bool {
    let addr = format!("127.0.0.1:{}", node.rpc_port);
    TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(1)).is_ok()
}

/// Get block height from node RPC
fn get_block_height(node: &NodeState) -> Result<u64> {
    let rpc_url = format!("http://127.0.0.1:{}", node.rpc_port);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "get_block_count",
        "params": {}
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(RPC_TIMEOUT_SECS))
        .build()?;

    let response: serde_json::Value = client.post(&rpc_url).json(&request).send()?.json()?;

    response
        .get("result")
        .and_then(|r| r.as_u64())
        .ok_or_else(|| anyhow!("Invalid response"))
}

/// Get node's receiving address via RPC
fn get_node_address(node: &NodeState) -> Result<String> {
    let rpc_url = format!("http://127.0.0.1:{}", node.rpc_port);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "get_address",
        "params": {}
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(RPC_TIMEOUT_SECS))
        .build()?;

    let response: serde_json::Value = client.post(&rpc_url).json(&request).send()?.json()?;

    response
        .get("result")
        .and_then(|r| r.get("address"))
        .and_then(|a| a.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow!("Could not get address from RPC"))
}

/// Wait for consensus to be achieved
fn wait_for_consensus(nodes: &[NodeState], timeout_secs: u64) -> Result<()> {
    let start = Instant::now();

    while start.elapsed().as_secs() < timeout_secs {
        // Check if all responsive nodes have the same block height > 0
        let heights: Vec<u64> = nodes
            .iter()
            .filter_map(|n| get_block_height(n).ok())
            .collect();

        if heights.len() >= 2 && heights.iter().all(|h| *h > 0) {
            let min = heights.iter().min().unwrap();
            let max = heights.iter().max().unwrap();

            // Allow small variance (1 block difference is OK)
            if max - min <= 1 {
                println!("Consensus achieved! Block height: {}-{}", min, max);
                return Ok(());
            }
        }

        thread::sleep(Duration::from_secs(1));
    }

    Err(anyhow!("Timeout waiting for consensus"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The derived key files have the exact on-disk shape the bridge loader
    /// (`bth-bridge-service`'s `ReserveKeys::load`) expects: 32-byte-hex
    /// classical scalars and a 64-byte-hex BIP39 seed, plus a v2 address.
    #[test]
    fn bridge_key_material_has_expected_shape() {
        let mnemonic = generate_deterministic_mnemonic(0).unwrap();
        let keys = derive_bridge_key_material(&mnemonic).unwrap();

        // 32-byte classical scalars, 64-byte PQ seed (hex-encoded).
        assert_eq!(hex::decode(&keys.view_hex).unwrap().len(), 32);
        assert_eq!(hex::decode(&keys.spend_hex).unwrap().len(), 32);
        assert_eq!(hex::decode(&keys.pq_seed_hex).unwrap().len(), 64);

        // Published address is a testnet v2 (post-quantum) URI.
        assert!(
            keys.address.starts_with("tbotho://2/"),
            "expected a v2 testnet address, got {}",
            keys.address
        );
    }

    /// Derivation is deterministic for the reserve (same node mnemonic → same
    /// keys), which is what lets the reserve keys detect the running node's
    /// pre-funded outputs.
    #[test]
    fn reserve_derivation_is_deterministic() {
        let mnemonic = generate_deterministic_mnemonic(3).unwrap();
        let a = derive_bridge_key_material(&mnemonic).unwrap();
        let b = derive_bridge_key_material(&mnemonic).unwrap();
        assert_eq!(a.view_hex, b.view_hex);
        assert_eq!(a.spend_hex, b.spend_hex);
        assert_eq!(a.pq_seed_hex, b.pq_seed_hex);
        assert_eq!(a.address, b.address);
    }

    /// The exported classical view/spend scalars are exactly the node wallet's
    /// own keys — the invariant that makes "reserve == the pre-funded miner"
    /// hold, so the reserve detects (and, after `fund-reserve` settles one into
    /// factor-1, can spend) the node's own coinbase outputs.
    #[test]
    fn reserve_keys_match_node_wallet() {
        let mnemonic = generate_deterministic_mnemonic(1).unwrap();
        let keys = derive_bridge_key_material(&mnemonic).unwrap();
        let wallet = botho::wallet::Wallet::from_mnemonic(&mnemonic).unwrap();
        assert_eq!(
            keys.view_hex,
            hex::encode(wallet.account_key().view_private_key().to_bytes())
        );
        assert_eq!(
            keys.spend_hex,
            hex::encode(wallet.account_key().spend_private_key().to_bytes())
        );
    }

    /// Distinct nodes yield distinct reserve keys (no accidental key reuse).
    #[test]
    fn distinct_nodes_yield_distinct_keys() {
        let a = derive_bridge_key_material(&generate_deterministic_mnemonic(0).unwrap()).unwrap();
        let b = derive_bridge_key_material(&generate_deterministic_mnemonic(1).unwrap()).unwrap();
        assert_ne!(a.spend_hex, b.spend_hex);
        assert_ne!(a.address, b.address);
    }
}
