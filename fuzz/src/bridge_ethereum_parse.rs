// Copyright (c) 2024 The Botho Foundation

//! Shared harness body for the Ethereum wBTH burn-event decode path
//! (fuzz target `fuzz_bridge_ethereum_parse`; #1076, Phase 3 of bridge epic
//! #816).
//!
//! Security rationale: the burn flow decodes **attacker-influenced, on-chain**
//! data into a native-BTH release recipient. Whoever emits a `BridgeBurn`
//! event fully controls both the `bthAddress` string and the `uint256` amount,
//! and that decode is the last pure boundary before custody-relevant state:
//!
//!   * [`decode_burn_log`](bth_bridge_service::fuzz_decode_burn_log_parts)
//!     narrows the on-chain `uint256` amount to a `u64` picocredit quantity
//!     (`try_into`). A wrap here would fabricate a release amount out of thin
//!     air; the contract cannot be trusted to bound it, so the decoder must
//!     *reject* anything above `u64::MAX` rather than truncate it.
//!   * The `bthAddress` string rides through the decoded `BurnEvent` unchanged
//!     and later becomes the release recipient via
//!     [`decode_recipient_address`](bth_bridge_service::decode_recipient_address)
//!     (`bridge/service/src/bth_scan.rs`). A panic on adversarial input is a
//!     remote DoS of the bridge; an empty/garbage address that decoded to a
//!     "valid" recipient would misdirect a release.
//!
//! One target covers the whole Ethereum burn-decode boundary via an
//! `Arbitrary` mode enum (the same grouping sanctioned for the Solana
//! mint-path parsers in #897). Each arm asserts *semantic* postconditions
//! against a reference model, not just panic-freedom:
//!
//!   * `BurnLog`: a well-formed `BridgeBurn` log assembled from fuzzer-chosen
//!     parts decodes to a `BurnEvent` **iff** the `uint256` amount fits in a
//!     `u64` (top 24 big-endian bytes all zero); when it does, the decoded
//!     amount is the exact low-64-bit narrowing and every attacker-controlled
//!     field round-trips without corruption. An oversized amount yields `None`
//!     (never a wrapped quantity).
//!   * `RawLogData`: arbitrary bytes as the non-indexed ABI data region never
//!     panic the alloy decoder; whatever decodes still feeds the downstream
//!     recipient decode without panic.
//!   * `Recipient`: `decode_recipient_address` never panics on an arbitrary
//!     string, and an **empty** address never decodes to a valid recipient.
//!
//! The harness body lives here (not in the `fuzz_targets/` binary) so the
//! coverage-guided libFuzzer target and the macOS native-smoke driver run the
//! exact same assertions over one source of truth (#920).

use arbitrary::Arbitrary;

use bth_bridge_service::{
    decode_recipient_address, fuzz_decode_burn_log_parts, fuzz_decode_burn_log_raw,
};

// ============================================================================
// Structured Fuzzing Types
// ============================================================================

/// One Ethereum burn-decode exercise per fuzz input.
#[derive(Debug, Arbitrary)]
pub enum Mode {
    /// A well-formed `BridgeBurn` log assembled from attacker-controlled
    /// parts: the `uint256` amount (`amount_be`, raw big-endian) and the
    /// `bthAddress` string are the adversarial fields; the rest is log
    /// metadata that must round-trip.
    BurnLog {
        from: [u8; 20],
        amount_be: [u8; 32],
        bth_address: String,
        tx_hash: [u8; 32],
        block_hash: [u8; 32],
        block_number: u64,
        log_index: u64,
    },
    /// Arbitrary bytes as the non-indexed ABI log `data` region — decoder
    /// robustness against a malformed dynamic `string`/`uint256` encoding.
    RawLogData { data: Vec<u8> },
    /// A recipient-address string straight into `decode_recipient_address`.
    Recipient { address: String },
}

// ============================================================================
// Entry point (shared by the libFuzzer target and the native-smoke driver)
// ============================================================================

/// Decode a [`Mode`] from raw bytes the same way libFuzzer would and run the
/// harness. Bytes that cannot form a `Mode` are skipped.
pub fn run_from_bytes(data: &[u8]) {
    if let Some(mode) = crate::decode_take_rest::<Mode>(data) {
        run(mode);
    }
}

