use anyhow::{bail, Context, Result};
use bip39::{Language, Mnemonic, MnemonicType};
use std::io::{self, BufRead, Write};
use std::path::Path;
use tracing::info;

use crate::config::Config;

/// Run the init command
pub fn run(config_path: &Path, recover: bool) -> Result<()> {
    // Check if config already exists
    if Config::exists(config_path) {
        bail!(
            "Wallet already exists at {}\nUse a different --config path or delete the existing config.",
            config_path.display()
        );
    }

    let mnemonic = if recover {
        recover_mnemonic()?
    } else {
        generate_new_mnemonic()?
    };

    // Create and save config
    let config = Config::new(mnemonic.phrase().to_string());
    config.save(config_path)?;

    info!("Wallet initialized at {}", config_path.display());
    println!("\nYour wallet has been created.");
    println!("Config saved to: {}", config_path.display());
    println!("\nNext steps:");
    println!("  1. Add peers to your config file");
    println!("  2. Run 'cadence run' to start syncing");
    println!("  3. Run 'cadence run --mine' to start mining");

    Ok(())
}

/// Generate a new BIP39 mnemonic
fn generate_new_mnemonic() -> Result<Mnemonic> {
    let mnemonic = Mnemonic::new(MnemonicType::Words24, Language::English);

    println!("\n{}", "=".repeat(60));
    println!("IMPORTANT: Write down your recovery phrase!");
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
