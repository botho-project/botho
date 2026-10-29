// Copyright (c) 2024 The Botho Foundation

//! V2 lottery payout detection and one-time key recovery for reserve outputs
//! (#1657, part of the #1286 V2 lottery upgrade).
//!
//! On a V2 (protocol 7.0.0) chain a reserve output can win the lottery. The
//! payout is keyed `P = P_base + T·G`, where `P_base` is the winning output's
//! one-time key (derived at its original index `j`) and `T` is the cumulative
//! tweak. The reserve's ordinary ownership scan tests `P` directly and so can
//! never match a payout. This module scans the base target `P − T·G` at `j`
//! instead, recovers `x = x_base + T` through
//! [`lottery_v2::recover_with_context`], and requires `x·G == P` before the key
//! is used for anything.
//!
//! The wire shape is the frozen #1641 RPC contract: a payout row carries
//! `lottery: true`, `cryptoOutputIndex` (= `lotteryContext.baseIndex` = `j`,
//! NOT the payout outpoint index) and
//! `lotteryContext {version, baseIndex, tweak, ordinal, height}`.
//!
//! ## Trust
//!
//! The context is served by the node and is NOT chain authentication. A forged
//! context (wrong tweak, zero tweak, wrong `baseIndex`, unknown version) can
//! only make the reserve MISS a payout or fail a spend it would otherwise
//! build. Because of the `x·G == P` check it can never make the bridge sign
//! with a key that does not own the output.
//!
//! ## Relationship to the P8 signer-core helpers
//!
//! [`detect_lottery_payout`] and [`recover_onetime_private_key`] deliberately
//! mirror the signatures and rules of the same-named helpers in
//! `bth_wasm_signer::core` (#1645 / PR #1658), so all clients apply one rule.
//! The bridge does not depend on that crate. It is GPL-3.0 while the bridge is
//! Apache-2.0, it builds a `cdylib` with wasm-bindgen and getrandom-js
//! dependencies, and PR #1658 was not merged when this landed. If the helpers
//! move into a shared, license-compatible crate (for example
//! `bth-transaction-clsag`), these bodies can be replaced by re-exports. The
//! callers in [`crate::bth_scan`] only go through the two functions below.

use bth_account_keys::AccountKey;
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_pq::MlKem768KeyPair;
use bth_transaction_clsag::{lottery_v2, TxOutput};
use serde::Deserialize;

/// The V2 lottery payout derivation context, exactly as `chain_getOutputs`
/// serves it on a payout row (`lotteryContext`, frozen #1641 contract):
///
/// ```json
/// { "version": 2, "baseIndex": 3, "tweak": "<64 hex>", "ordinal": 0, "height": 1234 }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LotteryContext {
    /// Context encoding version. Only [`lottery_v2::CONTEXT_VERSION`] (2) is
    /// accepted.
    pub version: u8,
    /// The original derivation index `j` of the lineage's base output.
    pub base_index: u32,
    /// Hex-encoded 32-byte canonical scalar `T` (the cumulative tweak).
    pub tweak: String,
    /// The award's ordinal within its block's payout manifest (informational).
    pub ordinal: u32,
    /// Height the payout was created at (informational).
    pub height: u64,
}

impl LotteryContext {
    /// Validate and convert to the shared [`lottery_v2::Context`].
    ///
    /// Rejects an unknown version, a malformed or non-canonical tweak, and the
    /// zero tweak. A payout always carries a nonzero cumulative tweak, and
    /// `recover_with_context` itself accepts zero, so the check has to live
    /// here: a zero tweak would make the "payout" an alias of its source
    /// output (the #1450 legacy shape).
    pub fn to_context(&self) -> Result<lottery_v2::Context, String> {
        if self.version != lottery_v2::CONTEXT_VERSION {
            return Err(format!(
                "lotteryContext.version: unsupported version {} (expected {})",
                self.version,
                lottery_v2::CONTEXT_VERSION
            ));
        }
        let bytes = hex::decode(&self.tweak)
            .map_err(|e| format!("lotteryContext.tweak: invalid hex: {e}"))?;
        let tweak: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "lotteryContext.tweak: expected 32 bytes".to_string())?;
        if tweak == [0u8; 32] {
            return Err("lotteryContext.tweak: zero tweak is not a V2 payout".to_string());
        }
        let context = lottery_v2::Context {
            base_index: self.base_index,
            tweak,
        };
        // `encode` enforces a canonical scalar encoding of the tweak.
        context
            .encode()
            .map_err(|e| format!("lotteryContext: invalid context: {e:?}"))?;
        Ok(context)
    }
}

