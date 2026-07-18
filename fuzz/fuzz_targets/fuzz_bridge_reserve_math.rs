#![no_main]

//! libFuzzer entry point for the proof-of-reserves peg-drift math. The harness
//! body (input types + all semantic assertions) lives in
//! `botho_fuzz::bridge_reserve_math` so this coverage-guided target and the
//! macOS native-smoke driver share one source of truth (#920, #1078). See that
//! module for the full security rationale and invariant list.

use libfuzzer_sys::fuzz_target;

use botho_fuzz::bridge_reserve_math::{run, Input};

// `fuzz_target!(|input: T|)` decodes `T` via `Arbitrary::arbitrary_take_rest`,
// the exact decoding the native-smoke driver uses, so a corpus seed drives the
// same code path in both worlds.
fuzz_target!(|input: Input| {
    run(input);
});
