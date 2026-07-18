#![no_main]

//! libFuzzer entry point for the Ethereum wBTH burn-event decode path. The
//! harness body (mode enum + all semantic assertions) lives in
//! `botho_fuzz::bridge_ethereum_parse` so this coverage-guided target and the
//! macOS native-smoke driver share one source of truth (#920, #1076). See that
//! module for the full security rationale and invariant list.

use libfuzzer_sys::fuzz_target;

use botho_fuzz::bridge_ethereum_parse::{run, Mode};

// `fuzz_target!(|input: T|)` decodes `T` via `Arbitrary::arbitrary_take_rest`,
// the exact decoding the native-smoke driver uses, so a corpus seed drives the
// same code path in both worlds.
fuzz_target!(|mode: Mode| {
    run(mode);
});
