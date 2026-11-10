//! Wallet operations: sync, balance, and CLSAG send.
//!
//! All cryptography is delegated to the shared, node-identical
//! `bth_wasm_signer::core` primitives (ownership scan, key-image derivation,
//! ring build + CLSAG sign). This module only orchestrates them with the node
//! RPC -- it is a native port of the browser wallet's `send.ts` flow so the
//! mobile bridge produces transactions the node verifier accepts byte-for-byte.

use botho_wallet::decoy_selection::{
    select_aged_decoys, DecoySelectionError, MIN_DECOY_AGE_BLOCKS,
};
use bth_transaction_clsag::{DEFAULT_RING_SIZE, DUST_THRESHOLD, MIN_TX_FEE};
use bth_util_from_random::OsRng;
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
    /// Height of the block that created this output (from `chain_getOutputs`;
    /// the RPC client fails closed when a block omits it). Ring decoys are
    /// chosen by age `chain_height − height` (#1861).
    pub height: u64,
    /// The output's stored cluster tags (`clusterTags`). When this output is
    /// spent, the signer's outputs inherit them decayed 5%; since protocol
    /// 10.0.0 consensus rejects a spend that keeps less than 95% (#1687).
    pub cluster_tags: Vec<(u64, u32)>,
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
        height: o.height,
        cluster_tags: o.cluster_tags,
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

/// True only when a LEGACY lottery payout is what blocks funding: some owned
/// output was left out of `supported` because it matches a context-less
/// lottery row's envelope (#1450), and counting those payouts would cover
/// `target`. Other exclusions (a duplicated ordinary target, an envelope that
/// disagrees with its canonical row) report plain insufficient funds.
pub fn legacy_payouts_block_selection(
    spendable: &[OwnedOutput],
    supported: &[OwnedOutput],
    candidates: &[Candidate],
    target: u64,
) -> bool {
    let supported_total: u64 = supported.iter().map(|o| o.amount).sum();
    let mut counted = std::collections::HashSet::new();
    let legacy_total: u64 = spendable
        .iter()
        .filter(|o| {
            !supported.iter().any(|s| {
                s.target_key == o.target_key && s.amount == o.amount && s.public_key == o.public_key
            })
        })
        .filter(|o| {
            candidates.iter().any(|c| {
                c.is_legacy_lottery()
                    && c.output.target_key == o.target_key
                    && c.output.public_key == o.public_key
                    && c.output.amount == o.amount
            })
        })
        .filter(|o| counted.insert((o.target_key.clone(), o.amount)))
        .map(|o| o.amount)
        .sum();
    legacy_total > 0 && supported_total.saturating_add(legacy_total) >= target
}

/// Ring decoys: canonical, non-lottery, non-input, non-genesis outputs.
///
/// The send path uses [`aged_decoy_pool`] (it needs each row's height). This
/// output-only view is kept for the decoy-exclusion tests.
#[cfg(test)]
pub fn decoy_pool<'a>(canonical: &[&'a Candidate], inputs: &[OwnedOutput]) -> Vec<&'a ChainOutput> {
    aged_decoy_pool(canonical, inputs)
        .into_iter()
        .map(|c| &c.output)
        .collect()
}

