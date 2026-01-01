//! Node management command

use anyhow::Result;
use std::path::Path;

use crate::discovery::NodeDiscovery;
use crate::rpc_pool::RpcPool;
use crate::storage::EncryptedWallet;

use super::{decrypt_wallet_with_rate_limiting, print_error, print_success};

/// Run the nodes command
pub async fn run(wallet_path: &Path, discover: bool) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet with rate limiting protection
    let (mut wallet, _mnemonic, password) = decrypt_wallet_with_rate_limiting(wallet_path)?;

    // Get or create discovery state
    let mut discovery = wallet
        .get_discovery_state(&password)?
        .unwrap_or_else(NodeDiscovery::new);

    if discover {
        println!();
        println!("Discovering nodes...");

        // Discover new nodes
        let nodes = discovery.discover().await;

        if nodes.is_empty() {
            print_error("No nodes found");
        } else {
            print_success(&format!("Found {} nodes", nodes.len()));
        }

        // Try to connect and get more peers
        let mut rpc = RpcPool::new(discovery);
        if rpc.connect().await.is_ok() {
            // Request peers from connected nodes
            if let Ok(peers) = rpc.get_peers().await {
                println!("Discovered {} additional peers via gossip", peers.len());
                rpc.discovery_mut().add_peers(&peers);
            }
        }

        // Save updated discovery state
        wallet.set_discovery_state(rpc.discovery(), &password)?;
        wallet.save(wallet_path)?;

        discovery = rpc.discovery().clone();
    }

    // Display known nodes
    let peers = discovery.known_peers();

    println!();
    if peers.is_empty() {
        println!("No known nodes. Run with --discover to find nodes.");
    } else {
        println!("Known nodes ({}):", peers.len());
        println!();

        for peer in peers {
            let health = discovery.get_health(peer);
            let status = match health {
                Some(h) if h.failures >= 3 => "\x1b[31munreachable\x1b[0m",
                Some(h) if h.failures > 0 => "\x1b[33mflaky\x1b[0m",
                Some(_) => "\x1b[32mhealthy\x1b[0m",
                None => "unknown",
            };

            let info = health
                .map(|h| {
                    format!(
                        "latency: {}ms, height: {}, failures: {}",
                        h.latency_ms, h.block_height, h.failures
                    )
                })
                .unwrap_or_default();

            println!("  {} [{}]", peer, status);
            if !info.is_empty() {
                println!("    {}", info);
            }
        }
    }

    Ok(())
}

impl Clone for NodeDiscovery {
    fn clone(&self) -> Self {
        // Serialize and deserialize to clone
        let bytes = self.to_bytes().unwrap_or_default();
        Self::from_bytes(&bytes).unwrap_or_else(|_| Self::new())
    }
}
