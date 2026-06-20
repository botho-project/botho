//! UniFFI bindings generator for the mobile bridge.
//!
//! App builds use this to generate the Swift/Kotlin bindings from the compiled
//! library, e.g.:
//!
//! ```sh
//! cargo build -p botho-mobile
//! cargo run -p botho-mobile --bin uniffi-bindgen -- \
//!   generate --library target/debug/libbotho_mobile.dylib \
//!   --language swift --out-dir ./bindings
//! ```
fn main() {
    uniffi::uniffi_bindgen_main()
}
