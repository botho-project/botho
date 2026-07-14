#![no_main]

//! Fuzzing target for the federation peer-push HTTP status-line parser
//! (#897, Tier B of #892).
//!
//! Security rationale: [`parse_status_line`] is the only parsing a bridge
//! node performs on the raw bytes a *peer* (or anything squatting on a
//! peer's address) sends back after an attestation-envelope push (#858).
//! The transport deliberately caps the read at [`MAX_PEER_RESPONSE_BYTES`]
//! (8 KiB) before parsing, so a slow/malicious peer cannot grow unbounded
//! per-task heap during the push timeout window. A panic here is a
//! remote-triggerable DoS of the outbound federation path; a wrong parse
//! only mislabels push logging/retries (the envelope itself is
//! self-authenticating), so the properties under fuzz are panic-freedom and
//! cap-stability.
//!
//! Semantic assertions (not just no-panic):
//!
//!   * arbitrary bytes — including empty, all-whitespace, and non-UTF-8 —
//!     never panic; inputs with fewer than two whitespace-separated tokens
//!     parse to `0` (the "no parseable status" sentinel);
//!   * round-trip: a well-formed `HTTP/1.1 <code> ...` line parses back to
//!     exactly `<code>` for every `u16`, with arbitrary header/body bytes
//!     after the CRLF;
//!   * cap behavior (`MAX_PEER_RESPONSE_BYTES`): for a response flooded to
//!     more than 3x the 8 KiB cap, parsing the transport-capped prefix
//!     yields the same status as parsing the whole flood — truncation at
//!     the cap is harmless because the status line always comes first (the
//!     exact invariant `read_status_line` relies on); the > 8 KiB parse
//!     itself is panic-free and allocation is proportional to the input
//!     actually given.
//!
//! Issue refs: #897 (this target + the `parse_status_line` extraction from
//! the async `read_status_line`), #892 (fuzz plan), #858 (federation
//! transport), #885 (bounded peer-response read).

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use bth_bridge_service::{parse_status_line, MAX_PEER_RESPONSE_BYTES};

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// The full fuzz input: raw adversarial bytes plus the ingredients for the
/// well-formed round-trip and flood cases.
#[derive(Debug, Arbitrary)]
struct FuzzInput {
    /// Arbitrary raw response bytes (empty / garbage / non-UTF-8 / partial
    /// status lines).
    raw: Vec<u8>,
    /// Status code for the well-formed round-trip case.
    code: u16,
    /// Bytes appended after the status line's CRLF (headers/body).
    tail: Vec<u8>,
    /// Filler byte for the > 8 KiB flood case.
    flood_byte: u8,
}

// ============================================================================
// Fuzz Target
// ============================================================================

fuzz_target!(|input: FuzzInput| {
    exercise_raw(&input.raw);
    exercise_round_trip_and_flood(input.code, &input.tail, input.flood_byte);
});

/// Arbitrary bytes must never panic, and the "no parseable status" sentinel
/// contract must hold: fewer than two whitespace tokens always yields 0.
fn exercise_raw(raw: &[u8]) {
    let status = parse_status_line(raw);

    // Reference token count on the lossy-decoded text (the same decoding the
    // parser is specified to use). With < 2 tokens there is no status field
    // at all, so the sentinel is the only acceptable answer.
    let tokens = String::from_utf8_lossy(raw).split_whitespace().count();
    if tokens < 2 {
        assert_eq!(
            status, 0,
            "a response without a second whitespace token must yield the 0 sentinel"
        );
    }
}

/// A canonical `HTTP/1.1 <code> ...` response must round-trip for every u16,
/// and truncating a flooded response at the transport cap must not change
/// the parsed status.
fn exercise_round_trip_and_flood(code: u16, tail: &[u8], flood_byte: u8) {
    // --- Round-trip on a well-formed short response ---
    let mut response = format!("HTTP/1.1 {code} Reason\r\n").into_bytes();
    response.extend_from_slice(tail);
    // The status line is first; `tail` only ever contributes tokens AFTER the
    // second one, so it must never perturb the parse.
    assert_eq!(
        parse_status_line(&response),
        code,
        "well-formed status line failed to round-trip its code"
    );

    // --- Cap behavior: flood past MAX_PEER_RESPONSE_BYTES (> 8 KiB) ---
    let cap = MAX_PEER_RESPONSE_BYTES as usize;
    let mut flooded = format!("HTTP/1.1 {code} Reason\r\n").into_bytes();
    flooded.resize(cap * 3 + 1, flood_byte);

    // Parsing the full > 8 KiB flood is panic-free and still finds the code
    // (the flood is a single trailing token at most).
    let full = parse_status_line(&flooded);
    // Parsing exactly what the transport layer would hand over (the capped
    // prefix, per `read_status_line`'s `take(MAX_PEER_RESPONSE_BYTES)`)
    // yields the same status: truncation at the cap is harmless because the
    // status line always comes first.
    let capped = parse_status_line(&flooded[..cap]);
    assert_eq!(
        full, code,
        "flooded response lost its status code on a full parse"
    );
    assert_eq!(
        capped, code,
        "transport-capped prefix parsed a different status than the full response"
    );
}
