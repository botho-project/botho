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
//! ## Implementation status (#857)
//!
//! Live-wired: recent-blockhash fetch, `bridge_mint` transaction assembly
//! (PDA derivations, account metas matching the hardened program #850),
//! local Ed25519 signing, `sendTransaction` with idempotent re-broadcast,
//! and `getSignatureStatuses` polling honoring [`SolanaConfig::commitment`].
//! The RPC transport is a lightweight raw JSON-RPC client
//! ([`crate::solana_rpc`]); the heavy `solana-sdk`/`solana-client` stack is
//! deliberately NOT pulled (see that module's docs).
//!
//! ## Custody model (ADR 0002)
//!
//! The #824 Ed25519 threshold attestation is verified here as the OFF-CHAIN
//! federation authorization gate (scheme, order-id binding, `t`-of-`n`
//! threshold): those signatures sign the domain-separated attestation
//! payload, not the Solana transaction message, so they are the federation's
//! proof-of-consent rather than native transaction signatures. The
//! transaction itself is signed by the local mint-authority key
//! (`solana.keypair_file`), which the program checks equals
//! `bridge.mint_authority`. In production that authority is an SPL/Squads
//! multisig whose members are the validators' keys (ADR 0002); the local key
//! is then the member/relayer that assembles the multisig transaction. This
//! mirrors the Ethereum side, where a relayer EOA submits the Safe
//! `execTransaction` carrying the threshold owner signatures. The on-chain
//! per-order marker PDA (`seeds = [b"order", order_id]`, #850) is the
//! exactly-once backstop: a duplicate order id fails at `init` regardless of
//! how many times a transaction is re-broadcast.

use async_trait::async_trait;
use bth_bridge_core::{
    BridgeOrder, Chain, MintAuthorization, SignatureScheme, SolanaCommitment, SolanaConfig,
};
use ed25519_dalek::{Signer as _, SigningKey};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::{ConfirmationStatus, MintError, Minter, PreparedMint};
use crate::solana_rpc::{
    AccountMeta, HttpSolanaRpc, Instruction, LegacyMessage, Pubkey, SignatureState, SolanaRpc,
    Transaction, ALREADY_PROCESSED_MARKER, SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID,
};

/// Seed for the bridge state PDA (`seeds = [b"bridge"]` in the program).
pub const BRIDGE_PDA_SEED: &[u8] = b"bridge";

/// Seed prefix for the per-order replay-guard marker PDA
/// (`seeds = [b"order", order_id]` in the program, #850).
pub const ORDER_MARKER_SEED: &[u8] = b"order";

/// The Associated Token Account program id
/// (`ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`).
pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey = Pubkey([
    140, 151, 37, 143, 78, 36, 137, 241, 187, 61, 16, 41, 20, 142, 13, 131, 11, 90, 19, 153, 218,
    255, 16, 132, 4, 142, 123, 216, 219, 233, 248, 89,
]);

/// Derive the associated token account for `owner` holding `mint`
/// (`seeds = [owner, TOKEN_PROGRAM_ID, mint]` under the ATA program).
pub fn derive_associated_token_account(owner: &Pubkey, mint: &Pubkey) -> Result<Pubkey, MintError> {
    Pubkey::find_program_address(
        &[&owner.0, &TOKEN_PROGRAM_ID.0, &mint.0],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .map(|(pda, _bump)| pda)
    .ok_or_else(|| MintError::Config("could not derive associated token account".to_string()))
}

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

/// Assemble the `bridge_mint` [`Instruction`] with the account metas the
/// hardened program (#850) requires, in its declared order:
/// bridge PDA, per-order marker PDA, mint, recipient ATA, recipient,
/// mint-authority signer, token program, system program.
#[allow(clippy::too_many_arguments)]
pub fn build_bridge_mint_instruction(
    program_id: Pubkey,
    bridge_pda: Pubkey,
    order_marker_pda: Pubkey,
    mint: Pubkey,
    recipient_ata: Pubkey,
    recipient: Pubkey,
    mint_authority: Pubkey,
    amount: u64,
    order_id: [u8; 32],
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::writable(bridge_pda),
            AccountMeta::writable(order_marker_pda),
            AccountMeta::writable(mint),
            AccountMeta::writable(recipient_ata),
            AccountMeta::readonly(recipient),
            // The mint authority both signs and pays rent for the order
            // marker PDA (`payer = mint_authority` in the program), so it is
            // a writable signer.
            AccountMeta::writable_signer(mint_authority),
            AccountMeta::readonly(TOKEN_PROGRAM_ID),
            AccountMeta::readonly(SYSTEM_PROGRAM_ID),
        ],
        data: encode_bridge_mint_instruction_data(amount, order_id),
    }
}

