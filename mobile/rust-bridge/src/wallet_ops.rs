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
    /// Hex-encoded 64-byte BIP39 seed of the wallet (never leaves device). The
    /// RECEIVE scan derives the wallet's ML-KEM SECRET from this
    /// (feature-independent `derive_pq_keys_from_seed`, NOT
    /// `WalletKeys::public_address()` — which needs the `pq` feature the mobile
    /// `botho-wallet` build disables) to decapsulate and detect 6.0.0 hybrid
    /// outputs — incoming payments and the wallet's own change (issue #988).
    pub seed: String,
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

/// The node reports a coinbase output's `outputIndex` as this sentinel to
/// distinguish it from regular transaction outputs; its hybrid one-time key is
/// bound to `MINTING_OUTPUT_INDEX` (0), so normalize it before scanning.
const COINBASE_OUTPUT_INDEX_SENTINEL: u32 = u32::MAX;
/// The index a coinbase output's hybrid one-time key is actually bound to (the
/// node's `MINTING_OUTPUT_INDEX`).
const MINTING_OUTPUT_INDEX: u32 = 0;

/// One chain output as the wallet sees it: the scan input plus whether the
/// node flagged it as a lottery payout.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub output: ChainOutput,
    /// `lottery` flag from `chain_getOutputs`. A flagged row WITHOUT a
    /// `lotteryContext` is a legacy (V1) payout aliasing its source (#1450).
    pub lottery: bool,
}

impl Candidate {
    /// A legacy, context-less lottery payout: never an input or a decoy
    /// (#1450).
    pub fn is_legacy_lottery(&self) -> bool {
        self.lottery && self.output.lottery_context.is_none()
    }

    /// Any lottery payout (legacy or V2). Payouts stay out of the decoy set,
    /// matching the native wallet's initial V2 policy (#1642).
    pub fn is_lottery_payout(&self) -> bool {
        self.lottery || self.output.lottery_context.is_some()
    }
}

/// Fetch every chain output (with transparent amount, its one-time-key
/// derivation index, its ML-KEM ciphertext and any V2 lottery payout context)
/// — everything the RECEIVE scan needs to detect 6.0.0 hybrid outputs (#988)
/// and V2 lottery payouts (#1645).
async fn fetch_candidates(rpc: &NodeRpc, height: u64) -> Result<Vec<Candidate>, String> {
    let raw = rpc.get_outputs(0, height).await?;
    Ok(raw.into_iter().map(candidate_from_rpc).collect())
}

/// Decode one `chain_getOutputs` row into a [`Candidate`].
fn candidate_from_rpc(o: crate::rpc::RpcOutput) -> Candidate {
    // Prefer the node's `cryptoOutputIndex` (it is `baseIndex` for a V2
    // payout). Older nodes omit it: normalize the coinbase sentinel to the
    // index its one-time key is actually bound to, so a coinbase is detectable.
    let output_index =
        o.crypto_output_index
            .unwrap_or(if o.output_index == COINBASE_OUTPUT_INDEX_SENTINEL {
                MINTING_OUTPUT_INDEX
            } else {
                o.output_index
            });
    Candidate {
        output: ChainOutput {
            target_key: o.target_key,
            public_key: o.public_key,
            amount: amount_from_commitment(&o.amount_commitment),
            output_index,
            kem_ciphertext: o.kem_ciphertext,
            lottery_context: o.lottery_context,
        },
        lottery: o.lottery,
    }
}

/// The canonical row for each target, in chain order: the first occurrence
/// that is not a legacy lottery alias (#1450). V2 payouts have their own
/// one-time key, so they are canonical (#1645).
pub fn canonical_candidates(candidates: &[Candidate]) -> Vec<&Candidate> {
    let mut seen = std::collections::HashSet::new();
    candidates
        .iter()
        .filter(|c| !c.is_legacy_lottery() && seen.insert(c.output.target_key.clone()))
        .collect()
}

