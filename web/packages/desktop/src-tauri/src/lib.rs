//! Botho Desktop Wallet - Tauri Backend
//!
//! Provides Tauri commands for wallet operations including:
//! - Transaction building and signing
//! - Fee estimation
//! - Wallet synchronization

mod wallet;

use wallet::WalletCommands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .manage(WalletCommands::new())
        .invoke_handler(tauri::generate_handler![
            wallet::send_transaction,
            wallet::sync_wallet,
            wallet::get_balance,
            wallet::load_wallet_file,
            wallet::save_wallet_file,
            wallet::wallet_file_exists,
            wallet::get_wallet_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
