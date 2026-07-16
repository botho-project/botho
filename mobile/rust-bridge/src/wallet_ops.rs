//! Wallet operations: sync, balance, and CLSAG send.
//!
//! All cryptography is delegated to the shared, node-identical
//! `bth_wasm_signer::core` primitives (ownership scan, key-image derivation,
//! ring build + CLSAG sign). This module only orchestrates them with the node
//! RPC -- it is a native port of the browser wallet's `send.ts` flow so the
//! mobile bridge produces transactions the node verifier accepts byte-for-byte.

use bth_transaction_clsag::{DEFAULT_RING_SIZE, DUST_THRESHOLD, MIN_TX_FEE};
use bth_wasm_signer::core::{
    build_and_sign_inner, compute_owned_output_key_images_inner, scan_owned_outputs_inner,
    ChainOutput, DecoyOutput, KeyImageRequest, OwnedOutput, RecipientAddress, ScanRequest,
    SignRequest, SpendInput,
};

use crate::rpc::{amount_from_commitment, NodeRpc};

/// Hex-encoded account keys used to drive the signer core (never leave device).
pub struct SignerKeys {
    pub spend_private_key: String,
    pub view_private_key: String,
    /// Hex-encoded raw ML-KEM-768 public key (1184 bytes) of the wallet's OWN
    /// v2 address, derived from its BIP39 seed via the node-identical
    /// `derive_pq_keys_from_seed`. The change output is a self-send whose
    /// ciphertext is encapsulated against this key, so the sender can later
    /// recover its change under the 6.0.0 hybrid scheme (issue #978).
    pub sender_kem_public_key: String,
}

/// A wallet's spendable view of the chain: owned, unspent outputs + the height
/// they were synced against.
pub struct SyncedWallet {
    pub spendable: Vec<OwnedOutput>,
    pub height: u64,
}

impl SyncedWallet {
    /// Total spendable balance in picocredits.
    pub fn balance(&self) -> u64 {
        self.spendable.iter().map(|o| o.amount).sum()
    }
}

/// Fetch every chain output as a `ChainOutput` (with transparent amount).
async fn fetch_candidates(rpc: &NodeRpc, height: u64) -> Result<Vec<ChainOutput>, String> {
    let raw = rpc.get_outputs(0, height).await?;
    Ok(raw
        .into_iter()
        .map(|o| ChainOutput {
            target_key: o.target_key,
            public_key: o.public_key,
            amount: amount_from_commitment(&o.amount_commitment),
        })
        .collect())
}

/// Sync the wallet: scan the chain for owned outputs and exclude any that are
/// already spent on-chain or pending in the mempool (the #392 spent-filter
/// model, mirrored from `wasm-signer`'s `spendableOwnedOutputs`).
pub async fn sync(rpc: &NodeRpc, keys: &SignerKeys) -> Result<SyncedWallet, String> {
    let height = rpc.chain_height().await?;
    let candidates = fetch_candidates(rpc, height).await?;

    if candidates.is_empty() {
        return Ok(SyncedWallet {
            spendable: vec![],
            height,
        });
    }

    let owned = scan_owned_outputs_inner(&ScanRequest {
        spend_private_key: keys.spend_private_key.clone(),
        view_private_key: keys.view_private_key.clone(),
        outputs: candidates,
    })?;

    if owned.is_empty() {
        return Ok(SyncedWallet {
            spendable: vec![],
            height,
        });
    }

    let spendable = filter_spendable(rpc, keys, owned).await?;
    Ok(SyncedWallet { spendable, height })
}

/// Exclude owned outputs whose key image is spent on-chain or pending. Anything
/// without a clear "unspent" answer is treated as spent (never overstate
/// balance, never select a spent output as an input).
async fn filter_spendable(
    rpc: &NodeRpc,
    keys: &SignerKeys,
    owned: Vec<OwnedOutput>,
) -> Result<Vec<OwnedOutput>, String> {
    if owned.is_empty() {
        return Ok(vec![]);
    }

    let with_images = compute_owned_output_key_images_inner(&KeyImageRequest {
        spend_private_key: keys.spend_private_key.clone(),
        view_private_key: keys.view_private_key.clone(),
        outputs: owned,
    })?;

    let key_images: Vec<String> = with_images.iter().map(|o| o.key_image.clone()).collect();
    let statuses = rpc.are_key_images_spent(&key_images).await?;

    let mut spendable = Vec::new();
    for o in with_images {
        let unspent = statuses
            .iter()
            .find(|s| s.key_image == o.key_image)
            .map(|s| !s.spent && !s.pending)
            .unwrap_or(false);
        if unspent {
            spendable.push(OwnedOutput {
                target_key: o.target_key,
                public_key: o.public_key,
                amount: o.amount,
                subaddress_index: o.subaddress_index,
            });
        }
    }
    Ok(spendable)
}

