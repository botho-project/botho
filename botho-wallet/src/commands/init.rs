//! Wallet initialization command

use anyhow::{anyhow, Result};
use std::io::{self, Write};
use std::path::Path;

use crate::keys::{validate_mnemonic, WalletKeys};
use crate::storage::EncryptedWallet;

use super::{print_error, print_success, print_warning, prompt_confirm, prompt_password};

/// Run the init command
pub async fn run(wallet_path: &Path, recover: bool) -> Result<()> {
    // Check if wallet already exists
    if EncryptedWallet::exists(wallet_path) {
        print_error("Wallet already exists at this location");
        println!("Path: {}", wallet_path.display());

        if !prompt_confirm("Overwrite existing wallet?")? {
            println!("Aborted.");
            return Ok(());
        }

        print_warning("Existing wallet will be overwritten!");
    }

    // Get or generate mnemonic
    let mnemonic = if recover {
        prompt_mnemonic()?
    } else {
        generate_mnemonic()?
    };

    // Validate the mnemonic
    validate_mnemonic(&mnemonic)?;

    // Get password
    println!();
    let password = prompt_new_password()?;

    // Create wallet keys
    let keys = WalletKeys::from_mnemonic(&mnemonic)?;

    // Encrypt and save
    let wallet = EncryptedWallet::encrypt(&mnemonic, &password)?;
    wallet.save(wallet_path)?;

    // Show success
    println!();
    print_success("Wallet created successfully!");
    println!();
    println!("Your receiving address:");
    println!("  {}", keys.address_string());
    println!();
    println!("Wallet saved to: {}", wallet_path.display());

    if !recover {
        println!();
        print_warning("IMPORTANT: Write down your recovery phrase and store it safely!");
        print_warning("Anyone with this phrase can access your funds.");
        print_warning("If you lose it, you cannot recover your wallet.");
    }

    Ok(())
}

/// Generate a new mnemonic and display it
fn generate_mnemonic() -> Result<String> {
    let keys = WalletKeys::generate()?;
    let words = keys.mnemonic_words();

    println!();
    println!("Your recovery phrase (24 words):");
    println!();

    // Display in 4 columns
    for (i, word) in words.iter().enumerate() {
        print!("{:>2}. {:<12}", i + 1, word);
        if (i + 1) % 4 == 0 {
            println!();
        }
    }
    println!();

    // Verify the user has written it down
    println!();
    if !prompt_confirm("Have you written down your recovery phrase?")? {
        return Err(anyhow!("Please write down your recovery phrase before continuing"));
    }

    // Verify by asking for a random word
    let verify_index = rand::random::<usize>() % 24;
    println!();
    print!("Verify: Enter word #{}: ", verify_index + 1);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    if input.trim() != words[verify_index] {
        return Err(anyhow!("Verification failed. Please try again."));
    }

    Ok(keys.mnemonic_phrase().to_string())
}

/// Prompt user to enter their recovery phrase
fn prompt_mnemonic() -> Result<String> {
    println!();
    println!("Enter your 24-word recovery phrase:");
    println!("(You can enter all words on one line, separated by spaces)");
    println!();

    let mut words = Vec::new();

    // Read input
    print!("> ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    // Parse words
    for word in input.trim().split_whitespace() {
        words.push(word.to_lowercase());
    }

    // If not enough words, prompt for more
    while words.len() < 24 {
        print!("Enter word #{}: ", words.len() + 1);
        io::stdout().flush()?;

        input.clear();
        io::stdin().read_line(&mut input)?;

        let word = input.trim().to_lowercase();
        if !word.is_empty() {
            words.push(word);
        }
    }

    if words.len() != 24 {
        return Err(anyhow!("Expected 24 words, got {}", words.len()));
    }

    Ok(words.join(" "))
}

/// Prompt for a new password with confirmation
fn prompt_new_password() -> Result<String> {
    loop {
        let password = prompt_password("Enter wallet password: ")?;

        if password.len() < 8 {
            print_error("Password must be at least 8 characters");
            continue;
        }

        let confirm = prompt_password("Confirm password: ")?;

        if password != confirm {
            print_error("Passwords do not match");
            continue;
        }

        return Ok(password);
    }
}
