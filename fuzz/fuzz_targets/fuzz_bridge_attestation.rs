#![no_main]

//! libFuzzer entry point for the bridge federation `AttestationSet` threshold
//! logic. The harness body (input type + all invariant assertions) lives in
//! `botho_fuzz::bridge_attestation` so this coverage-guided target and the
//! macOS native-smoke driver share one source of truth (#920). See that
//! module for the full security rationale and invariant list.

use libfuzzer_sys::fuzz_target;

use botho_fuzz::bridge_attestation::{run, FuzzInput};

// `fuzz_target!(|input: T|)` decodes `T` via `Arbitrary::arbitrary_take_rest`,
// the exact decoding the native-smoke driver uses, so a corpus seed drives the
// same code path in both worlds.
fuzz_target!(|input: FuzzInput| {
    run(&input);
});
