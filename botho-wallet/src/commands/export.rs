//! Wallet export/backup command

use anyhow::Result;
use std::fs;
use std::path::Path;

use crate::keys::WalletKeys;
use crate::storage::EncryptedWallet;

use super::{decrypt_wallet_with_rate_limiting, print_error, print_success, print_warning, prompt_confirm};

/// Run the export command
pub async fn run(wallet_path: &Path, output: Option<String>) -> Result<()> {
    // Check wallet exists
    if !EncryptedWallet::exists(wallet_path) {
        print_error("No wallet found. Run 'botho-wallet init' first.");
        return Ok(());
    }

    // Load and decrypt wallet with rate limiting protection
    let (_wallet, mnemonic, _password) = decrypt_wallet_with_rate_limiting(wallet_path)?;

    let keys = WalletKeys::from_mnemonic(&mnemonic)?;
    let words = keys.mnemonic_words();

    // Determine output mode
    if let Some(output_path) = output {
        // Export to file
        let output_path = Path::new(&output_path);

        if output_path.exists() {
            if !prompt_confirm("Output file exists. Overwrite?")? {
                println!("Aborted.");
                return Ok(());
            }
        }

        // Create backup content
        let backup = format!(
            "# Botho Wallet Backup\n\
             # Created: {}\n\
             # Address: {}\n\
             #\n\
             # KEEP THIS FILE SAFE AND SECRET!\n\
             # Anyone with this phrase can access your funds.\n\
             #\n\
             \n\
             {}\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
            keys.address_string(),
            mnemonic.as_str()
        );

        // Write with restricted permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(output_path)?;
            use std::io::Write;
            file.write_all(backup.as_bytes())?;
        }

        #[cfg(not(unix))]
        {
            fs::write(output_path, backup)?;
        }

        println!();
        print_success(&format!("Backup saved to: {}", output_path.display()));
        print_warning("Keep this file safe and secret!");

    } else {
        // Display on screen
        println!();
        print_warning("IMPORTANT: Keep your recovery phrase secret!");
        print_warning("Anyone with these words can access your funds.");
        println!();

        if !prompt_confirm("Show recovery phrase on screen?")? {
            println!("Aborted.");
            return Ok(());
        }

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
        println!();
        println!("Address: {}", keys.address_string());
    }

    Ok(())
}