/// [`decoy_pool`] as full candidates, so the send path can read each decoy's
/// creation height.
fn aged_decoy_pool<'a>(canonical: &[&'a Candidate], inputs: &[OwnedOutput]) -> Vec<&'a Candidate> {
    let input_keys: std::collections::HashSet<&str> =
        inputs.iter().map(|i| i.target_key.as_str()).collect();
    canonical
        .iter()
        .filter(|c| {
            !c.is_lottery_payout()
                && !input_keys.contains(c.output.target_key.as_str())
                && !is_zero_key(&c.output.target_key)
        })
        .copied()
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

    // Resolve each target once in chain order before selecting inputs or
    // decoys (#1450): legacy lottery aliases are never selected; V2 payouts
    // are ordinary canonical inputs (#1645).
    let height = rpc.chain_height().await.map_err(SendError::Other)?;
    let candidates = fetch_candidates(rpc, height)
        .await
        .map_err(SendError::Other)?;

    // Price against the node's own fee floor (#2001). The flat MIN_TX_FEE
    // underpays any spend that owes the class-exit charge (#1685) or the
    // input import floor (#1687), which the node then refuses as `Fee too
    // low`. Sign at MIN_TX_FEE, quote the draft with `estimateFee`, and
    // re-sign at the quote until the fee settles, as `botho send` does; each
    // pass reselects inputs for amount + fee and draws a fresh ring.
    let tx_hex = price_send(
        MIN_TX_FEE,
        |fee| {
            let request = plan_send(
                keys,
                recipient.clone(),
                amount,
                fee,
                height,
                &synced.spendable,
                &candidates,
            )?;
            build_and_sign_inner(&request).map_err(SendError::Other)
        },
        |tx_hex: String| async move { rpc.quote_draft_fee(&tx_hex).await },
    )
    .await?;
    rpc.submit_transaction(&tx_hex)
        .await
        .map_err(SendError::Other)
}

/// Sign at `seed_fee`, quote each draft with `quote`, and re-sign at the
/// quoted `resignFee` until the signed fee settles (#2001). Returns the
/// settled tx hex. `sign(fee)` plans and signs a draft paying `fee`.
pub async fn price_send<S, Q, F>(seed_fee: u64, mut sign: S, quote: Q) -> Result<String, SendError>
where
    S: FnMut(u64) -> Result<String, SendError>,
    Q: Fn(String) -> F,
    F: std::future::Future<Output = Result<botho_wallet::fee_estimation::DraftFeeQuote, String>>,
{
    use botho_wallet::fee_estimation::{next_fee_pass, MAX_FEE_PASSES};
    let mut fee = seed_fee.max(MIN_TX_FEE);
    for _ in 0..MAX_FEE_PASSES {
        let tx_hex = sign(fee)?;
        let quoted = quote(tx_hex.clone()).await.map_err(SendError::Other)?;
        match next_fee_pass(fee, &quoted) {
            None => return Ok(tx_hex),
            Some(next) => fee = next,
        }
    }
    Err(SendError::Other(format!(
        "could not settle a fee meeting the node's fee floor after {MAX_FEE_PASSES} passes"
    )))
}

/// Plan a send offline: canonical input selection over the spendable set,
/// decoy gathering, and the [`SignRequest`] for the shared signer core. Split
/// out of [`send`] (which only adds the RPC fetch, sign and submit) so the
/// selection rules are testable without a node.
///
/// Decoys come from the OS CSPRNG; see [`plan_send_with_rng`].
pub fn plan_send(
    keys: &SignerKeys,
    recipient: RecipientAddress,
    amount: u64,
    fee: u64,
    height: u64,
    spendable: &[OwnedOutput],
    candidates: &[Candidate],
) -> Result<SignRequest, SendError> {
    plan_send_with_rng(
        keys, recipient, amount, fee, height, spendable, candidates, &mut OsRng,
    )
}

