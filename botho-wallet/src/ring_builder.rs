//! CLSAG ring-member sourcing over RPC.
//!
//! The node builds ring signatures by pulling decoy outputs directly from its
//! ledger (`Ledger::get_decoy_outputs_for_input`). The thin wallet has no
//! ledger, so it reconstructs the same decoy pool via `chain_getOutputs` and
//! applies the identical age policy the node uses:
//!
//! - **Age-similarity band** (`±10%`, [`AGE_SIMILARITY_SPREAD_BPS`]): decoys
//!   are drawn from the height window whose ages fall within ±10% of the real
//!   input's age. This keeps CLI-wallet rings indistinguishable from
//!   node-wallet rings under a ring-age-spread adversary (issue #614 item 4).
//! - **Confirmation floor** ([`MIN_DECOY_AGE_BLOCKS`]): decoys (and the real
//!   input) must be at least 10 blocks deep. Inputs younger than this get a
//!   clean user-facing error instead of a degenerate band / panic (mirrors the
//!   #611 / #618 lesson).
//! - **Shuffle**: the eligible pool is shuffled before taking N, so ring
//!   membership is not a deterministic first-N slice of a height-sorted pool.

use anyhow::{anyhow, Result};
use bth_transaction_clsag::{RingMember, TxOutput};
use bth_transaction_types::ClusterTagVector;
use rand::{rngs::OsRng, seq::SliceRandom};

use crate::{
    decoy_selection::{age_similarity_band, MIN_DECOY_AGE_BLOCKS},
    rpc_pool::{RpcPool, TxOutput as RpcTxOutput},
};

/// Parse a 32-byte key from a hex string, returning `None` on malformed input.
fn parse_key32(hex_str: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.len() < 32 {
        return None;
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes[..32]);
    Some(key)
}

/// Parse a transparent amount from an `amount_commitment` hex string.
///
/// Botho uses transparent amounts (trivial zero-blinding Pedersen commitments),
/// and the node emits the plaintext amount as little-endian bytes in this
/// field (see `WalletScanner::parse_amount`). We need the amount to recompute
/// the ring member's commitment identically to the node's
/// `RingMember::from_output`.
fn parse_amount(hex_str: &str) -> Option<u64> {
    let bytes = hex::decode(hex_str).ok()?;
    if bytes.len() < 8 {
        return None;
    }
    Some(u64::from_le_bytes(bytes[..8].try_into().ok()?))
}

/// Convert an RPC output into a CLSAG [`RingMember`].
///
/// Reconstructs the same transparent commitment the node would compute for this
/// output via `RingMember::from_output`, so the ring member the wallet submits
/// matches the ledger's stored output at block acceptance.
fn rpc_output_to_ring_member(out: &RpcTxOutput) -> Option<RingMember> {
    let target_key = parse_key32(&out.target_key)?;
    let public_key = parse_key32(&out.public_key)?;
    let amount = parse_amount(&out.amount_commitment)?;

    let tx_out = TxOutput {
        amount,
        target_key,
        public_key,
        e_memo: None,
        cluster_tags: ClusterTagVector::empty(),
        kem_ciphertext: None,
    };
    Some(RingMember::from_output(&tx_out))
}

