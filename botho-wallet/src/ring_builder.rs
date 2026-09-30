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
use bth_util_from_random::OsRng;
use rand::{seq::SliceRandom, Rng};

use crate::{
    decoy_selection::{age_similarity_band, MIN_DECOY_AGE_BLOCKS},
    rpc_pool::{BlockOutputs, RpcPool, TxOutput as RpcTxOutput},
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

    // Resolve target identity from history before narrowing the age window.
    // A later equal-target output is not a new independently spendable output.
    let history = rpc.get_outputs(0, current_height).await?;
    select_rpc_decoys_from_history(
        &history,
        current_height,
        real_input_age,
        exclude_keys,
        count,
        &mut OsRng,
    )
}

/// Canonical ordinary outputs in validated chain order. Keep the first target
/// across the entire supplied history, not the first target in an age window.
/// Lottery receipts remain in scans/accounting but are never independently
/// usable.
pub fn canonical_rpc_history(blocks: &[BlockOutputs]) -> Vec<BlockOutputs> {
    let mut ordered: Vec<_> = blocks.iter().collect();
    ordered.sort_by_key(|block| block.height);
    let mut seen = std::collections::HashSet::new();
    ordered
        .into_iter()
        .map(|block| BlockOutputs {
            height: block.height,
            outputs: block
                .outputs
                .iter()
                .filter(|output| {
                    // Older RPC servers/caches include a synthetic genesis
                    // mint, but genesis creates no ledger outputs.
                    block.height > 0
                        && !output.lottery
                        && parse_key32(&output.target_key).is_some_and(|key| seen.insert(key))
                })
                .cloned()
                .collect(),
        })
        .collect()
}

/// Prepare the live age band only after resolving canonical target identities.
pub fn select_rpc_decoys_from_history<R: Rng + ?Sized>(
    blocks: &[BlockOutputs],
    current_height: u64,
    real_input_age: u64,
    exclude_keys: &[[u8; 32]],
    count: usize,
    rng: &mut R,
) -> Result<Vec<RingMember>> {
    if real_input_age < MIN_DECOY_AGE_BLOCKS {
        return Err(anyhow!("Input is too new to spend privately"));
    }
    let (min_age, max_age) = age_similarity_band(real_input_age);
    let window: Vec<_> = canonical_rpc_history(blocks)
        .into_iter()
        .filter(|block| {
            current_height
                .checked_sub(block.height)
                .is_some_and(|age| age >= min_age && age <= max_age)
        })
        .collect();
    select_rpc_decoy_pool(&window, exclude_keys, count, min_age, max_age, rng)
}

/// Apply the live CLI ring pool preparation with an explicit RNG.
///
/// The full-history wrapper above supplies the canonical age window and OsRng.
/// This small extraction also permits deterministic research replay; it changes
/// neither filtering, error behavior nor production randomness. It does not
/// fetch/filter heights itself: callers must provide exactly the requested RPC
/// window.
#[doc(hidden)]
pub fn select_rpc_decoy_pool<R: Rng + ?Sized>(
    blocks: &[BlockOutputs],
    exclude_keys: &[[u8; 32]],
    count: usize,
    min_age: u64,
    max_age: u64,
    rng: &mut R,
) -> Result<Vec<RingMember>> {
    sample_prepared_rpc_decoy_pool(
        prepare_rpc_decoy_pool(blocks, exclude_keys),
        count,
        min_age,
        max_age,
        rng,
    )
}