/// Spendable outputs that may be selected as inputs: each must match its
/// canonical row's envelope (keys, amount, derivation index and V2 context),
/// once per target. Mirrors the web orchestrator (`send.ts`). Legacy lottery
/// aliases are left out, so they still count toward the displayed balance but
/// are never spent independently (#1450).
pub fn supported_inputs(spendable: &[OwnedOutput], canonical: &[&Candidate]) -> Vec<OwnedOutput> {
    let by_target: std::collections::HashMap<&str, &ChainOutput> = canonical
        .iter()
        .map(|c| (c.output.target_key.as_str(), &c.output))
        .collect();
    let mut seen = std::collections::HashSet::new();
    spendable
        .iter()
        .filter(|input| {
            by_target
                .get(input.target_key.as_str())
                .is_some_and(|original| {
                    original.public_key == input.public_key
                        && original.amount == input.amount
                        && original.output_index == input.output_index
                        && original.lottery_context == input.lottery_context
                })
                && seen.insert(input.target_key.clone())
        })
        .cloned()
        .collect()
}

/// Ring decoys: canonical, non-lottery, non-input, non-genesis outputs.
pub fn decoy_pool<'a>(canonical: &[&'a Candidate], inputs: &[OwnedOutput]) -> Vec<&'a ChainOutput> {
    let input_keys: std::collections::HashSet<&str> =
        inputs.iter().map(|i| i.target_key.as_str()).collect();
    canonical
        .iter()
        .filter(|c| {
            !c.is_lottery_payout()
                && !input_keys.contains(c.output.target_key.as_str())
                && !is_zero_key(&c.output.target_key)
        })
        .map(|c| &c.output)
        .collect()
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
        // Seed-derived ML-KEM secret so the scan decapsulates hybrid outputs
        // (incoming payments + our own change) under 6.0.0 (issue #988).
        seed: keys.seed.clone(),
        // V2 lottery payouts are discovered through their context (#1645).
        outputs: candidates.into_iter().map(|c| c.output).collect(),
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
        // Seed so a hybrid owned output's one-time spend key (hence key image)
        // recovers via the unified path (issue #988).
        seed: keys.seed.clone(),
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
                // Preserve hybrid metadata so a later spend recovers the
                // one-time key on the unified path (issue #988).
                output_index: o.output_index,
                kem_ciphertext: o.kem_ciphertext,
                // V2 lottery payout context for spend recovery (#1645).
                lottery_context: o.lottery_context,
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

    // Resolve each target once in chain order before selecting inputs or
    // decoys (#1450): legacy lottery aliases are never selected; V2 payouts
    // are ordinary canonical inputs (#1645).
    let height = rpc.chain_height().await.map_err(SendError::Other)?;
    let candidates = fetch_candidates(rpc, height)
        .await
        .map_err(SendError::Other)?;
    let request = plan_send(
        keys,
        recipient,
        amount,
        fee,
        height,
        &synced.spendable,
        &candidates,
    )?;

    let tx_hex = build_and_sign_inner(&request).map_err(SendError::Other)?;
    rpc.submit_transaction(&tx_hex)
        .await
        .map_err(SendError::Other)
}

/// Plan a send offline: canonical input selection over the spendable set,
/// decoy gathering, and the [`SignRequest`] for the shared signer core. Split
/// out of [`send`] (which only adds the RPC fetch, sign and submit) so the
/// selection rules are testable without a node.
pub fn plan_send(
    keys: &SignerKeys,
    recipient: RecipientAddress,
    amount: u64,
    fee: u64,
    height: u64,
    spendable: &[OwnedOutput],
    candidates: &[Candidate],
) -> Result<SignRequest, SendError> {
    let target = amount
        .checked_add(fee)
        .ok_or_else(|| SendError::Other("amount + fee overflow".into()))?;
    let canonical = canonical_candidates(candidates);
    let supported = supported_inputs(spendable, &canonical);

    let inputs = match select_inputs(&supported, target) {
        Some(i) => i,
        None if supported.len() != spendable.len() => {
            return Err(SendError::Other(
                "Legacy lottery payouts cannot be spent independently yet".into(),
            ))
        }
        None => return Err(SendError::Insufficient),
    };

    // Gather decoys: any canonical on-chain output that is not one of the real
    // inputs, not a lottery payout, and not the all-zero genesis placeholder.
    // Decoys may include the wallet's own other outputs (the node's decoy
    // selector only excludes the real inputs), which keeps a low-traffic
    // testnet spendable.
    let decoys_per_input = DEFAULT_RING_SIZE - 1;
    let decoy_pool = decoy_pool(&canonical, &inputs);

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
                // Thread the hybrid metadata scanned on the receive side (#988):
                // under protocol 6.0.0 the one-time key is bound to
                // `output_index` and recovered by decapsulating `kem_ciphertext`
                // with the wallet seed. Omitting these would classically recover
                // a hybrid input's spend key, yielding the wrong one-time key and
                // an invalid CLSAG signature — the same bug this PR fixes for web.
                output_index: input.output_index,
                kem_ciphertext: input.kem_ciphertext.clone(),
                // V2 lottery payout input: recovered through its context with
                // the x·G == P check (#1645).
                lottery_context: input.lottery_context.clone(),
                decoys,
            }
        })
        .collect();

    let request = SignRequest {
        spend_private_key: keys.spend_private_key.clone(),
        view_private_key: keys.view_private_key.clone(),
        // The wallet seed lets the signer derive the ML-KEM secret and
        // decapsulate a hybrid input's ciphertext to recover its true one-time
        // spend key on the unified path (#988). Never leaves the device.
        seed: keys.seed.clone(),
        inputs: spend_inputs,
        recipient,
        // The sender's own published ML-KEM-768 key: the change (self-send)
        // output encapsulates against this so the sender can recover it under
        // the 6.0.0 hybrid scheme (issue #978).
        sender_kem_public_key: keys.sender_kem_public_key.clone(),
        amount,
        fee,
        created_at_height: height,
        // The mobile wallet has no bridge-deposit flow, so it never sets the
        // dedicated bridge order-memo channel (#1037). An ordinary send carries
        // no on-chain memo — byte-identical to before.
        bridge_deposit_memo: None,
    };
    Ok(request)
}