/// Fetch `count` decoy ring members for a real input of age `real_input_age`.
///
/// # Arguments
/// * `rpc` - connected RPC pool
/// * `real_input_age` - `current_height - utxo.created_at`
/// * `current_height` - current chain tip height
/// * `exclude_keys` - target keys of the wallet's own inputs (never used as
///   decoys)
/// * `count` - number of decoys required (`MIN_RING_SIZE - 1`)
///
/// # Errors
/// - The input is younger than [`MIN_DECOY_AGE_BLOCKS`] ("too young to spend
///   privately") — returned *before* any RPC call, so a fresh UTXO never panics
///   on a degenerate age band.
/// - The chain does not yet hold enough age-similar confirmed outputs to fill
///   the ring.
pub async fn fetch_decoy_ring_members(
    rpc: &mut RpcPool,
    real_input_age: u64,
    current_height: u64,
    exclude_keys: &[[u8; 32]],
    count: usize,
) -> Result<Vec<RingMember>> {
    // Young-input guard (mirrors node decoy_selection.rs guard; #611/#618).
    // Under the ±10% band the lower bound is floored at MIN_DECOY_AGE_BLOCKS,
    // so any input younger than that yields a degenerate band. Fail cleanly.
    if real_input_age < MIN_DECOY_AGE_BLOCKS {
        return Err(anyhow!(
            "Input is too new to spend privately — wait for at least {} confirmations \
             (current age: {} block(s)).",
            MIN_DECOY_AGE_BLOCKS,
            real_input_age
        ));
    }

    // Compute the ±10% age band and translate it into an inclusive block-height
    // window. Older age => lower height.
    let (min_age, max_age) = age_similarity_band(real_input_age);
    let start_height = current_height.saturating_sub(max_age);
    // end_height is the youngest allowed decoy height; min_age >= MIN_DECOY_AGE
    // guarantees it is at least MIN_DECOY_AGE_BLOCKS deep.
    let end_height = current_height.saturating_sub(min_age);

    // Fetch the candidate pool. `get_outputs` scans the [start, end] window.
    // We add 1 to end to make the window inclusive of the youngest in-band
    // height.
    let blocks = rpc
        .get_outputs(start_height, end_height.saturating_add(1))
        .await?;

    // Flatten to ring members, excluding our own inputs and malformed outputs.
    let mut pool: Vec<RingMember> = Vec::new();
    for block in &blocks {
        for out in &block.outputs {
            let member = match rpc_output_to_ring_member(out) {
                Some(m) => m,
                None => continue,
            };
            if exclude_keys.contains(&member.target_key) {
                continue;
            }
            pool.push(member);
        }
    }

    // De-duplicate by target key (an output can legitimately appear once; guard
    // against any accidental repeats across overlapping ranges).
    pool.sort_by(|a, b| a.target_key.cmp(&b.target_key));
    pool.dedup_by(|a, b| a.target_key == b.target_key);

    if pool.len() < count {
        return Err(anyhow!(
            "Not enough age-similar decoy outputs on-chain to build a ring. \
             Need {}, found {} in the ±10% age band [{}, {}] blocks. \
             The chain needs more confirmed outputs of similar age.",
            count,
            pool.len(),
            min_age,
            max_age
        ));
    }

    // Shuffle before taking N so ring membership is not a deterministic slice.
    let mut rng = OsRng;
    pool.shuffle(&mut rng);
    pool.truncate(count);
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::WalletKeys;

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

    /// Build a valid RPC output by generating a real stealth output to a wallet
    /// address, then serializing its fields the way `chain_getOutputs` does.
    fn random_rpc_output(amount: u64) -> RpcTxOutput {
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let out = TxOutput::new(amount, &keys.public_address());
        RpcTxOutput {
            tx_hash: hex::encode([0u8; 32]),
            output_index: 0,
            target_key: hex::encode(out.target_key),
            public_key: hex::encode(out.public_key),
            amount_commitment: hex::encode(amount.to_le_bytes()),
            cluster_tags: vec![],
            kem_ciphertext: None,
        }
    }

    #[test]
    fn test_rpc_output_to_ring_member_roundtrips_fields() {
        let out = random_rpc_output(12_345);
        let member = rpc_output_to_ring_member(&out).expect("valid output");
        assert_eq!(hex::encode(member.target_key), out.target_key);
        assert_eq!(hex::encode(member.public_key), out.public_key);
        // Commitment must equal the node's transparent commitment for 12_345.
        let expected = TxOutput {
            amount: 12_345,
            target_key: member.target_key,
            public_key: member.public_key,
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
            kem_ciphertext: None,
        };
        assert_eq!(
            member.commitment,
            RingMember::from_output(&expected).commitment
        );
    }

    #[test]
    fn test_rpc_output_to_ring_member_rejects_short_key() {
        let mut out = random_rpc_output(1);
        out.target_key = hex::encode([0u8; 4]); // too short
        assert!(rpc_output_to_ring_member(&out).is_none());
    }
}
