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
//! ## Relationship to the shared helpers (#1667)
//!
//! The payout rules live once, in `bth_transaction_clsag::lottery_v2`, which
//! the bridge already depends on for its transaction types:
//! [`lottery_v2::payout_context`] (version / zero / non-canonical tweak),
//! [`lottery_v2::recover_payout_owner`] (base target `P − T·G`, owner and
//! `x·G == P`) and [`lottery_v2::recover_with_context`]. This module keeps only
//! what is the bridge's own: the wire struct, its hex syntax and error strings,
//! and the reserve's ordinary ownership rule (a hybrid output without an
//! ML-KEM secret is an explicit error here). [`detect_lottery_payout`] and
//! [`recover_onetime_private_key`] keep the signatures of the same-named
//! `bth_wasm_signer::core` helpers (#1645); the bridge does not depend on that
//! crate (it builds a `cdylib` with wasm-bindgen and getrandom-js
//! dependencies). Both crates run the one shared vector
//! `transaction/clsag/tests/fixtures/lottery-v2-wallet.json`. The callers in
//! [`crate::bth_scan`] only go through the two functions below.

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
    /// zero tweak. A payout always carries a nonzero cumulative tweak; a zero
    /// tweak would make the "payout" an alias of its source output (the #1450
    /// legacy shape). `recover_with_context` also fails closed on zero and on
    /// an unknown version since #1667; this check keeps the bridge's own,
    /// descriptive error at the RPC boundary.
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
        // The shared rule (#1667); only the error strings are the bridge's.
        lottery_v2::payout_context(self.version, self.base_index, tweak).map_err(|e| match e {
            lottery_v2::Error::ZeroTweak => {
                "lotteryContext.tweak: zero tweak is not a V2 payout".to_string()
            }
            e => format!("lotteryContext: invalid context: {e:?}"),
        })
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
    let (owner, _) = lottery_v2::recover_payout_owner(output, &context, |base, index| {
        let subaddress_index = detect_ordinary(base, account, kem_keypair, index)?;
        let key = recover_ordinary(base, account, kem_keypair, subaddress_index, index).ok()?;
        Some((subaddress_index, key))
    })
    .ok()?;
    Some(owner)
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

#[cfg(test)]
mod tests {
    //! The shared V2 payout wallet vector (#1667 part 2), committed once in
    //! `transaction/clsag/tests/fixtures/lottery-v2-wallet.json` and run by
    //! every payout consumer.
    use super::*;
    use bth_crypto_ring_signature::KeyImage;
    use serde_json::Value;

    const SHARED: &str =
        include_str!("../../../transaction/clsag/tests/fixtures/lottery-v2-wallet.json");

    fn shared() -> Value {
        serde_json::from_str(SHARED).unwrap()
    }

    fn h32(s: &str) -> [u8; 32] {
        hex::decode(s).unwrap().try_into().unwrap()
    }

    /// The bridge's verdict for a wire context, exact error strings pinned.
    fn verdict(wire: &Value) -> String {
        let context: LotteryContext = serde_json::from_value(wire.clone()).unwrap();
        match context.to_context() {
            Ok(_) => "ok".into(),
            Err(e) if e.starts_with("lotteryContext.version: unsupported version ") => {
                assert!(e.ends_with("(expected 2)"), "{e}");
                "Version".into()
            }
            Err(e) if e == "lotteryContext.tweak: zero tweak is not a V2 payout" => {
                "ZeroTweak".into()
            }
            Err(e) if e == "lotteryContext: invalid context: NonCanonicalTweak" => {
                "NonCanonicalTweak".into()
            }
            Err(e)
                if e.starts_with("lotteryContext.tweak: invalid hex: ")
                    || e == "lotteryContext.tweak: expected 32 bytes" =>
            {
                "Syntax".into()
            }
            Err(e) => panic!("unclassified context error: {e}"),
        }
    }

    #[test]
    fn contexts_match_the_shared_verdicts() {
        for case in shared()["contexts"].as_array().unwrap() {
            assert_eq!(
                verdict(&case["wire"]),
                case["expect"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn recoveries_match_the_shared_vector() {
        let v = shared();
        let a = &v["account"];
        let private =
            |name: &str| RistrettoPrivate::try_from(&h32(a[name].as_str().unwrap())).unwrap();
        let account = AccountKey::new(&private("spendPrivateKey"), &private("viewPrivateKey"));
        let seed: [u8; 64] = hex::decode(a["seed"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let kp = bth_crypto_pq::derive_pq_keys_from_seed(&seed).kem_keypair;
        for case in v["recoveries"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let o = &case["output"];
            let output = TxOutput {
                amount: 1,
                target_key: h32(o["targetKey"].as_str().unwrap()),
                public_key: h32(o["publicKey"].as_str().unwrap()),
                e_memo: None,
                cluster_tags: Default::default(),
                kem_ciphertext: o["kemCiphertext"].as_str().map(|s| hex::decode(s).unwrap()),
            };
            let context: LotteryContext = serde_json::from_value(case["wire"].clone()).unwrap();
            let detected = detect_lottery_payout(&output, &account, Some(&kp), &context);
            match case["expect"].as_object() {
                None => {
                    assert_eq!(detected, None, "{name}");
                    let err = recover_onetime_private_key(
                        &output,
                        &account,
                        Some(&kp),
                        0,
                        0,
                        Some(&context),
                    )
                    .unwrap_err();
                    assert_eq!(
                        err, "failed to recover V2 lottery payout key: Ownership",
                        "{name}"
                    );
                }
                Some(expect) => {
                    let sub = expect["subaddressIndex"].as_u64().unwrap();
                    assert_eq!(detected, Some(sub), "{name}");
                    let key = recover_onetime_private_key(
                        &output,
                        &account,
                        Some(&kp),
                        sub,
                        0,
                        Some(&context),
                    )
                    .unwrap();
                    assert_eq!(
                        hex::encode(KeyImage::from(&key).as_bytes()),
                        expect["keyImage"].as_str().unwrap(),
                        "{name}"
                    );
                }
            }
        }
    }
}
