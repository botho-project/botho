#[path = "../tests/support/lottery_vectors.rs"]
mod vectors;
fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&vectors::vectors()).unwrap()
    );
}