/// [`plan_send`] with an injectable RNG for the decoy draw. Production passes
/// a CSPRNG; tests may pass a seeded one.
#[allow(clippy::too_many_arguments)]
pub fn plan_send_with_rng<R: rand::Rng + ?Sized>(
    keys: &SignerKeys,
    recipient: RecipientAddress,
    amount: u64,
    fee: u64,
    height: u64,
    spendable: &[OwnedOutput],
    candidates: &[Candidate],
    rng: &mut R,
) -> Result<SignRequest, SendError> {
    let target = amount
        .checked_add(fee)
        .ok_or_else(|| SendError::Other("amount + fee overflow".into()))?;
    let canonical = canonical_candidates(candidates);
    let supported = supported_inputs(spendable, &canonical);

    let inputs = match select_inputs(&supported, target) {
        Some(i) => i,
        None if legacy_payouts_block_selection(spendable, &supported, candidates, target) => {
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
    //
    // Per-input selection (#1861) uses the node/web rule through the shared
    // `botho_wallet::decoy_selection::select_aged_decoys`: confirmed decoys
    // (>= 10 blocks), drawn with a CSPRNG from a random-offset ±5% age window
    // that holds the real input's age, filled by nearest age when the window
    // is thin, then shuffled. This replaces the chain-ordered
    // `pool[(i*k + j) % len]` window, which gave every ring the same outputs
    // regardless of the real input's age.
    let decoys_per_input = DEFAULT_RING_SIZE - 1;
    let aged_pool: Vec<(&ChainOutput, u64)> = aged_decoy_pool(&canonical, &inputs)
        .into_iter()
        .map(|c| (&c.output, height.saturating_sub(c.height)))
        .collect();
    let canonical_height: std::collections::HashMap<&str, u64> = canonical
        .iter()
        .map(|c| (c.output.target_key.as_str(), c.height))
        .collect();
    let canonical_tags: std::collections::HashMap<&str, &[(u64, u32)]> = canonical
        .iter()
        .map(|c| (c.output.target_key.as_str(), c.cluster_tags.as_slice()))
        .collect();

    let mut spend_inputs: Vec<SpendInput> = Vec::with_capacity(inputs.len());
    for input in &inputs {
        // The real input's age comes from its canonical chain row, as web's
        // `outputAge(original, height)` does. `supported_inputs` only admits
        // inputs that have one.
        let input_height = *canonical_height
            .get(input.target_key.as_str())
            .ok_or_else(|| {
                SendError::Other("Selected input is missing from the chain scan".into())
            })?;
        let real_age = height.saturating_sub(input_height);
        let decoys: Vec<DecoyOutput> =
            select_aged_decoys(&aged_pool, decoys_per_input, real_age, rng)
                .map_err(|e| match e {
                    DecoySelectionError::InsufficientDecoys { available, .. } => {
                        SendError::Other(format!(
                            "Not enough decoys on chain for a ring of {DEFAULT_RING_SIZE}: need \
                             {decoys_per_input} confirmed (>= {MIN_DECOY_AGE_BLOCKS} blocks old) \
                             per input, found {available}. Wait for more on-chain outputs."
                        ))
                    }
                    other => SendError::Other(format!("decoy selection failed: {other}")),
                })?
                .into_iter()
                .map(|d| DecoyOutput {
                    target_key: d.target_key.clone(),
                    public_key: d.public_key.clone(),
                    amount: d.amount,
                })
                .collect();
        spend_inputs.push(SpendInput {
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
            // The input's stored cluster tags, from its canonical chain row:
            // the signer's outputs inherit them (#1687 retained-weight floor).
            cluster_tags: canonical_tags
                .get(input.target_key.as_str())
                .map(|tags| tags.iter().map(|&(id, w)| (id.to_string(), w)).collect())
                .unwrap_or_default(),
            decoys,
        });
    }

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

        // A send even the alias could not fund is plain insufficient funds.
        let result = plan_send(
            &keys(&v),
            recipient(&v),
            50_000_000_000_000,
            MIN_TX_FEE,
            40,
            &spendable,
            &candidates,
        );
        assert!(matches!(result, Err(SendError::Insufficient)));
    }

    /// An unfundable send must not blame legacy payouts when none is involved,
    /// e.g. an ordinary output listed twice under the same key.
    #[test]
    fn duplicate_ordinary_output_reports_insufficient_not_legacy() {
        let v = vector();
        let mut candidates = chain(&v, &[]);
        candidates.retain(|c| c.output.lottery_context.is_none()); // no payouts
        let source = candidates[0].output.clone();
        let mut spendable: Vec<OwnedOutput> =
            scan(&v, &candidates).into_iter().map(|(o, _)| o).collect();
        assert_eq!(spendable.len(), 1);
        // The same owned output twice: one copy is dropped by the
        // once-per-target rule, so `supported` is shorter than `spendable`.
        spendable.push(spendable[0].clone());
        let canonical = canonical_candidates(&candidates);
        assert_eq!(supported_inputs(&spendable, &canonical).len(), 1);

        let result = plan_send(
            &keys(&v),
            recipient(&v),
            source.amount, // + fee: more than the single source holds
            MIN_TX_FEE,
            40,
            &spendable,
            &candidates,
        );
        assert!(matches!(result, Err(SendError::Insufficient)));
    }
}

/// Ring decoy selection in `plan_send` (#1861): the node/web random-offset age
/// window through the shared `botho_wallet::decoy_selection` helper, not the
/// old chain-ordered `pool[(i*k + j) % len]` window.
#[cfg(test)]
mod decoy_selection_tests {
    use super::*;
    use botho_wallet::decoy_selection::age_similarity_window_width;

    const CHAIN_HEIGHT: u64 = 10_000;
    const REAL_KEY: &str = "ee00000000000000000000000000000000000000000000000000000000000000";
    const REAL_AGE: u64 = 2_000;

    fn keys() -> SignerKeys {
        SignerKeys {
            spend_private_key: "11".repeat(32),
            view_private_key: "22".repeat(32),
            sender_kem_public_key: String::new(),
            seed: "33".repeat(64),
        }
    }

    fn recipient() -> RecipientAddress {
        RecipientAddress {
            spend_public_key: "55".repeat(32),
            view_public_key: "44".repeat(32),
            kem_public_key: String::new(),
        }
    }

    fn row(key: String, height: u64, amount: u64) -> Candidate {
        Candidate {
            output: ChainOutput {
                public_key: format!("{:0>64}", &key[..8]),
                target_key: key,
                amount,
                output_index: 0,
                kem_ciphertext: None,
                lottery_context: None,
            },
            lottery: false,
            height,
            cluster_tags: Vec::new(),
        }
    }

    /// Decoy age encoded in the key, so a ring member's age is readable from
    /// the signer request.
    fn decoy_key(age: u64) -> String {
        format!("{age:016x}{:048x}", 1)
    }

    fn age_of(key: &str) -> u64 {
        u64::from_str_radix(&key[..16], 16).unwrap()
    }

    /// `young` unconfirmed rows (age < 10) plus `confirmed` rows at ages
    /// `20, 30, …`, then the real input (age [`REAL_AGE`]).
    fn chain(confirmed: u64, young: u64) -> (Vec<Candidate>, Vec<OwnedOutput>) {
        let mut rows: Vec<Candidate> = (0..young)
            .map(|i| row(decoy_key(i % 10), CHAIN_HEIGHT - i % 10, 1))
            .collect();
        rows.extend((0..confirmed).map(|i| {
            let age = 20 + i * 10;
            row(decoy_key(age), CHAIN_HEIGHT - age, 1)
        }));
        let real = row(REAL_KEY.into(), CHAIN_HEIGHT - REAL_AGE, 5_000_000_000_000);
        let owned = OwnedOutput {
            target_key: real.output.target_key.clone(),
            public_key: real.output.public_key.clone(),
            amount: real.output.amount,
            subaddress_index: 0,
            output_index: 0,
            kem_ciphertext: None,
            lottery_context: None,
        };
        rows.push(real);
        (rows, vec![owned])
    }

    fn ring_ages(request: &SignRequest) -> Vec<u64> {
        assert_eq!(request.inputs.len(), 1);
        assert_eq!(request.inputs[0].target_key, REAL_KEY);
        request.inputs[0]
            .decoys
            .iter()
            .map(|d| age_of(&d.target_key))
            .collect()
    }

    fn plan(candidates: &[Candidate], spendable: &[OwnedOutput]) -> Result<SignRequest, SendError> {
        plan_send(
            &keys(),
            recipient(),
            1_000_000_000_000,
            MIN_TX_FEE,
            CHAIN_HEIGHT,
            spendable,
            candidates,
        )
    }

    #[test]
    fn plan_send_draws_random_confirmed_decoys_inside_one_age_window() {
        // 400 confirmed rows (ages 20..=4010, every 10 blocks) and 30 young
        // ones. W = 200 at age 2000, so every window holds >= 20 candidates
        // and phase 1 always fills the ring.
        let (candidates, spendable) = chain(400, 30);
        let width = age_similarity_window_width(REAL_AGE);
        assert_eq!(width, 200);

        let old_ring: Vec<String> = {
            let canonical = canonical_candidates(&candidates);
            let pool = decoy_pool(&canonical, &spendable);
            (0..DEFAULT_RING_SIZE - 1)
                .map(|j| pool[j % pool.len()].target_key.clone())
                .collect()
        };

        let mut rings = Vec::new();
        for _ in 0..8 {
            let request = plan(&candidates, &spendable).ok().expect("plan a send");
            let decoys = &request.inputs[0].decoys;
            assert_eq!(decoys.len(), DEFAULT_RING_SIZE - 1);
            let keys: Vec<String> = decoys.iter().map(|d| d.target_key.clone()).collect();
            assert_ne!(keys, old_ring, "chain-ordered window is gone");
            let mut unique = keys.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), keys.len(), "no decoy twice in one ring");

            let ages = ring_ages(&request);
            assert!(
                ages.iter().all(|&a| a >= MIN_DECOY_AGE_BLOCKS),
                "unconfirmed decoy in {ages:?}"
            );
            let lo = ages.iter().copied().chain([REAL_AGE]).min().unwrap();
            let hi = ages.iter().copied().chain([REAL_AGE]).max().unwrap();
            assert!(
                hi - lo <= width,
                "ring {ages:?} does not fit one window of width {width} around {REAL_AGE}"
            );
            rings.push(keys);
        }
        rings.sort();
        rings.dedup();
        assert!(rings.len() > 1, "plan_send rings must be randomized");
    }

    /// Protocol 10.0.0 (#1687): the real input's stored cluster tags reach
    /// the signer exactly (a full u64 id, as a decimal string), so the send's
    /// outputs inherit them.
    #[test]
    fn plan_send_threads_the_input_cluster_tags_to_the_signer() {
        let (mut candidates, spendable) = chain(400, 0);
        let id = u64::MAX - 12;
        candidates.last_mut().unwrap().cluster_tags = vec![(id, 900_000)];
        let rpc_row: crate::rpc::RpcOutput = serde_json::from_value(serde_json::json!({
            "targetKey": REAL_KEY, "publicKey": "00".repeat(32),
            "amountCommitment": "00".repeat(8), "clusterTags": [[id, 900_000]],
        }))
        .unwrap();
        assert_eq!(rpc_row.cluster_tags, vec![(id, 900_000)]);
        let request = plan(&candidates, &spendable).ok().expect("plan a send");
        assert_eq!(
            request.inputs[0].cluster_tags,
            vec![(id.to_string(), 900_000)]
        );
    }

    #[test]
    fn plan_send_refuses_below_nineteen_confirmed_decoys() {
        // 18 confirmed plus plenty of young rows: the old pool-size check
        // would have accepted this.
        let (candidates, spendable) = chain(18, 40);
        let result = plan(&candidates, &spendable);
        match result {
            Err(SendError::Other(m)) => {
                assert!(
                    m.contains("need 19 confirmed (>= 10 blocks old) per input, found 18"),
                    "{m}"
                );
            }
            _ => panic!("expected a decoy refusal"),
        }
    }

    #[test]
    fn plan_send_fills_a_thin_window_by_nearest_age() {
        // 19 confirmed rows far from the real age: the window is empty, so the
        // node/web nearest-age fill takes them all (the CLI would refuse,
        // #1882).
        let (candidates, spendable) = chain(19, 0);
        let request = plan(&candidates, &spendable).ok().expect("plan a send");
        let mut ages = ring_ages(&request);
        ages.sort_unstable();
        assert_eq!(ages, (0..19).map(|i| 20 + i * 10).collect::<Vec<_>>());
    }

    #[test]
    fn rpc_height_reaches_the_candidate() {
        let blocks = vec![serde_json::json!({
            "height": 77,
            "outputs": [{
                "targetKey": REAL_KEY,
                "publicKey": "ab",
                "amountCommitment": hex::encode(5u64.to_le_bytes()),
            }],
        })];
        let rows = crate::rpc::flatten_outputs(&blocks).unwrap();
        let candidate = candidate_from_rpc(rows.into_iter().next().unwrap());
        assert_eq!(candidate.height, 77);
    }
}