/// Byte offset of the `mint: Pubkey` field inside the `Bridge` account:
/// 8-byte Anchor discriminator + mint_authority(32) + admin_authority(32) +
/// pauser_authority(32).
pub const BRIDGE_MINT_OFFSET: usize = 8 + 32 + 32 + 32;

/// Extract the wBTH mint pubkey from raw `Bridge` account data (#850 layout).
pub fn parse_bridge_mint(data: &[u8]) -> Result<Pubkey, MintError> {
    let end = BRIDGE_MINT_OFFSET + 32;
    if data.len() < end {
        return Err(MintError::Rpc(format!(
            "bridge account too small ({} bytes) to hold the mint field",
            data.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&data[BRIDGE_MINT_OFFSET..end]);
    Ok(Pubkey(arr))
}

/// Solana minting backend. Live-wired per #857 (see module docs).
pub struct SolMinter {
    config: SolanaConfig,
    program_id: Pubkey,
    /// The bridge state PDA (mint authority for the SPL `MintTo`).
    bridge_pda: Pubkey,
    /// Local mint-authority signing key + its pubkey. `None` in watch-only
    /// mode (confirmation polling still works, but `prepare_mint` errors).
    signer: Option<(SigningKey, Pubkey)>,
    /// The RPC transport (abstracted so tests inject a mock).
    rpc: Arc<dyn SolanaRpc>,
}

impl SolMinter {
    /// Build a minter from configuration. Does not perform network I/O.
    pub fn new(config: SolanaConfig) -> Result<Self, MintError> {
        if config.wbth_program.is_empty() {
            return Err(MintError::Config(
                "solana.wbth_program is empty".to_string(),
            ));
        }
        let program_id = Pubkey::from_base58(&config.wbth_program)
            .map_err(|e| MintError::Config(format!("invalid solana.wbth_program: {}", e)))?;

        let (bridge_pda, _bump) = Pubkey::find_program_address(&[BRIDGE_PDA_SEED], &program_id)
            .ok_or_else(|| MintError::Config("could not derive bridge PDA".to_string()))?;

        let signer = match config.keypair_file.as_deref() {
            Some(path) => Some(load_signer(path)?),
            None => None,
        };

        let rpc: Arc<dyn SolanaRpc> = Arc::new(
            HttpSolanaRpc::new(config.rpc_url.clone())
                .map_err(|e| MintError::Config(format!("invalid solana rpc_url: {}", e)))?,
        );

        Ok(Self {
            config,
            program_id,
            bridge_pda,
            signer,
            rpc,
        })
    }

    /// Test constructor: inject a mock RPC and a signing key.
    #[cfg(test)]
    pub fn with_parts(
        config: SolanaConfig,
        rpc: Arc<dyn SolanaRpc>,
        signer: Option<(SigningKey, Pubkey)>,
    ) -> Result<Self, MintError> {
        let program_id = Pubkey::from_base58(&config.wbth_program)
            .map_err(|e| MintError::Config(format!("invalid solana.wbth_program: {}", e)))?;
        let (bridge_pda, _bump) = Pubkey::find_program_address(&[BRIDGE_PDA_SEED], &program_id)
            .ok_or_else(|| MintError::Config("could not derive bridge PDA".to_string()))?;
        Ok(Self {
            config,
            program_id,
            bridge_pda,
            signer,
            rpc,
        })
    }

    /// The commitment level a mint must reach before `Completed`.
    #[allow(dead_code)]
    pub fn required_commitment(&self) -> SolanaCommitment {
        self.config.commitment
    }

    /// Resolve the wBTH mint address from the on-chain `Bridge` account
    /// (its `mint` field is the source of truth, set at `initialize`).
    async fn resolve_mint(&self) -> Result<Pubkey, MintError> {
        let data = self
            .rpc
            .get_account_data(&self.bridge_pda.to_base58(), "finalized")
            .await
            .map_err(MintError::Rpc)?
            .ok_or_else(|| {
                MintError::Config(format!(
                    "bridge account {} not found — is the program initialized?",
                    self.bridge_pda.to_base58()
                ))
            })?;
        parse_bridge_mint(&data)
    }
}

/// Load an Ed25519 signing key from a file. Accepts either a 64-char hex
/// 32-byte seed (parity with `bridge.attestation_ed25519_key_file`) or a
/// Solana CLI JSON keypair array of 64 bytes (`[secret_scalar(32) ||
/// pub(32)]`).
fn load_signer(path: &str) -> Result<(SigningKey, Pubkey), MintError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        MintError::Config(format!("cannot read solana keypair_file {}: {}", path, e))
    })?;
    let trimmed = raw.trim();

    let seed: [u8; 32] = if trimmed.starts_with('[') {
        // Solana CLI JSON array: 64 bytes, the first 32 are the seed.
        let bytes: Vec<u8> = serde_json::from_str(trimmed)
            .map_err(|e| MintError::Config(format!("invalid keypair JSON: {}", e)))?;
        if bytes.len() != 64 {
            return Err(MintError::Config(format!(
                "solana keypair JSON must be 64 bytes, got {}",
                bytes.len()
            )));
        }
        bytes[..32]
            .try_into()
            .map_err(|_| MintError::Config("keypair seed slice error".to_string()))?
    } else {
        let bytes = hex::decode(trimmed)
            .map_err(|e| MintError::Config(format!("invalid keypair hex: {}", e)))?;
        bytes.try_into().map_err(|v: Vec<u8>| {
            MintError::Config(format!("keypair seed must be 32 bytes, got {}", v.len()))
        })?
    };

    let sk = SigningKey::from_bytes(&seed);
    let pubkey = Pubkey(sk.verifying_key().to_bytes());
    Ok((sk, pubkey))
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
        // The #824 Ed25519 threshold attestation is the off-chain federation
        // authorization gate (see module docs): validate scheme, order-id
        // binding, and t-of-n threshold before assembling anything.
        validate_solana_attestation(order, auth)?;

        let (signing_key, mint_authority) = self.signer.as_ref().ok_or_else(|| {
            MintError::Config(
                "no solana.keypair_file configured — cannot sign the mint transaction".to_string(),
            )
        })?;

        let order_id = order.order_id_bytes();
        let amount = order.net_amount();

        // Recipient of the wBTH mint (the order's destination address).
        let recipient = Pubkey::from_base58(&order.dest_address).map_err(|e| {
            MintError::Config(format!(
                "invalid solana dest_address {}: {}",
                order.dest_address, e
            ))
        })?;

        // Resolve the wBTH mint (bridge account is source of truth) and the
        // per-order PDAs + recipient ATA the hardened program (#850) expects.
        let mint = self.resolve_mint().await?;
        let recipient_ata = derive_associated_token_account(&recipient, &mint)?;
        let (order_marker_pda, _bump) =
            Pubkey::find_program_address(&[ORDER_MARKER_SEED, &order_id], &self.program_id)
                .ok_or_else(|| {
                    MintError::Config("could not derive order-marker PDA".to_string())
                })?;

        let instruction = build_bridge_mint_instruction(
            self.program_id,
            self.bridge_pda,
            order_marker_pda,
            mint,
            recipient_ata,
            recipient,
            *mint_authority,
            amount,
            order_id,
        );

        // Fetch a recent blockhash. A resubmit after expiry re-runs prepare
        // and gets a fresh blockhash; the on-chain order-marker PDA keeps the
        // mint itself exactly-once regardless.
        let (recent_blockhash, _last_valid) = self
            .rpc
            .get_latest_blockhash()
            .await
            .map_err(MintError::Rpc)?;

        // The mint authority is the sole required signer (fee payer +
        // authority). Sign the serialized message bytes with Ed25519 — the
        // native Solana transaction signature.
        let message = LegacyMessage::compile(*mint_authority, &[instruction], recent_blockhash);
        let message_bytes = message.serialize();
        let signature = signing_key.sign(&message_bytes).to_bytes();

        let transaction = Transaction {
            signatures: vec![signature],
            message,
        };
        let tx_id = transaction.signature_base58().ok_or_else(|| {
            MintError::Config("assembled transaction has no signature".to_string())
        })?;
        let raw = transaction.serialize();

        info!(
            "prepared Solana mint for order {}: tx {} ({} wBTH to {})",
            order.id, tx_id, amount, order.dest_address
        );

        Ok(PreparedMint { tx_id, raw })
    }

    async fn broadcast(&self, prepared: &PreparedMint) -> Result<(), MintError> {
        match self.rpc.send_transaction(&prepared.raw).await {
            Ok(sig) => {
                debug!("broadcast Solana tx {}", sig);
                Ok(())
            }
            Err(e) if e == ALREADY_PROCESSED_MARKER => {
                // Idempotent re-broadcast: the node already processed this
                // exact transaction. The on-chain per-order marker PDA (#850)
                // guarantees the mint is exactly-once regardless.
                warn!(
                    "Solana tx {} already processed; treating as broadcast",
                    prepared.tx_id
                );
                Ok(())
            }
            Err(e) => Err(MintError::Rpc(format!("sendTransaction failed: {}", e))),
        }
    }

    async fn check_confirmation(
        &self,
        _order: &BridgeOrder,
        dest_tx: &str,
    ) -> Result<ConfirmationStatus, MintError> {
        let state = self
            .rpc
            .get_signature_status(dest_tx)
            .await
            .map_err(MintError::Rpc)?;

        match state {
            SignatureState::Unknown => {
                // Not found. Either still propagating or dropped after its
                // blockhash expired. Fail-safe: report Reorged so the engine
                // rolls the order back to DepositConfirmed and re-submits
                // (with a fresh blockhash). The order-marker PDA makes a
                // re-submit that races an actually-landed tx a no-op.
                Ok(ConfirmationStatus::Reorged)
            }
            SignatureState::Landed {
                err: Some(reason), ..
            } => Ok(ConfirmationStatus::Failed {
                reason: format!("bridge_mint failed on-chain: {}", reason),
            }),
            SignatureState::Landed {
                confirmation_status,
                err: None,
            } => {
                if confirmation_reached(confirmation_status.as_deref(), self.config.commitment) {
                    Ok(ConfirmationStatus::Confirmed)
                } else {
                    Ok(ConfirmationStatus::Pending { confirmations: 1 })
                }
            }
        }
    }
}

