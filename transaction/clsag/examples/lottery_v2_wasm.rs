//! Test-only WASM ABI. Never linked into the production wallet.
#[path = "../tests/support/lottery_vectors.rs"]
mod vectors;
#[no_mangle]
pub extern "C" fn verify_lottery_vectors() -> u32 {
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/lottery-v2.json")).unwrap();
    u32::from(vectors::vectors() != expected)
}
