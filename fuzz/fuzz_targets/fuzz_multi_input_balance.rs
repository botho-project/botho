#![no_main]

//! Fuzz target for the multi-input balance equation in
//! `Transaction::verify_ring_signatures` (audit finding I4).
//!
//! The transaction-level balance check is consensus-critical: it must hold
//! that `sum(pseudo_output_amount) == sum(outputs) + fee` exactly, using
//! integer (checked) arithmetic, and must NEVER panic or integer-overflow —
//! including for adversarial amounts chosen to sit right on `u64::MAX`.
//!
//! ## Strategy
//!
//! We synthesize a `Transaction` directly from arbitrary, structured data
//! (`ClsagRingInput`/`TxOutput` have public fields, so we can set
//! `pseudo_output_amount` / output `amount` / `fee` freely — exactly the
//! attacker-controlled wire fields). We then drive the REAL
//! `verify_ring_signatures` and assert the invariant.
//!
//! Note on reachability: `verify_ring_signatures` verifies each input's CLSAG
//! signature *before* accumulating the balance, so for inputs carrying a
//! garbage signature it returns `Err("Invalid CLSAG signature")` early. The
//! zero-input case (a structurally-degenerate but wire-decodable transaction)
//! skips the signature loop entirely and reaches the balance equation
//! directly — this is the path that exercises `total_output()` + the
//! `checked_add(fee)` accumulation against overflow. We deliberately bias the
//! generator toward small input counts so the overflow-sensitive zero-input
//! path is hit frequently.
//!
//! ## Invariant asserted (issue #337, target 2)
//!
//! 1. `verify_ring_signatures` never panics and never integer-overflows the
//!    balance sum (any input → returns, does not crash).
//! 2. Whenever it returns `Ok(())`, the balance equation holds exactly:
//!    `sum(pseudo_output_amount) == sum(output.amount) + fee`, computed in u128
//!    (which cannot overflow for ≤2^64 terms summed here), with no truncation.
//!    An `Ok` that does not satisfy this is a consensus break.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use botho::transaction::{ClsagRingInput, Transaction, TxOutput};

#[derive(Debug, Arbitrary)]
struct FuzzInput {
    /// pseudo_output_amount for one synthesized input.
    pseudo_output_amount: u64,
    /// Raw CLSAG signature bytes (essentially always invalid — the point is
    /// to confirm no panic on the signature path, and to drive the zero-input
    /// balance path when `inputs` is empty).
    sig: Vec<u8>,
    key_image: [u8; 32],
    commitment_key_image: [u8; 32],
}

#[derive(Debug, Arbitrary)]
struct FuzzOutput {
    amount: u64,
    target_key: [u8; 32],
    public_key: [u8; 32],
}

#[derive(Debug, Arbitrary)]
struct FuzzTx {
    inputs: Vec<FuzzInput>,
    outputs: Vec<FuzzOutput>,
    fee: u64,
    created_at_height: u64,
}

fn build_input(f: &FuzzInput) -> ClsagRingInput {
    ClsagRingInput {
        ring: Vec::new(),
        key_image: f.key_image,
        commitment_key_image: f.commitment_key_image,
        clsag_signature: f.sig.clone(),
        pseudo_output_amount: f.pseudo_output_amount,
    }
}

fn build_output(f: &FuzzOutput) -> TxOutput {
    TxOutput {
        amount: f.amount,
        target_key: f.target_key,
        public_key: f.public_key,
        e_memo: None,
        cluster_tags: Default::default(),
    }
}

fuzz_target!(|tx_data: FuzzTx| {
    // Cap structure sizes so a single input vector cannot blow up memory; the
    // balance/overflow logic is fully exercised with a handful of terms.
    let inputs: Vec<ClsagRingInput> = tx_data.inputs.iter().take(16).map(build_input).collect();
    let outputs: Vec<TxOutput> = tx_data.outputs.iter().take(16).map(build_output).collect();

    let tx = Transaction::new_clsag(inputs, outputs, tx_data.fee, tx_data.created_at_height);

    // Reference balance computed in u128: the sums here are at most 16 terms,
    // each < 2^64, so the u128 accumulation cannot overflow and gives us the
    // ground truth the consensus path must match.
    let input_sum: u128 = tx
        .inputs
        .clsag()
        .iter()
        .map(|i| i.pseudo_output_amount as u128)
        .sum();
    let output_sum: u128 = tx.outputs.iter().map(|o| o.amount as u128).sum();
    let balanced = input_sum == output_sum + tx.fee as u128;

    // Drive the REAL consensus function. Must never panic / overflow-trap.
    let result = tx.verify_ring_signatures();

    // Invariant 2: an Ok result implies the exact balance equation. (The
    // converse is NOT asserted — verify can legitimately reject a balanced tx
    // on a bad signature; but it must NEVER accept an unbalanced one.)
    if result.is_ok() {
        assert!(
            balanced,
            "verify_ring_signatures returned Ok for an UNBALANCED transaction: \
             input_sum={} != output_sum={} + fee={}",
            input_sum, output_sum, tx.fee
        );
    }
});
