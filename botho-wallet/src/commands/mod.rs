//! CLI Commands
//!
//! Implementation of all wallet CLI commands.

pub mod address;
pub mod balance;
pub mod export;
pub mod history;
pub mod init;
pub mod nodes;
pub mod send;
pub mod sync;

use anyhow::Result;
use std::io::{self, Write};

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