fn is_zero_key(k: &str) -> bool {
    !k.is_empty() && k.bytes().all(|b| b == b'0')
}

/// V2 lottery payouts in the mobile wallet core (#1645). Driven by the same
/// committed vector the WASM signer is checked against
/// (`web/packages/wasm-signer/test/fixtures/lottery-v2-payout.json`), so the
/// mobile path, the compiled WASM and native recovery agree on one payout.
#[cfg(test)]
mod lottery_v2_tests {
    use super::*;
    use serde_json::Value;

    const VECTOR: &str =
        include_str!("../../../web/packages/wasm-signer/test/fixtures/lottery-v2-payout.json");

    fn vector() -> Value {
        serde_json::from_str(VECTOR).unwrap()
    }

    /// Re-encode a decoded fixture row as the node serves it (amount as a LE
    /// `amountCommitment`) and decode it through the mobile RPC path.
    fn candidate(row: &Value) -> Candidate {
        let mut row = row.clone();
        let amount = row["amount"].as_u64().unwrap();
        row["amountCommitment"] = Value::String(hex::encode(amount.to_le_bytes()));
        candidate_from_rpc(serde_json::from_value(row).unwrap())
    }

    fn keys(v: &Value) -> SignerKeys {
        SignerKeys {
            spend_private_key: v["spendPrivateKey"].as_str().unwrap().into(),
            view_private_key: v["viewPrivateKey"].as_str().unwrap().into(),
            sender_kem_public_key: v["senderKemPublicKey"].as_str().unwrap().into(),
            seed: v["seed"].as_str().unwrap().into(),
        }
    }

    fn recipient(v: &Value) -> RecipientAddress {
        serde_json::from_value(v["recipient"].clone()).unwrap()
    }

    /// Chain order: the source, the ordinary decoy rows, then the payout.
    fn chain(v: &Value, extra: &[Value]) -> Vec<Candidate> {
        let mut rows = vec![v["source"].clone()];
        rows.extend(v["decoys"].as_array().unwrap().iter().cloned());
        rows.push(v["payout"].clone());
        rows.extend(extra.iter().cloned());
        rows.iter().map(candidate).collect()
    }

    /// The `sync` scan + key-image step (every output treated as unspent).
    fn scan(v: &Value, candidates: &[Candidate]) -> Vec<(OwnedOutput, String)> {
        let k = keys(v);
        let owned = scan_owned_outputs_inner(&ScanRequest {
            spend_private_key: k.spend_private_key.clone(),
            view_private_key: k.view_private_key.clone(),
            seed: k.seed.clone(),
            outputs: candidates.iter().map(|c| c.output.clone()).collect(),
        })
        .unwrap();
        compute_owned_output_key_images_inner(&KeyImageRequest {
            spend_private_key: k.spend_private_key,
            view_private_key: k.view_private_key,
            seed: k.seed,
            outputs: owned,
        })
        .unwrap()
        .into_iter()
        .map(|o| {
            (
                OwnedOutput {
                    target_key: o.target_key,
                    public_key: o.public_key,
                    amount: o.amount,
                    subaddress_index: o.subaddress_index,
                    output_index: o.output_index,
                    kem_ciphertext: o.kem_ciphertext,
                    lottery_context: o.lottery_context,
                },
                o.key_image,
            )
        })
        .collect()
    }