/// #2001: the mobile send prices against the node's fee floor.
#[cfg(test)]
mod price_send_tests {
    use super::{price_send, SendError};
    use botho_wallet::fee_estimation::DraftFeeQuote;
    use bth_transaction_clsag::MIN_TX_FEE;
    use std::cell::RefCell;

    /// The node's quote (`bth_cluster_tax::settle_fee` / `resign_fee`) for a
    /// relay floor `floor`.
    fn quote(floor: u64) -> DraftFeeQuote {
        let h = floor / 64;
        DraftFeeQuote {
            minimum_fee: floor.max(MIN_TX_FEE),
            consensus_fee_floor: floor,
            relay_fee_floor: floor,
            settle_fee: (floor + h).max(MIN_TX_FEE),
            resign_fee: (floor + 2 * h).max(MIN_TX_FEE),
        }
    }

    /// A node that refuses a tx below its floor, as the mempool does.
    fn admits(fee: u64, floor: u64) -> bool {
        fee >= floor.max(MIN_TX_FEE)
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn epoch_zero_exit_charge_is_resigned_at_the_node_quote() {
        // stress_node's 10.0.0 case: a ~2e9 floor against the flat MIN_TX_FEE.
        let floor = 2_000_000_074u64;
        // The pre-#2001 mobile send signed once at MIN_TX_FEE: refused.
        assert!(!admits(MIN_TX_FEE, floor));
        let signed = RefCell::new(Vec::new());
        let tx_hex = block_on(price_send(
            MIN_TX_FEE,
            |fee| {
                signed.borrow_mut().push(fee);
                Ok(format!("{fee:016x}"))
            },
            |_tx_hex: String| async move { Ok(quote(floor)) },
        ))
        .ok()
        .unwrap();
        let resign = floor + 2 * (floor / 64);
        assert_eq!(*signed.borrow(), vec![MIN_TX_FEE, resign]);
        let paid = u64::from_str_radix(&tx_hex, 16).unwrap();
        assert_eq!(paid, resign);
        assert!(admits(paid, floor));
    }

    #[test]
    fn background_spend_settles_at_min_tx_fee_in_one_pass() {
        let signed = RefCell::new(0);
        assert!(block_on(price_send(
            MIN_TX_FEE,
            |fee| {
                *signed.borrow_mut() += 1;
                Ok(format!("{fee:016x}"))
            },
            |_tx_hex: String| async move { Ok(quote(3_960)) },
        ))
        .is_ok());
        assert_eq!(*signed.borrow(), 1);
    }

    #[test]
    fn insufficient_funds_on_a_resign_surfaces_as_insufficient() {
        let result = block_on(price_send(
            MIN_TX_FEE,
            |fee| {
                if fee > MIN_TX_FEE {
                    Err(SendError::Insufficient)
                } else {
                    Ok(String::from("00"))
                }
            },
            |_tx_hex: String| async move { Ok(quote(2_000_000_074)) },
        ));
        assert!(matches!(result, Err(SendError::Insufficient)));
    }
}
