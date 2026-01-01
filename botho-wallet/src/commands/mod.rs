//! CLI Commands
//!
//! Implementation of all wallet CLI commands.

pub mod address;
pub mod balance;
pub mod export;
pub mod history;
pub mod init;
pub mod migrate_to_pq;
pub mod nodes;
pub mod send;
pub mod sync;

use anyhow::Result;
use std::io::{self, Write};
use std::path::Path;
use zeroize::Zeroizing;

use crate::storage::{DecryptionRateLimiter, EncryptedWallet};

/// Prompt for password input (hidden)
pub fn prompt_password(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;

    let password = rpassword::read_password()?;
    Ok(password)
}

/// Prompt for confirmation
pub fn prompt_confirm(message: &str) -> Result<bool> {
    print!("{} [y/N]: ", message);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes"))
}

/// Print an error message
pub fn print_error(message: &str) {
    eprintln!("\x1b[31mError:\x1b[0m {}", message);
}

/// Print a success message
pub fn print_success(message: &str) {
    println!("\x1b[32m{}\x1b[0m", message);
}

/// Print a warning message
pub fn print_warning(message: &str) {
    println!("\x1b[33mWarning:\x1b[0m {}", message);
}

/// Load and decrypt a wallet with rate limiting protection.
///
/// This function handles the complete workflow for secure wallet decryption:
/// 1. Loads or creates rate limiter state
/// 2. Checks if currently rate limited (and displays error if so)
/// 3. Prompts for password
/// 4. Attempts decryption with rate limiting
/// 5. Saves rate limiter state (success resets, failure increments)
///
/// # Arguments
/// * `wallet_path` - Path to the encrypted wallet file
///
/// # Returns
/// * `Ok((EncryptedWallet, Zeroizing<String>, String))` - The wallet, decrypted mnemonic, and password
/// * `Err` - If rate limited, wallet not found, or decryption failed
///
/// # Security
/// The returned mnemonic is wrapped in `Zeroizing<String>` which automatically
/// overwrites memory when dropped, preventing sensitive data from persisting.
pub fn decrypt_wallet_with_rate_limiting(
    wallet_path: &Path,
) -> Result<(EncryptedWallet, Zeroizing<String>, String)> {
    // Load rate limiter state
    let mut rate_limiter = DecryptionRateLimiter::load_for_wallet(wallet_path);

    // Check if we're currently rate limited before prompting for password
    if let Err(e) = rate_limiter.check_rate_limit() {
        print_error(&e.to_string());
        return Err(e);
    }

    // Check for lockout
    if rate_limiter.is_locked_out() {
        if let Some(remaining) = rate_limiter.remaining_lockout_time() {
            let msg = format!(
                "Account temporarily locked due to too many failed attempts. Try again in {}",
                remaining
            );
            print_error(&msg);
            return Err(anyhow::anyhow!("{}", msg));
        }
    }

    // Load wallet
    let wallet = EncryptedWallet::load(wallet_path)?;

    // Prompt for password
    let password = prompt_password("Enter wallet password: ")?;

    // Attempt decryption with rate limiting
    match wallet.decrypt_with_rate_limit(&password, &mut rate_limiter) {
        Ok(mnemonic) => {
            // Save updated rate limiter state (success resets failures)
            if let Err(e) = rate_limiter.save_for_wallet(wallet_path) {
                // Log but don't fail - decryption succeeded
                eprintln!("Warning: Failed to save rate limiter state: {}", e);
            }
            Ok((wallet, mnemonic, password))
        }
        Err(e) => {
            // Save updated rate limiter state (failure increments counter)
            if let Err(save_err) = rate_limiter.save_for_wallet(wallet_path) {
                eprintln!("Warning: Failed to save rate limiter state: {}", save_err);
            }
            print_error(&e.to_string());
            Err(e)
        }
    }
}