/// Whether an observed `confirmationStatus` meets the required commitment.
/// Ordering: processed < confirmed < finalized.
pub fn confirmation_reached(observed: Option<&str>, required: SolanaCommitment) -> bool {
    let rank = |s: &str| match s {
        "processed" => 1,
        "confirmed" => 2,
        "finalized" => 3,
        _ => 0,
    };
    let required_rank = match required {
        SolanaCommitment::Processed => 1,
        SolanaCommitment::Confirmed => 2,
        SolanaCommitment::Finalized => 3,
    };
    observed.map(|o| rank(o) >= required_rank).unwrap_or(false)
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
            safe_nonce: None,
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

    #[test]
    fn test_bridge_mint_instruction_account_metas() {
        let program = Pubkey([5u8; 32]);
        let bridge = Pubkey([1u8; 32]);
        let marker = Pubkey([2u8; 32]);
        let mint = Pubkey([3u8; 32]);
        let ata = Pubkey([4u8; 32]);
        let recipient = Pubkey([6u8; 32]);
        let authority = Pubkey([7u8; 32]);

        let ix = build_bridge_mint_instruction(
            program, bridge, marker, mint, ata, recipient, authority, 100, [9u8; 32],
        );
        assert_eq!(ix.program_id, program);
        // Order + privileges must match the program's BridgeMint accounts.
        assert_eq!(ix.accounts.len(), 8);
        assert_eq!(ix.accounts[0], AccountMeta::writable(bridge));
        assert_eq!(ix.accounts[1], AccountMeta::writable(marker));
        assert_eq!(ix.accounts[2], AccountMeta::writable(mint));
        assert_eq!(ix.accounts[3], AccountMeta::writable(ata));
        assert_eq!(ix.accounts[4], AccountMeta::readonly(recipient));
        assert_eq!(ix.accounts[5], AccountMeta::writable_signer(authority));
        assert_eq!(ix.accounts[6], AccountMeta::readonly(TOKEN_PROGRAM_ID));
        assert_eq!(ix.accounts[7], AccountMeta::readonly(SYSTEM_PROGRAM_ID));
        // Data is the pinned discriminator + borsh args.
        assert_eq!(ix.data, encode_bridge_mint_instruction_data(100, [9u8; 32]));
    }

    #[test]
    fn test_parse_bridge_mint_offset() {
        let mint = [0xABu8; 32];
        let mut data = vec![0u8; crate::mint::solana::BRIDGE_MINT_OFFSET];
        data.extend_from_slice(&mint);
        data.extend_from_slice(&[0u8; 100]); // trailing bridge fields
        assert_eq!(parse_bridge_mint(&data).unwrap(), Pubkey(mint));
        // Too small -> error, never a truncated read.
        assert!(parse_bridge_mint(&[0u8; 10]).is_err());
    }

    #[test]
    fn test_confirmation_reached_commitment_ordering() {
        // Finalized required: only "finalized" passes.
        assert!(confirmation_reached(
            Some("finalized"),
            SolanaCommitment::Finalized
        ));
        assert!(!confirmation_reached(
            Some("confirmed"),
            SolanaCommitment::Finalized
        ));
        // Confirmed required: confirmed and finalized pass.
        assert!(confirmation_reached(
            Some("confirmed"),
            SolanaCommitment::Confirmed
        ));
        assert!(confirmation_reached(
            Some("finalized"),
            SolanaCommitment::Confirmed
        ));
        assert!(!confirmation_reached(
            Some("processed"),
            SolanaCommitment::Confirmed
        ));
        // No status observed never confirms.
        assert!(!confirmation_reached(None, SolanaCommitment::Processed));
    }

    // === Full pipeline against a mocked JSON-RPC transport ===

    use crate::solana_rpc::base64_encode;
    use std::sync::Mutex;

    /// A programmable mock of the Solana RPC surface.
    struct MockRpc {
        /// Raw bytes handed to send_transaction (records re-broadcasts).
        sent: Mutex<Vec<Vec<u8>>>,
        /// If set, send_transaction returns this error string.
        send_error: Mutex<Option<String>>,
        /// The signature status get_signature_status returns.
        status: Mutex<SignatureState>,
        /// The raw bridge-account data get_account_data returns.
        bridge_account: Mutex<Option<Vec<u8>>>,
    }

    impl MockRpc {
        fn with_mint(mint: Pubkey) -> Self {
            let mut data = vec![0u8; BRIDGE_MINT_OFFSET];
            data.extend_from_slice(&mint.0);
            data.extend_from_slice(&[0u8; 100]);
            Self {
                sent: Mutex::new(Vec::new()),
                send_error: Mutex::new(None),
                status: Mutex::new(SignatureState::Unknown),
                bridge_account: Mutex::new(Some(data)),
            }
        }
    }

    #[async_trait]
    impl SolanaRpc for MockRpc {
        async fn get_latest_blockhash(&self) -> Result<([u8; 32], u64), String> {
            Ok(([42u8; 32], 1000))
        }
        async fn send_transaction(&self, raw: &[u8]) -> Result<String, String> {
            if let Some(e) = self.send_error.lock().unwrap().clone() {
                return Err(e);
            }
            self.sent.lock().unwrap().push(raw.to_vec());
            Ok("mocksig".to_string())
        }
        async fn get_signature_status(&self, _signature: &str) -> Result<SignatureState, String> {
            Ok(self.status.lock().unwrap().clone())
        }
        async fn get_account_data(
            &self,
            _address: &str,
            _commitment: &str,
        ) -> Result<Option<Vec<u8>>, String> {
            Ok(self.bridge_account.lock().unwrap().clone())
        }
        async fn get_signatures_for_address(
            &self,
            _address: &str,
            _until: Option<&str>,
            _commitment: &str,
        ) -> Result<Vec<(String, u64)>, String> {
            Ok(vec![])
        }
        async fn get_transaction_logs(
            &self,
            _signature: &str,
            _commitment: &str,
        ) -> Result<Option<(Vec<String>, u64)>, String> {
            Ok(None)
        }
    }

    fn test_config() -> SolanaConfig {
        SolanaConfig {
            rpc_url: "http://localhost:8899".to_string(),
            // A valid base58 32-byte program id.
            wbth_program: Pubkey([8u8; 32]).to_base58(),
            keypair_file: None,
            commitment: SolanaCommitment::Finalized,
            mint_signers: Vec::new(),
            mint_threshold: 0,
        }
    }

    fn test_signer() -> (SigningKey, Pubkey) {
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let pk = Pubkey(sk.verifying_key().to_bytes());
        (sk, pk)
    }

    /// A recipient address that is a valid Ed25519 pubkey (so ATA derivation
    /// and base58 parsing succeed).
    fn recipient_pubkey() -> Pubkey {
        let sk = SigningKey::from_bytes(&[4u8; 32]);
        Pubkey(sk.verifying_key().to_bytes())
    }

    fn sol_order(recipient: Pubkey) -> BridgeOrder {
        BridgeOrder::new_mint(
            Chain::Solana,
            1_000_000_000_000,
            0,
            "bth_addr".to_string(),
            recipient.to_base58(),
        )
    }

    fn ed25519_auth(order: &BridgeOrder) -> MintAuthorization {
        MintAuthorization {
            order_id: order.order_id_bytes(),
            scheme: SignatureScheme::Ed25519,
            threshold: 1,
            signatures: vec![AttestationSignature {
                signer: vec![1u8; 32],
                signature: vec![2u8; 64],
            }],
            safe_nonce: None,
        }
    }

    #[tokio::test]
    async fn test_prepare_mint_assembles_signed_transaction() {
        let mint = Pubkey([0x33u8; 32]);
        let rpc = Arc::new(MockRpc::with_mint(mint));
        let (sk, pk) = test_signer();
        let minter = SolMinter::with_parts(test_config(), rpc.clone(), Some((sk, pk))).unwrap();

        let recipient = recipient_pubkey();
        let order = sol_order(recipient);
        let auth = ed25519_auth(&order);

        let prepared = minter.prepare_mint(&order, &auth).await.unwrap();
        // A real base58 signature id, and non-empty raw bytes.
        assert!(!prepared.tx_id.is_empty());
        assert!(!prepared.raw.is_empty());

        // The raw transaction carries exactly one signature and the
        // bridge_mint instruction data bound to this order.
        let order_id = order.order_id_bytes();
        let ix_data = encode_bridge_mint_instruction_data(order.net_amount(), order_id);
        let hay = &prepared.raw;
        assert!(
            hay.windows(ix_data.len()).any(|w| w == ix_data.as_slice()),
            "raw tx must contain the pinned bridge_mint instruction data"
        );
    }

    #[tokio::test]
    async fn test_prepare_mint_requires_signer() {
        let rpc = Arc::new(MockRpc::with_mint(Pubkey([0x33u8; 32])));
        let minter = SolMinter::with_parts(test_config(), rpc, None).unwrap();
        let recipient = recipient_pubkey();
        let order = sol_order(recipient);
        let auth = ed25519_auth(&order);
        assert!(matches!(
            minter.prepare_mint(&order, &auth).await,
            Err(MintError::Config(_))
        ));
    }

    #[tokio::test]
    async fn test_prepare_mint_rejects_below_threshold_attestation() {
        let rpc = Arc::new(MockRpc::with_mint(Pubkey([0x33u8; 32])));
        let (sk, pk) = test_signer();
        let minter = SolMinter::with_parts(test_config(), rpc, Some((sk, pk))).unwrap();
        let recipient = recipient_pubkey();
        let order = sol_order(recipient);
        let mut auth = ed25519_auth(&order);
        auth.threshold = 2; // only one signature present
        assert!(matches!(
            minter.prepare_mint(&order, &auth).await,
            Err(MintError::Attestation(_))
        ));
    }

    #[tokio::test]
    async fn test_broadcast_and_idempotent_rebroadcast() {
        let rpc = Arc::new(MockRpc::with_mint(Pubkey([0x33u8; 32])));
        let (sk, pk) = test_signer();
        let minter = SolMinter::with_parts(test_config(), rpc.clone(), Some((sk, pk))).unwrap();

        let prepared = PreparedMint {
            tx_id: "mocksig".to_string(),
            raw: vec![1, 2, 3],
        };
        // Normal broadcast records the send.
        minter.broadcast(&prepared).await.unwrap();
        assert_eq!(rpc.sent.lock().unwrap().len(), 1);

        // "Already processed" is treated as a successful (idempotent)
        // broadcast, not an error.
        *rpc.send_error.lock().unwrap() = Some(ALREADY_PROCESSED_MARKER.to_string());
        minter.broadcast(&prepared).await.unwrap();

        // Any other RPC error is retryable (surfaces as Rpc).
        *rpc.send_error.lock().unwrap() = Some("node unreachable".to_string());
        assert!(matches!(
            minter.broadcast(&prepared).await,
            Err(MintError::Rpc(_))
        ));
    }

    #[tokio::test]
    async fn test_check_confirmation_states() {
        let rpc = Arc::new(MockRpc::with_mint(Pubkey([0x33u8; 32])));
        let (sk, pk) = test_signer();
        let minter = SolMinter::with_parts(test_config(), rpc.clone(), Some((sk, pk))).unwrap();
        let order = sol_order(recipient_pubkey());

        // Unknown -> Reorged (fail-safe: retryable, never false success).
        *rpc.status.lock().unwrap() = SignatureState::Unknown;
        assert_eq!(
            minter.check_confirmation(&order, "sig").await.unwrap(),
            ConfirmationStatus::Reorged
        );

        // Landed at confirmed but Finalized required -> still Pending.
        *rpc.status.lock().unwrap() = SignatureState::Landed {
            confirmation_status: Some("confirmed".to_string()),
            err: None,
        };
        assert!(matches!(
            minter.check_confirmation(&order, "sig").await.unwrap(),
            ConfirmationStatus::Pending { .. }
        ));

        // Finalized -> Confirmed.
        *rpc.status.lock().unwrap() = SignatureState::Landed {
            confirmation_status: Some("finalized".to_string()),
            err: None,
        };
        assert_eq!(
            minter.check_confirmation(&order, "sig").await.unwrap(),
            ConfirmationStatus::Confirmed
        );

        // Landed with an on-chain error -> Failed (operator attention).
        *rpc.status.lock().unwrap() = SignatureState::Landed {
            confirmation_status: Some("finalized".to_string()),
            err: Some("Custom(6001)".to_string()),
        };
        assert!(matches!(
            minter.check_confirmation(&order, "sig").await.unwrap(),
            ConfirmationStatus::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn test_send_transaction_base64_wire_is_standard() {
        // Guards the base64 encoding sendTransaction receives.
        assert_eq!(base64_encode(&[0, 0, 0]), "AAAA");
    }
}