/// Greedily select the fewest owned outputs (largest-first) covering `target`.
fn select_inputs(owned: &[OwnedOutput], target: u64) -> Option<Vec<OwnedOutput>> {
    let mut sorted = owned.to_vec();
    sorted.sort_by(|a, b| b.amount.cmp(&a.amount));

    let mut chosen = Vec::new();
    let mut total: u64 = 0;
    for o in sorted {
        total = total.saturating_add(o.amount);
        chosen.push(o);
        if total >= target {
            return Some(chosen);
        }
    }
    None
}

/// Outcome of a send build: insufficient funds is distinguished so the bridge
/// can surface the right error variant.
pub enum SendError {
    /// Spendable balance cannot cover amount + fee.
    Insufficient,
    /// Any other build/submit failure (network, decoys, signer).
    Other(String),
}

/// Build and CLSAG-sign a transfer of `amount` picocredits to `recipient`,
/// submit it, and return the tx hash.
///
/// Mirrors `wasm-signer/src/send.ts` `buildSendTransaction`: scan ->
/// spent-filter -> select inputs -> gather decoys -> build+sign (node-identical
/// core) -> `tx_submit`.
pub async fn send(
    rpc: &NodeRpc,
    keys: &SignerKeys,
    recipient: RecipientAddress,
    amount: u64,
    synced: &SyncedWallet,
) -> Result<String, SendError> {
    if amount == 0 {
        return Err(SendError::Other("amount must be greater than 0".into()));
    }
    if amount < DUST_THRESHOLD {
        return Err(SendError::Other(format!(
            "amount {amount} is below the dust threshold of {DUST_THRESHOLD} picocredits"
        )));
    }

    let fee = MIN_TX_FEE;
    let target = amount
        .checked_add(fee)
        .ok_or_else(|| SendError::Other("amount + fee overflow".into()))?;

    let inputs = match select_inputs(&synced.spendable, target) {
        Some(i) => i,
        None => return Err(SendError::Insufficient),
    };

    // Gather decoys: any on-chain output that is not one of the real inputs and
    // is not the all-zero genesis placeholder. Decoys may include the wallet's
    // own other outputs (the node's decoy selector only excludes the real
    // inputs), which keeps a low-traffic testnet spendable.
    let height = rpc.chain_height().await.map_err(SendError::Other)?;
    let candidates = fetch_candidates(rpc, height)
        .await
        .map_err(SendError::Other)?;

    let decoys_per_input = DEFAULT_RING_SIZE - 1;
    let input_keys: std::collections::HashSet<&String> =
        inputs.iter().map(|i| &i.target_key).collect();
    let decoy_pool: Vec<&ChainOutput> = candidates
        .iter()
        .filter(|c| !input_keys.contains(&c.target_key) && !is_zero_key(&c.target_key))
        .collect();

    if decoy_pool.len() < decoys_per_input {
        return Err(SendError::Other(format!(
            "not enough decoys on chain for a ring of {DEFAULT_RING_SIZE}: need {decoys_per_input} \
             per input, found {}. Wait for more on-chain outputs.",
            decoy_pool.len()
        )));
    }

    let spend_inputs: Vec<SpendInput> = inputs
        .iter()
        .enumerate()
        .map(|(i, input)| {
            let decoys: Vec<DecoyOutput> = (0..decoys_per_input)
                .map(|j| {
                    let d = decoy_pool[(i * decoys_per_input + j) % decoy_pool.len()];
                    DecoyOutput {
                        target_key: d.target_key.clone(),
                        public_key: d.public_key.clone(),
                        amount: d.amount,
                    }
                })
                .collect();
            SpendInput {
                target_key: input.target_key.clone(),
                public_key: input.public_key.clone(),
                amount: input.amount,
                subaddress_index: input.subaddress_index,
                decoys,
            }
        })
        .collect();

    let request = SignRequest {
        spend_private_key: keys.spend_private_key.clone(),
        view_private_key: keys.view_private_key.clone(),
        inputs: spend_inputs,
        recipient,
        // The sender's own published ML-KEM-768 key: the change (self-send)
        // output encapsulates against this so the sender can recover it under
        // the 6.0.0 hybrid scheme (issue #978).
        sender_kem_public_key: keys.sender_kem_public_key.clone(),
        amount,
        fee,
        created_at_height: height,
    };

    let tx_hex = build_and_sign_inner(&request).map_err(SendError::Other)?;
    rpc.submit_transaction(&tx_hex)
        .await
        .map_err(SendError::Other)
}

fn is_zero_key(k: &str) -> bool {
    !k.is_empty() && k.bytes().all(|b| b == b'0')
}
