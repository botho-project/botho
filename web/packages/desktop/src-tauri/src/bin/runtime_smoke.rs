//! Explicitly selected, disposable fixture application. Never called by
//! app_lib::run.
#[path = "../wallet.rs"]
mod wallet;

use botho_wallet::{keys::WalletKeys, storage::EncryptedWallet};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{State, WebviewUrl, WebviewWindowBuilder};

// Public BIP-39 test vector. Never import a user's mnemonic or wallet.
const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
const PASSWORD: &str = "public-runtime-smoke-fixture-only";
struct Fixture {
    path: PathBuf,
    recipient: String,
}
#[derive(Serialize)]
struct FixtureInfo {
    path: String,
    recipient: String,
}
#[tauri::command]
fn fixture_info(fixture: State<'_, Fixture>) -> FixtureInfo {
    FixtureInfo {
        path: fixture.path.to_string_lossy().into_owned(),
        recipient: fixture.recipient.clone(),
    }
}
#[tauri::command]
async fn fixture_unlock(
    fixture: State<'_, Fixture>,
    state: State<'_, wallet::WalletCommands>,
    path: String,
) -> Result<wallet::UnlockWalletResult, String> {
    // No default path, aliases, symlinks or caller-supplied credentials.
    if PathBuf::from(&path) != fixture.path
        || std::fs::canonicalize(&path).map_err(|e| e.to_string())? != fixture.path
    {
        return Err("Only the owned fixture path is permitted".into());
    }
    wallet::unlock_wallet(
        state,
        wallet::UnlockWalletParams {
            network: wallet::WalletNetwork::Testnet,
            password: PASSWORD.into(),
            path: Some(path),
        },
    )
    .await
}
#[tauri::command]
async fn fixture_preflight(
    state: State<'_, wallet::WalletCommands>,
    recipient: String,
    amount: String,
) -> Result<String, String> {
    let status = wallet::get_session_status(state, wallet::WalletNetwork::Testnet).await?;
    if !status.is_unlocked {
        return Err("Fixture wallet must be unlocked".into());
    }
    let (_, _, amount) = wallet::prepare_send(&recipient, &amount).map_err(|e| e.to_string())?;
    Ok(amount.to_string())
}
fn main() -> anyhow::Result<()> {
    // Mandatory explicit opt-in prevents accidental execution by all-features
    // tooling.
    anyhow::ensure!(
        std::env::var("BOTHO_RUNTIME_SMOKE").as_deref() == Ok("owned-fixture-only"),
        "explicit smoke opt-in required"
    );
    let port: u16 = std::env::var("BOTHO_SMOKE_PORT")?.parse()?;
    anyhow::ensure!(port >= 1024, "use an unprivileged explicit loopback port");
    let directory = tempfile::Builder::new()
        .prefix("botho-native-smoke-")
        .tempdir()?;
    eprintln!("BOTHO_SMOKE_OWNED_DIRECTORY={}", directory.path().display());
    let path = directory.path().join("fixture.wallet");
    EncryptedWallet::encrypt(MNEMONIC, PASSWORD)?.save(&path)?;
    let path = std::fs::canonicalize(path)?;
    let recipient = WalletKeys::from_mnemonic(MNEMONIC)?
        .public_address_string(bth_address_codec::Network::Testnet)?;
    let app = tauri::Builder::default()
        // 1.4.0 server/mod.rs binds 127.0.0.1 on desktop, with no default feature set.
        .plugin(tauri_plugin_wdio_webdriver::init_with_port(port))
        .manage(Fixture { path, recipient })
        .manage(wallet::WalletCommands::new())
        .invoke_handler(tauri::generate_handler![
            fixture_info,
            fixture_unlock,
            fixture_preflight,
            wallet::get_session_status,
            wallet::lock_wallet
        ])
        .setup(|app| {
            WebviewWindowBuilder::new(app, "runtime-smoke", WebviewUrl::App("index.html".into()))
                .title("Botho isolated runtime smoke — public fixture")
                .inner_size(1200.0, 850.0)
                .incognito(true)
                .on_navigation(|url| url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!("tauri.smoke.conf.json"))?;
    // run_return permits TempDir destruction; the runner also checks cleanup on
    // timeout.
    let code = app.run_return(|_, _| {});
    directory.close()?;
    anyhow::ensure!(code == 0, "smoke app exited with {code}");
    Ok(())
}