/// Ordinary (context-less) ownership check at `output_index`: the unified
/// hybrid scan when an ML-KEM secret is configured, else the classical one.
fn detect_ordinary(
    output: &TxOutput,
    account: &AccountKey,
    kem_keypair: Option<&MlKem768KeyPair>,
    output_index: u32,
) -> Option<u64> {
    match kem_keypair {
        Some(kp) => output.belongs_to_account(account, kp, output_index),
        None => output.belongs_to(account),
    }
}

/// Ordinary one-time private key recovery for a known subaddress. A hybrid
/// output (KEM ciphertext present) needs the ML-KEM secret and its bound
/// `output_index`; a classical output uses the pure view-key path.
fn recover_ordinary(
    output: &TxOutput,
    account: &AccountKey,
    kem_keypair: Option<&MlKem768KeyPair>,
    subaddress_index: u64,
    output_index: u32,
) -> Result<RistrettoPrivate, String> {
    if output.kem_ciphertext.is_some() {
        let kp = kem_keypair
            .ok_or("hybrid reserve output requires an ML-KEM secret to recover its one-time key")?;
        output.recover_spend_key_for(account, kp, subaddress_index, output_index)
    } else {
        output.recover_spend_key(account, subaddress_index)
    }
    .ok_or_else(|| "failed to recover one-time private key".to_string())
}

/// Detect a V2 lottery payout owned by `account`.
///
/// Tests the base target `P − T·G` at `context.baseIndex` with the ordinary
/// ownership rule, then recovers the tweaked key through
/// [`lottery_v2::recover_with_context`], which proves `x·G == P`. Returns the
/// owning subaddress, or `None` when the payout is not ours or the context is
/// malformed or forged.
pub fn detect_lottery_payout(
    output: &TxOutput,
    account: &AccountKey,
    kem_keypair: Option<&MlKem768KeyPair>,
    context: &LotteryContext,
) -> Option<u64> {
    let context = context.to_context().ok()?;
    let mut owner = None;
    lottery_v2::recover_with_context(output, &context, |base, index| {
        let subaddress_index = detect_ordinary(base, account, kem_keypair, index)?;
        owner = Some(subaddress_index);
        recover_ordinary(base, account, kem_keypair, subaddress_index, index).ok()
    })
    .ok()?;
    owner
}

/// Recover an owned reserve output's one-time private key on the single path
/// shared by key-image derivation (spent-status checks) and release signing.
///
/// * No lottery context: the ordinary hybrid-when-present rule at
///   `output_index`.
/// * V2 lottery context: [`lottery_v2::recover_with_context`] recovers the base
///   key at `context.baseIndex` (NOT `output_index`), adds the tweak and checks
///   `x·G == P`. A mismatch is a hard error, so the bridge never signs with a
///   key that does not own the payout.
pub fn recover_onetime_private_key(
    output: &TxOutput,
    account: &AccountKey,
    kem_keypair: Option<&MlKem768KeyPair>,
    subaddress_index: u64,
    output_index: u32,
    lottery_context: Option<&LotteryContext>,
) -> Result<RistrettoPrivate, String> {
    match lottery_context {
        None => recover_ordinary(output, account, kem_keypair, subaddress_index, output_index),
        Some(context) => {
            let context = context.to_context()?;
            lottery_v2::recover_with_context(output, &context, |base, index| {
                recover_ordinary(base, account, kem_keypair, subaddress_index, index).ok()
            })
            .map_err(|e| format!("failed to recover V2 lottery payout key: {e:?}"))
        }
    }
}