/// Decode an already canonical age window (or a complete research fixture).
/// Live callers must resolve full history before age selection using
/// `select_rpc_decoys_from_history`; this helper cannot recover omitted
/// history.
#[doc(hidden)]
pub fn prepare_rpc_decoy_pool(
    blocks: &[BlockOutputs],
    exclude_keys: &[[u8; 32]],
) -> Vec<RingMember> {
    // Flatten to ring members, excluding our own inputs and malformed outputs.
    let mut pool: Vec<RingMember> = Vec::new();
    for block in canonical_rpc_history(blocks) {
        for out in &block.outputs {
            // The original winner can be outside this requested age window.
            // Deduplicating this window by target key cannot make a legacy
            // payout agree with the ledger's canonical first-target lookup.
            if out.lottery {
                continue;
            }
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

    pool
}

/// Sample a pool prepared by `prepare_rpc_decoy_pool`; production uses OsRng.
#[doc(hidden)]
pub fn sample_prepared_rpc_decoy_pool<R: Rng + ?Sized>(
    mut pool: Vec<RingMember>,
    count: usize,
    min_age: u64,
    max_age: u64,
    rng: &mut R,
) -> Result<Vec<RingMember>> {
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
    pool.shuffle(rng);
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
            crypto_output_index: None,
            coinbase: false,
            lottery: false,
            ledger_outpoint: None,
            target_key: hex::encode(out.target_key),
            public_key: hex::encode(out.public_key),
            amount_commitment: hex::encode(amount.to_le_bytes()),
            cluster_tags: vec![],
            kem_ciphertext: None,
        }
    }

    /// Captured from the fresh testnet RPC in #1485. Genesis has no ledger
    /// output, even though older servers advertised this zero-value
    /// placeholder.
    #[test]
    fn cached_genesis_output_is_never_a_canonical_decoy() {
        use rand::{rngs::StdRng, SeedableRng};
        let genesis: BlockOutputs =
            serde_json::from_str(include_str!("../tests/fixtures/genesis-output.json")).unwrap();
        assert_eq!(genesis.height, 0);
        assert_eq!(genesis.outputs[0].target_key, hex::encode([0u8; 32]));
        let mut blocks = vec![genesis];
        // Nineteen real outputs plus the old genesis placeholder: requesting
        // twenty decoys must fail, not silently fill the ring with genesis.
        for height in 1..=19 {
            blocks.push(BlockOutputs {
                height,
                outputs: vec![random_rpc_output(50_000_000_000_000)],
            });
        }
        let canonical = canonical_rpc_history(&blocks);
        assert_eq!(canonical[0].height, 0);
        assert!(canonical[0].outputs.is_empty());
        assert_eq!(canonical.iter().map(|b| b.outputs.len()).sum::<usize>(), 19);
        let mut rng = StdRng::seed_from_u64(1485);
        let ring = select_rpc_decoys_from_history(&blocks, 220, 219, &[], 19, &mut rng).unwrap();
        assert_eq!(ring.len(), 19);
        assert!(ring.iter().all(|member| member.target_key != [0u8; 32]));
        assert!(
            select_rpc_decoys_from_history(&blocks, 220, 219, &[], 20, &mut rng)
                .unwrap_err()
                .to_string()
                .contains("Need 20, found 19")
        );
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
    #[test]
    fn full_history_resolves_equal_targets_before_age_selection() {
        use rand::{rngs::StdRng, SeedableRng};
        let original = random_rpc_output(50_000_000_000_000);
        let mut alias = original.clone();
        alias.amount_commitment = hex::encode(20_000_000u64.to_le_bytes());
        alias.output_index = 1; // deliberately not flagged as a lottery receipt
        let mut outputs: Vec<_> = (0..19).map(|_| random_rpc_output(1_000_000)).collect();
        outputs.extend([alias.clone(), alias]);
        let blocks = vec![
            BlockOutputs {
                height: 90,
                outputs,
            },
            BlockOutputs {
                height: 1,
                outputs: vec![original.clone()],
            },
        ];
        let pool = select_rpc_decoys_from_history(
            &blocks,
            100,
            10,
            &[],
            19,
            &mut StdRng::seed_from_u64(1443),
        )
        .unwrap();
        assert!(pool
            .iter()
            .all(|member| hex::encode(member.target_key) != original.target_key));
        assert!(select_rpc_decoys_from_history(
            &blocks,
            100,
            10,
            &[],
            20,
            &mut StdRng::seed_from_u64(1443),
        )
        .unwrap_err()
        .to_string()
        .contains("Need 20, found 19"));
        let all = prepare_rpc_decoy_pool(&blocks, &[]);
        assert_eq!(all.len(), 20);
        assert!(all.contains(&rpc_output_to_ring_member(&original).unwrap()));
    }

    #[test]
    fn lottery_only_alias_is_not_a_decoy_when_original_is_outside_window() {
        use rand::{rngs::StdRng, SeedableRng};
        let mut payout = random_rpc_output(20_000_000);
        payout.lottery = true;
        let payout_key = parse_key32(&payout.target_key).unwrap();
        let mut outputs: Vec<_> = (0..19)
            .map(|_| random_rpc_output(50_000_000_000_000))
            .collect();
        outputs.push(payout);
        // The original winner is much older and absent from this age window.
        let blocks = vec![BlockOutputs {
            height: 90,
            outputs,
        }];
        let pool = prepare_rpc_decoy_pool(&blocks, &[]);
        assert_eq!(pool.len(), 19);
        assert!(pool.iter().all(|member| member.target_key != payout_key));
        assert!(
            select_rpc_decoy_pool(&blocks, &[], 19, 10, 11, &mut StdRng::seed_from_u64(1286))
                .is_ok()
        );
        let error =
            select_rpc_decoy_pool(&blocks, &[], 20, 10, 11, &mut StdRng::seed_from_u64(1286))
                .unwrap_err();
        assert!(error.to_string().contains("Need 20, found 19"));
    }

    #[test]
    fn lottery_alias_does_not_replace_original_decoy_regardless_of_order() {
        let original = random_rpc_output(50_000_000_000_000);
        let expected = rpc_output_to_ring_member(&original).unwrap();
        let mut payout = original.clone();
        payout.lottery = true;
        payout.amount_commitment = hex::encode(20_000_000u64.to_le_bytes());
        for outputs in [
            vec![payout.clone(), original.clone()],
            vec![original.clone(), payout.clone()],
        ] {
            let pool = prepare_rpc_decoy_pool(
                &[BlockOutputs {
                    height: 90,
                    outputs,
                }],
                &[],
            );
            assert_eq!(pool, vec![expected.clone()]);
        }
    }
}
