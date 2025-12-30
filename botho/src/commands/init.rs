use anyhow::{bail, Context, Result};
use bip39::{Language, Mnemonic, MnemonicType};
use bth_transaction_types::constants::Network;
use std::io::{self, BufRead, Write};
use std::path::Path;
use tracing::info;

use crate::config::Config;

/// Run the init command
pub fn run(config_path: &Path, recover: bool, relay: bool, network: Network) -> Result<()> {
    // Check if config already exists
    if Config::exists(config_path) {
        bail!(
            "Config already exists at {}\nUse a different --config path or delete the existing config.",
            config_path.display()
        );
    }

    let network_name = network.display_name();

    if relay {
        // Create relay node config (no wallet)
        let config = Config::new_relay(network);
        config.save(config_path)?;

        info!("Relay node initialized at {}", config_path.display());
        println!("\n[{}] Relay node configuration created (no wallet).", network_name);
        println!("Config saved to: {}", config_path.display());
        println!("\nThis node will:");
        println!("  - Relay blocks and transactions on {}", network);
        println!("  - Help with peer discovery");
        println!("  - NOT mine or receive funds");
        println!("\nNext steps:");
        println!("  1. Run 'botho run' to start the relay node");
    } else {
        let mnemonic = if recover {
            recover_mnemonic()?
        } else {
            generate_new_mnemonic(network)?
        };

        // Create and save config
        let config = Config::new(mnemonic.phrase().to_string(), network);
        config.save(config_path)?;

        info!("Wallet initialized at {}", config_path.display());
        println!("\n[{}] Your wallet has been created.", network_name);
        println!("Config saved to: {}", config_path.display());
        println!("\nNext steps:");
        println!("  1. Run 'botho run' to start syncing");
        println!("  2. Run 'botho run --mint' to start minting");
        if !network.is_production() {
            println!("\nNote: This is a testnet wallet. Coins have no real value.");
        }
    }

    Ok(())
}

/// Generate a new BIP39 mnemonic
fn generate_new_mnemonic(network: Network) -> Result<Mnemonic> {
    let mnemonic = Mnemonic::new(MnemonicType::Words24, Language::English);

    println!("\n{}", "=".repeat(60));
    println!("[{}] IMPORTANT: Write down your recovery phrase!", network.display_name());
    println!("This is the ONLY way to recover your wallet.");
    println!("{}", "=".repeat(60));
    println!("\nYour 24-word recovery phrase:\n");

    // Display words in a grid
    let words: Vec<&str> = mnemonic.phrase().split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        print!("{:2}. {:<12}", i + 1, word);
        if (i + 1) % 4 == 0 {
            println!();
        }
    }

    println!("\n{}", "=".repeat(60));
    println!("Keep this phrase secret and safe!");
    println!("{}", "=".repeat(60));

    // Confirm user has saved the phrase
    println!("\nHave you written down your recovery phrase? (yes/no)");
    print!("> ");
    io::stdout().flush()?;

    let stdin = io::stdin();
    let response = stdin.lock().lines().next()
        .context("Failed to read input")?
        .context("Failed to read input")?;

    if response.trim().to_lowercase() != "yes" {
        bail!("Please write down your recovery phrase before continuing.");
    }

    Ok(mnemonic)
}

/// Recover wallet from existing mnemonic
fn recover_mnemonic() -> Result<Mnemonic> {
    println!("\nEnter your 24-word recovery phrase:");
    println!("(You can enter all words on one line, separated by spaces)");
    print!("> ");
    io::stdout().flush()?;

    let stdin = io::stdin();
    let phrase = stdin.lock().lines().next()
        .context("Failed to read input")?
        .context("Failed to read input")?;

    let mnemonic = Mnemonic::from_phrase(phrase.trim(), Language::English)
        .map_err(|e| anyhow::anyhow!("Invalid mnemonic: {}", e))?;

    println!("\nRecovery phrase validated successfully.");

    Ok(mnemonic)
}