    #[test]
    fn rpc_row_carries_the_v2_context_and_crypto_index() {
        let v = vector();
        let payout = candidate(&v["payout"]);
        assert!(payout.lottery && !payout.is_legacy_lottery());
        // `cryptoOutputIndex` (= baseIndex 3), not the payout outpoint index 1.
        assert_eq!(payout.output.output_index, 3);
        assert_eq!(
            payout.output.lottery_context.as_ref().unwrap().base_index,
            3
        );
        let source = candidate(&v["source"]);
        assert!(!source.lottery && source.output.lottery_context.is_none());
    }

    #[test]
    fn v2_payout_is_discovered_under_its_native_key_image() {
        let v = vector();
        let owned = scan(&v, &chain(&v, &[]));
        let images: Vec<&str> = owned.iter().map(|(_, ki)| ki.as_str()).collect();
        assert_eq!(
            images,
            [
                v["expected"]["sourceKeyImage"].as_str().unwrap(),
                v["expected"]["payoutKeyImage"].as_str().unwrap(),
            ]
        );
        assert!(owned[1].0.lottery_context.is_some());
    }

    #[test]
    fn v2_payout_funds_a_send_and_stays_out_of_the_ring_decoys() {
        let v = vector();
        let candidates = chain(&v, &[]);
        let spendable: Vec<OwnedOutput> =
            scan(&v, &candidates).into_iter().map(|(o, _)| o).collect();
        let payout_key = v["payout"]["targetKey"].as_str().unwrap();
        // More than the source holds: only the payout can fund it.
        let request = plan_send(
            &keys(&v),
            recipient(&v),
            2_000_000_000_000,
            MIN_TX_FEE,
            40,
            &spendable,
            &candidates,
        )
        .ok()
        .expect("plan a send funded by the V2 payout");
        assert_eq!(request.inputs.len(), 1);
        assert_eq!(request.inputs[0].target_key, payout_key);
        assert!(request.inputs[0].lottery_context.is_some());
        assert!(request.inputs[0]
            .decoys
            .iter()
            .all(|d| d.target_key != payout_key));
        let tx_hex = build_and_sign_inner(&request).expect("signer builds the payout spend");
        assert!(tx_hex.contains(v["expected"]["payoutKeyImage"].as_str().unwrap()));
    }

    #[test]
    fn legacy_context_less_alias_is_never_selected_or_used_as_a_decoy() {
        let v = vector();
        // A legacy payout re-uses the source's key (#1450) with a new amount.
        let mut legacy = v["source"].clone();
        legacy["amount"] = Value::from(9_000_000_000_000u64);
        legacy["lottery"] = Value::Bool(true);
        let candidates = chain(&v, &[legacy]);
        assert!(candidates.last().unwrap().is_legacy_lottery());
        let spendable: Vec<OwnedOutput> =
            scan(&v, &candidates).into_iter().map(|(o, _)| o).collect();
        assert_eq!(
            spendable.len(),
            3,
            "the alias still counts as a recognized receipt"
        );

        let canonical = canonical_candidates(&candidates);
        let supported = supported_inputs(&spendable, &canonical);
        assert_eq!(supported.len(), 2, "source + V2 payout, never the alias");
        let pool = decoy_pool(&canonical, &[]);
        assert!(pool.iter().all(|d| d.lottery_context.is_none()));
        assert_eq!(
            pool.len(),
            candidates.len() - 2,
            "payout and alias excluded"
        );

        // A send only the alias could fund is refused explicitly.
        let result = plan_send(
            &keys(&v),
            recipient(&v),
            8_000_000_000_000,
            MIN_TX_FEE,
            40,
            &spendable,
            &candidates,
        );
        assert!(matches!(result, Err(SendError::Other(m)) if m.contains("Legacy lottery payouts")));
    }
}
