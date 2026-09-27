//! One structured request on stdin; one response on stdout. Never broadcasts.
#[cfg(all(unix, feature = "pq"))]
fn main() {
    use botho_wallet::stress::{execute, read_request};
    use std::io;
    let result = (|| -> anyhow::Result<serde_json::Value> {
        let request = read_request(io::stdin().lock())?;
        execute(request)
    })();
    match result {
        Ok(value) => println!("{}", serde_json::json!({"ok":true,"result":value})),
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({"ok":false,"error":error.to_string()})
            );
            std::process::exit(1);
        }
    }
}
#[cfg(not(all(unix, feature = "pq")))]
fn main() {
    eprintln!("botho-stress-wallet requires Unix and the pq feature");
    std::process::exit(1);
}
