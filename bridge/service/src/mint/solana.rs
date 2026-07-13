// Copyright (c) 2024 The Botho Foundation

//! Solana wBTH minting (Anchor program `wbth_bridge`).
//!
//! Per ADR 0002, Solana mint authorizations are signed natively by the
//! validators' Ed25519 keys — no secp256k1 detour is needed. The on-chain
//! program (`contracts/solana/programs/wbth`) exposes
//! `bridge_mint(amount: u64, order_id: [u8; 32])` gated on the `Bridge` PDA
//! (`seeds = [b"bridge"]`) authority. NOTE: the program currently names the
//! second argument `bth_tx_hash`; #826 renames it to `order_id` and adds the
//! duplicate-order guard. The Anchor discriminator is unchanged by that
//! rename (it hashes the instruction NAME, `bridge_mint`).
//!
//! ## Implementation status
//!
//! The deterministic, chain-agnostic pieces are implemented and unit-tested
//! here: attestation validation (Ed25519 scheme, order-id binding,
//! threshold) and Anchor instruction-data construction (discriminator +
//! borsh args), so the exact bytes that will land on-chain are pinned.
//!
//! The RPC-dependent bodies (recent-blockhash fetch, transaction assembly
//! and Ed25519 multisig signing per #824, `send_transaction`,
//! `get_signature_statuses` polling to the configured commitment) are
//! `TODO(#824/#857)` stubs returning [`MintError::NotImplemented`]. They
//! require the `solana-client`/`anchor-client` dependency stack, which is
//! deferred to keep this workspace's dependency tree (curve25519-dalek v4,
//! zeroize, etc.) unconflicted until the Solana test harness (#857) lands
//! and can validate the wiring end to end.

use async_trait::async_trait;
use bth_bridge_core::{
    BridgeOrder, Chain, MintAuthorization, SignatureScheme, SolanaCommitment, SolanaConfig,
};
use sha2::{Digest, Sha256};

use super::{ConfirmationStatus, MintError, Minter, PreparedMint};

/// Seed for the bridge state PDA (`seeds = [b"bridge"]` in the program).
/// Consumed by the #857 transaction-assembly work (PDA derivation).
#[allow(dead_code)]
pub const BRIDGE_PDA_SEED: &[u8] = b"bridge";

/// Compute the Anchor instruction discriminator for a global instruction:
/// the first 8 bytes of `sha256("global:<name>")`.
pub fn anchor_discriminator(instruction_name: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(b"global:");
    hasher.update(instruction_name.as_bytes());
    let digest = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&digest[..8]);
    disc
}

/// Build the `bridge_mint(amount, order_id)` instruction data:
/// 8-byte Anchor discriminator, then the borsh-encoded args
/// (`u64` little-endian amount, raw 32-byte order id).
pub fn encode_bridge_mint_instruction_data(amount: u64, order_id: [u8; 32]) -> Vec<u8> {
    let mut data = Vec::with_capacity(8 + 8 + 32);
    data.extend_from_slice(&anchor_discriminator("bridge_mint"));
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(&order_id);
    data
}

/// Validate that an attestation authorizes a Solana mint for `order`.
pub fn validate_solana_attestation(
    order: &BridgeOrder,
    auth: &MintAuthorization,
) -> Result<(), MintError> {
    if auth.scheme != SignatureScheme::Ed25519 {
        return Err(MintError::Attestation(
            "Solana mint requires Ed25519 attestation signatures".to_string(),
        ));
    }
    if auth.order_id != order.order_id_bytes() {
        return Err(MintError::Attestation(
            "attestation order id does not match order".to_string(),
        ));
    }
    if !auth.meets_threshold() {
        return Err(MintError::Attestation(format!(
            "attestation has {} signature(s), threshold is {}",
            auth.signatures.len(),
            auth.threshold
        )));
    }
    for sig in &auth.signatures {
        if sig.signer.len() != 32 {
            return Err(MintError::Attestation(format!(
                "ed25519 signer must be a 32-byte pubkey, got {} bytes",
                sig.signer.len()
            )));
        }
        if sig.signature.len() != 64 {
            return Err(MintError::Attestation(format!(
                "ed25519 signature must be 64 bytes, got {}",
                sig.signature.len()
            )));
        }
    }
    Ok(())
}

/// Solana minting backend.
///
/// See the module docs: instruction construction and attestation validation
/// are live; RPC submission/confirmation are `TODO(#824/#857)` stubs.
pub struct SolMinter {
    #[allow(dead_code)]
    config: SolanaConfig,
}

impl SolMinter {
    /// Build a minter from configuration. Does not perform network I/O.
    pub fn new(config: SolanaConfig) -> Result<Self, MintError> {
        if config.wbth_program.is_empty() {
            return Err(MintError::Config(
                "solana.wbth_program is empty".to_string(),
            ));
        }
        Ok(Self { config })
    }

    /// The commitment level a mint must reach before `Completed`.
    #[allow(dead_code)]
    pub fn required_commitment(&self) -> SolanaCommitment {
        self.config.commitment
    }
}

#[async_trait]
impl Minter for SolMinter {
    fn chain(&self) -> Chain {
        Chain::Solana
    }