/// Run the harness against an already-decoded input.
pub fn run(mode: Mode) {
    match mode {
        Mode::BurnLog {
            from,
            amount_be,
            bth_address,
            tx_hash,
            block_hash,
            block_number,
            log_index,
        } => exercise_burn_log(
            from,
            amount_be,
            &bth_address,
            tx_hash,
            block_hash,
            block_number,
            log_index,
        ),
        Mode::RawLogData { data } => exercise_raw_log_data(&data),
        Mode::Recipient { address } => exercise_recipient(&address),
    }
}

/// A well-formed `BridgeBurn` log decodes to a `BurnEvent` exactly when its
/// `uint256` amount fits in a `u64`; the narrowing is the low-64-bit window
/// (never a wrap of a larger value), and the attacker-controlled fields
/// round-trip byte-for-byte.
fn exercise_burn_log(
    from: [u8; 20],
    amount_be: [u8; 32],
    bth_address: &str,
    tx_hash: [u8; 32],
    block_hash: [u8; 32],
    block_number: u64,
    log_index: u64,
) {
    // Reference model for the `uint256 -> u64` narrowing, computed WITHOUT
    // alloy: the value fits in a u64 iff every big-endian byte above the low 8
    // is zero. u64::MAX itself fits (top 24 bytes zero), so the boundary is
    // exact.
    let fits = amount_be[..24].iter().all(|&b| b == 0);
    let expected_amount = u64::from_be_bytes(amount_be[24..].try_into().unwrap());

    let decoded = fuzz_decode_burn_log_parts(
        from,
        amount_be,
        bth_address,
        tx_hash,
        block_hash,
        block_number,
        log_index,
    );

    match decoded {
        Some(event) => {
            assert!(
                fits,
                "a uint256 amount above u64::MAX picocredits decoded to {} instead of being \
                 rejected — the narrowing must never wrap",
                event.amount
            );
            assert_eq!(
                event.amount, expected_amount,
                "decoded amount is not the exact low-64-bit narrowing of the uint256"
            );
            assert_eq!(
                event.bth_address, bth_address,
                "attacker-controlled bthAddress was corrupted in the decode round-trip"
            );
            assert_eq!(
                event.block_number, block_number,
                "block_number corrupted in the decode round-trip"
            );
            assert_eq!(
                event.log_index, log_index,
                "log_index corrupted in the decode round-trip"
            );
            // 0x-prefixed hex of a 20-byte address / 32-byte hashes.
            assert!(
                event.from.starts_with("0x") && event.from.len() == 42,
                "decoded `from` is not a 0x-prefixed 20-byte address: {}",
                event.from
            );
            assert!(
                event.tx_hash.starts_with("0x") && event.block_hash.starts_with("0x"),
                "decoded tx/block hashes are not 0x-prefixed"
            );
            // The recipient the release path will trust: decoding it must never
            // panic, whatever the attacker put in `bthAddress`.
            let _ = decode_recipient_address(&event.bth_address);
        }
        None => assert!(
            !fits,
            "a uint256 amount that fits in u64 picocredits must decode to a BurnEvent"
        ),
    }
}

/// Arbitrary ABI data bytes never panic the alloy decoder; anything that does
/// decode still feeds the recipient decode without panic.
fn exercise_raw_log_data(data: &[u8]) {
    if let Some(event) = fuzz_decode_burn_log_raw(data) {
        // A decode that succeeded already survived the u64 narrowing (else it
        // would be None); the recipient decode must not panic either.
        let _ = decode_recipient_address(&event.bth_address);
    }
}

/// `decode_recipient_address` never panics on an arbitrary string, and an
/// empty address never decodes to a valid recipient.
fn exercise_recipient(address: &str) {
    let result = decode_recipient_address(address);
    if address.is_empty() {
        assert!(
            result.is_err(),
            "an empty bthAddress must never decode to a valid release recipient"
        );
    }
    // Reaching here (no panic) is itself the primary invariant.
    let _ = result;
}
