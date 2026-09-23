//! One structured request on stdin; one response on stdout. Never broadcasts.
#[cfg(all(unix, feature = "pq"))]
fn main() {
    use botho_wallet::stress::{execute, Request, MAX_REQUEST};
    use std::io::{self, Read};
    let result = (|| -> anyhow::Result<serde_json::Value> {
        let mut input = Vec::new();
        io::stdin().take(MAX_REQUEST + 1).read_to_end(&mut input)?;
        anyhow::ensure!(
            input.len() as u64 <= MAX_REQUEST,
            "request exceeds byte limit"
        );
        let request: Request = serde_json::from_slice(&input)?;
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