    async fn prepare_mint(
        &self,
        order: &BridgeOrder,
        auth: &MintAuthorization,
    ) -> Result<PreparedMint, MintError> {
        validate_solana_attestation(order, auth)?;

        // The instruction bytes that will land on-chain are already pinned
        // (and unit-tested) by encode_bridge_mint_instruction_data.
        let _instruction_data =
            encode_bridge_mint_instruction_data(order.net_amount(), order.order_id_bytes());

        // TODO(#824/#857): assemble and sign the transaction. Requires the
        // solana-sdk/anchor-client stack:
        //   1. Derive the Bridge PDA (seeds=[b"bridge"]) and the recipient's associated
        //      token account; prepend a create-idempotent-ATA instruction
        //      (spl-associated-token-account).
        //   2. Fetch a recent blockhash (or use a durable nonce account so the signed
        //      tx never expires across retries).
        //   3. Sign with the #824 Ed25519 multisig authority (validator threshold;
        //      multisig authority PDA / Squads-style).
        //   4. The first signature is the transaction id (base58) — return it in
        //      PreparedMint.tx_id with the serialized tx in raw.
        Err(MintError::NotImplemented(
            "Solana transaction assembly/signing pending #824 (attestation) and #857 \
             (solana-test-validator harness)"
                .to_string(),
        ))
    }

    async fn broadcast(&self, _prepared: &PreparedMint) -> Result<(), MintError> {
        // TODO(#857): sendTransaction(base64(raw)) with
        // skip_preflight=false; treat "AlreadyProcessed" as success
        // (idempotent re-broadcast). The on-chain order-id guard (#826) and
        // the blockhash/durable-nonce guarantee a resubmit cannot
        // double-mint.
        Err(MintError::NotImplemented(
            "Solana send_transaction pending #857".to_string(),
        ))
    }

    async fn check_confirmation(
        &self,
        _order: &BridgeOrder,
        _dest_tx: &str,
    ) -> Result<ConfirmationStatus, MintError> {
        // TODO(#857): poll getSignatureStatuses(dest_tx) until the
        // configured SolanaCommitment (default Finalized); confirm the
        // BridgeMintEvent in the tx meta/logs, then Confirmed. A dropped /
        // expired-blockhash signature (status None after the blockhash's
        // last_valid_block_height) maps to Reorged so the engine rolls the
        // order back to DepositConfirmed and re-submits.
        Err(MintError::NotImplemented(
            "Solana get_signature_statuses polling pending #857".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_bridge_core::AttestationSignature;

    #[test]
    fn test_anchor_discriminator_known_vector() {
        // sha256("global:bridge_mint")[..8] — must match what Anchor
        // computes for the deployed program. Pinned so a silent rename of
        // the instruction breaks the build's tests, not mainnet.
        let disc = anchor_discriminator("bridge_mint");
        let mut hasher = Sha256::new();
        hasher.update(b"global:bridge_mint");
        assert_eq!(disc, hasher.finalize()[..8]);
        // The #826 rename of the ARGUMENT (bth_tx_hash -> order_id) does
        // not change the discriminator; renaming the INSTRUCTION would.
        assert_ne!(disc, anchor_discriminator("bridgeMint"));
    }

    #[test]
    fn test_bridge_mint_instruction_data_layout() {
        let order_id = [7u8; 32];
        let data = encode_bridge_mint_instruction_data(999_000_000_000, order_id);

        assert_eq!(data.len(), 8 + 8 + 32);
        assert_eq!(&data[..8], &anchor_discriminator("bridge_mint"));
        assert_eq!(&data[8..16], &999_000_000_000u64.to_le_bytes());
        assert_eq!(&data[16..48], &order_id);
    }

    fn order_and_auth() -> (BridgeOrder, MintAuthorization) {
        let order = BridgeOrder::new_mint(
            Chain::Solana,
            1_000_000_000_000,
            0,
            "bth_addr".to_string(),
            "So11111111111111111111111111111111111111112".to_string(),
        );
        let auth = MintAuthorization {
            order_id: order.order_id_bytes(),
            scheme: SignatureScheme::Ed25519,
            threshold: 1,
            signatures: vec![AttestationSignature {
                signer: vec![1u8; 32],
                signature: vec![2u8; 64],
            }],
        };
        (order, auth)
    }

    #[test]
    fn test_attestation_validation() {
        let (order, auth) = order_and_auth();
        assert!(validate_solana_attestation(&order, &auth).is_ok());

        // Wrong scheme.
        let mut bad = auth.clone();
        bad.scheme = SignatureScheme::Secp256k1;
        assert!(validate_solana_attestation(&order, &bad).is_err());

        // Bound to a different order id (replay from another order).
        let mut bad = auth.clone();
        bad.order_id = [0u8; 32];
        assert!(validate_solana_attestation(&order, &bad).is_err());

        // Below threshold.
        let mut bad = auth.clone();
        bad.threshold = 2;
        assert!(validate_solana_attestation(&order, &bad).is_err());
    }

    #[test]
    fn test_same_order_id_binding_as_ethereum() {
        // Both chains must bind the SAME 32-byte id for one order.
        let (order, _) = order_and_auth();
        let data = encode_bridge_mint_instruction_data(order.net_amount(), order.order_id_bytes());
        assert_eq!(&data[16..48], order.order_id_bytes().as_slice());
        assert_eq!(
            order.order_id_bytes(),
            bth_bridge_core::derive_order_id(&order.id)
        );
    }
}
