//! Transaction Building and Signing
//!
//! Handles local transaction construction and signing for the thin wallet.
//! All signing happens locally - private keys never leave the wallet.

use anyhow::{anyhow, Result};
#[cfg(feature = "pq")]
use bth_account_keys::QuantumSafeAccountKey;
use bth_account_keys::{AccountKey, PublicAddress};
use bth_crypto_keys::{RistrettoPrivate, RistrettoPublic};
use bth_crypto_ring_signature::onetime_keys::{
    recover_onetime_private_key, recover_public_subaddress_spend_key,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    fee_estimation::StoredTags,
    keys::WalletKeys,
    rpc_pool::{BlockOutputs, RpcPool},
};

#[cfg(feature = "pq")]
use bth_crypto_pq::MlKem768Ciphertext;
#[cfg(feature = "pq")]
use bth_crypto_ring_signature::pq_onetime_keys::check_pq_output_ownership;

/// Picocredits per CAD
pub const PICOCREDITS_PER_CAD: u64 = 1_000_000_000_000;

/// Minimum transaction fee
pub const MIN_FEE: u64 = 1_000_000; // 0.000001 CAD

/// Dust threshold - minimum output amount in picocredits.
/// Outputs below this value are rejected to prevent unspendable UTXOs.
/// Set to 1 microcredit (0.000001 CAD = 1_000_000 picocredits).
/// Change outputs below this threshold are added to the transaction fee
/// instead.
pub const DUST_THRESHOLD: u64 = 1_000_000;

/// A UTXO owned by this wallet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedUtxo {
    /// Transaction hash that created this output
    pub tx_hash: [u8; 32],
    /// Output index in the transaction
    pub output_index: u32,
    /// Amount in picocredits
    pub amount: u64,
    /// Block height where created
    pub created_at: u64,
    /// One-time target key (stealth spend key) - needed for signing
    pub target_key: [u8; 32],
    /// Ephemeral public key (for DH derivation) - needed for key recovery
    pub public_key: [u8; 32],
    /// Subaddress index that owns this output
    pub subaddress_index: u64,
    /// Cluster tag attribution for progressive fee calculation.
    /// Tracks wealth cluster attribution from the sender's history.
    /// Optional for backwards compatibility with older wallet files.
    #[serde(default)]
    pub cluster_tags: Option<StoredTags>,
}

impl OwnedUtxo {
    /// Create a UTXO identifier
    pub fn id(&self) -> UtxoId {
        UtxoId {
            tx_hash: self.tx_hash,
            output_index: self.output_index,
        }
    }

    /// Get cluster tags for fee estimation, returning empty tags if not set.
    pub fn tags(&self) -> StoredTags {
        self.cluster_tags.clone().unwrap_or_default()
    }

    /// Recover the one-time private key needed to spend this output
    ///
    /// Uses the stealth address protocol to derive the spend key from:
    /// - The view private key (for DH shared secret)
    /// - The subaddress spend private key
    /// - The output's public key (ephemeral DH key)
    pub fn recover_spend_key(&self, account_key: &AccountKey) -> Option<RistrettoPrivate> {
        let public_key = RistrettoPublic::try_from(&self.public_key[..]).ok()?;
        let view_private = account_key.view_private_key();
        let subaddress_spend_private = account_key.subaddress_spend_private(self.subaddress_index);

        Some(recover_onetime_private_key(
            &public_key,
            view_private,
            &subaddress_spend_private,
        ))
    }

    /// Derive a PQ shared secret for this output (bridge mode)
    ///
    /// For classical UTXOs that don't have PQ ciphertexts, we derive a
    /// deterministic shared secret from the output's key material. This
    /// provides forward secrecy: new PQ outputs will have proper ML-KEM.
    #[cfg(feature = "pq")]
    pub fn pq_bridge_secret(&self, view_private_bytes: &[u8; 32]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"botho-pq-bridge-v1");
        hasher.update(self.target_key);
        hasher.update(self.public_key);
        hasher.update(view_private_bytes);
        hasher.finalize().into()
    }
}

/// UTXO identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UtxoId {
    pub tx_hash: [u8; 32],
    pub output_index: u32,
}

/// A quantum-private UTXO owned by this wallet (ML-KEM encapsulated)
///
/// These outputs use post-quantum cryptography for stealth address
/// derivation, protecting against "harvest now, decrypt later" attacks.
#[cfg(feature = "pq")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PqOwnedUtxo {
    /// Transaction hash that created this output
    pub tx_hash: [u8; 32],
    /// Output index in the transaction
    pub output_index: u32,
    /// Amount in picocredits
    pub amount: u64,
    /// Block height where created
    pub created_at: u64,
    /// ML-KEM-768 ciphertext (1088 bytes) for key decapsulation
    pub kem_ciphertext: Vec<u8>,
    /// One-time target key (Ristretto point)
    pub target_key: [u8; 32],
    /// Subaddress index that owns this output
    pub subaddress_index: u64,
}

#[cfg(feature = "pq")]
impl PqOwnedUtxo {
    /// Create a UTXO identifier
    pub fn id(&self) -> UtxoId {
        UtxoId {
            tx_hash: self.tx_hash,
            output_index: self.output_index,
        }
    }

    /// Recover the one-time private key needed to spend this PQ output
    ///
    /// Uses ML-KEM decapsulation to derive the shared secret, then
    /// computes the one-time private key using the PQ stealth address protocol.
    pub fn recover_spend_key(
        &self,
        pq_account_key: &QuantumSafeAccountKey,
    ) -> Option<RistrettoPrivate> {
        use bth_crypto_ring_signature::pq_onetime_keys::recover_pq_onetime_private_key;

        let ciphertext = MlKem768Ciphertext::from_bytes(self.kem_ciphertext.as_slice()).ok()?;
        let kem_keypair = pq_account_key.pq_kem_keypair();
        let subaddress_spend_private = pq_account_key
            .classical()
            .subaddress_spend_private(self.subaddress_index);

        recover_pq_onetime_private_key(
            kem_keypair,
            &ciphertext,
            &subaddress_spend_private,
            self.output_index,
        )
        .ok()
    }
}

/// A transaction output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxOutput {
    pub amount: u64,
    pub recipient_view_key: [u8; 32],
    pub recipient_spend_key: [u8; 32],
    pub output_public_key: [u8; 32],
}

impl TxOutput {
    /// Create a new output for a recipient
    pub fn new(amount: u64, recipient: &PublicAddress) -> Self {
        let view_key = recipient.view_public_key().to_bytes();
        let spend_key = recipient.spend_public_key().to_bytes();

        // Generate one-time output key using cryptographically secure RNG
        let mut random_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut random_bytes);

        let mut hasher = Sha256::new();
        hasher.update(view_key);
        hasher.update(spend_key);
        hasher.update(amount.to_le_bytes());
        hasher.update(random_bytes);
        let output_key: [u8; 32] = hasher.finalize().into();

        Self {
            amount,
            recipient_view_key: view_key,
            recipient_spend_key: spend_key,
            output_public_key: output_key,
        }
    }
}

/// A transaction input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxInput {
    pub tx_hash: [u8; 32],
    pub output_index: u32,
    pub signature: Vec<u8>,
}

/// A complete transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub version: u32,
    pub inputs: Vec<TxInput>,
    pub outputs: Vec<TxOutput>,
    pub fee: u64,
    pub created_at_height: u64,
}

impl Transaction {
    /// Create a new unsigned transaction
    pub fn new(
        inputs: Vec<TxInput>,
        outputs: Vec<TxOutput>,
        fee: u64,
        created_at_height: u64,
    ) -> Self {
        Self {
            version: 1,
            inputs,
            outputs,
            fee,
            created_at_height,
        }
    }

    /// Compute the signing hash (message to be signed)
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"botho-tx-v1");
        hasher.update(self.version.to_le_bytes());

        for input in &self.inputs {
            hasher.update(input.tx_hash);
            hasher.update(input.output_index.to_le_bytes());
        }

        for output in &self.outputs {
            hasher.update(output.amount.to_le_bytes());
            hasher.update(output.recipient_view_key);
            hasher.update(output.recipient_spend_key);
            hasher.update(output.output_public_key);
        }

        hasher.update(self.fee.to_le_bytes());
        hasher.update(self.created_at_height.to_le_bytes());
        hasher.finalize().into()
    }

    /// Compute the transaction hash
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.version.to_le_bytes());

        for input in &self.inputs {
            hasher.update(input.tx_hash);
            hasher.update(input.output_index.to_le_bytes());
        }

        for output in &self.outputs {
            hasher.update(output.amount.to_le_bytes());
            hasher.update(output.recipient_view_key);
            hasher.update(output.recipient_spend_key);
            hasher.update(output.output_public_key);
        }

        hasher.update(self.fee.to_le_bytes());
        hasher.update(self.created_at_height.to_le_bytes());
        hasher.finalize().into()
    }

    /// Serialize to hex for submission
    pub fn to_hex(&self) -> String {
        let bytes = bincode::serialize(self).expect("Serialization should not fail");
        hex::encode(bytes)
    }

    /// Total output amount
    pub fn total_output(&self) -> u64 {
        self.outputs.iter().map(|o| o.amount).sum()
    }
}

/// Transaction builder for creating and signing transactions
pub struct TransactionBuilder {
    keys: WalletKeys,
    utxos: Vec<OwnedUtxo>,
    sync_height: u64,
}

impl TransactionBuilder {
    /// Create a new transaction builder
    pub fn new(keys: WalletKeys, utxos: Vec<OwnedUtxo>, sync_height: u64) -> Self {
        Self {
            keys,
            utxos,
            sync_height,
        }
    }

    /// Get total balance from UTXOs
    pub fn balance(&self) -> u64 {
        self.utxos.iter().map(|u| u.amount).sum()
    }

    /// Build and sign a transaction
    ///
    /// If the change amount is below `DUST_THRESHOLD`, it is added to the fee
    /// instead of creating an unspendable output.
    ///
    /// Returns the transaction and the actual fee (which may be higher than
    /// the requested fee if dust change was absorbed).
    pub fn build_transfer(
        &self,
        recipient: &PublicAddress,
        amount: u64,
        fee: u64,
    ) -> Result<Transaction> {
        // Validate amount
        if amount == 0 {
            return Err(anyhow!("Amount must be greater than 0"));
        }

        // Validate amount is above dust threshold
        if amount < DUST_THRESHOLD {
            return Err(anyhow!(
                "Amount {} is below dust threshold of {} picocredits",
                amount,
                DUST_THRESHOLD
            ));
        }

        let total_needed = amount
            .checked_add(fee)
            .ok_or_else(|| anyhow!("Amount overflow"))?;

        // Select UTXOs
        let (selected, total_selected) = self.select_utxos(total_needed)?;

        // Calculate change
        let change = total_selected
            .checked_sub(total_needed)
            .ok_or_else(|| anyhow!("Insufficient funds"))?;

        // Create inputs (unsigned)
        let inputs: Vec<TxInput> = selected
            .iter()
            .map(|utxo| TxInput {
                tx_hash: utxo.tx_hash,
                output_index: utxo.output_index,
                signature: vec![], // Will be filled in after signing
            })
            .collect();

        // Create outputs
        let mut outputs = vec![TxOutput::new(amount, recipient)];

        // Handle change: if above dust threshold, create change output
        // Otherwise, add dust to fee (prevents unspendable UTXOs)
        let actual_fee = if change >= DUST_THRESHOLD {
            outputs.push(TxOutput::new(change, &self.keys.public_address()));
            fee
        } else {
            // Dust change is absorbed into fee
            fee + change
        };

        // Create transaction
        let mut tx = Transaction::new(inputs, outputs, actual_fee, self.sync_height);

        // Sign all inputs
        self.sign_transaction(&mut tx)?;

        Ok(tx)
    }

    /// Select UTXOs using largest-first algorithm
    fn select_utxos(&self, target: u64) -> Result<(Vec<OwnedUtxo>, u64)> {
        if self.utxos.is_empty() {
            return Err(anyhow!("No UTXOs available"));
        }

        // Sort by amount descending
        let mut sorted: Vec<_> = self.utxos.clone();
        sorted.sort_by(|a, b| b.amount.cmp(&a.amount));

        let mut selected = Vec::new();
        let mut total = 0u64;

        for utxo in sorted {
            if total >= target {
                break;
            }
            total = total.saturating_add(utxo.amount);
            selected.push(utxo);
        }

        if total < target {
            return Err(anyhow!(
                "Insufficient funds: have {} picocredits, need {}",
                total,
                target
            ));
        }

        Ok((selected, total))
    }

    /// Sign all inputs of a transaction
    fn sign_transaction(&self, tx: &mut Transaction) -> Result<()> {
        let signing_hash = tx.signing_hash();

        for input in &mut tx.inputs {
            // Sign with our spend key
            let signature = self.keys.sign(b"botho-tx-v1", &signing_hash);
            input.signature = signature;
        }

        Ok(())
    }
}

/// Wallet scanner for finding owned outputs using stealth address detection
pub struct WalletScanner<'a> {
    account_key: &'a AccountKey,
}

impl<'a> WalletScanner<'a> {
    /// Create a new scanner for the given wallet keys
    pub fn new(keys: &'a WalletKeys) -> Self {
        Self {
            account_key: keys.account_key(),
        }
    }

    /// Scan block outputs for UTXOs belonging to this wallet
    ///
    /// Uses proper stealth address detection:
    /// 1. Parse target_key and public_key from output
    /// 2. Recover the expected spend public key using view private key
    /// 3. Check against known subaddresses
    pub fn scan_outputs(&self, block_outputs: &[BlockOutputs]) -> Vec<OwnedUtxo> {
        let mut owned = Vec::new();

        for block in block_outputs {
            for output in &block.outputs {
                // Parse all required keys
                let target_key = match Self::parse_key(&output.target_key) {
                    Some(k) => k,
                    None => continue,
                };
                let public_key = match Self::parse_key(&output.public_key) {
                    Some(k) => k,
                    None => continue,
                };
                let tx_hash = match Self::parse_hash(&output.tx_hash) {
                    Some(h) => h,
                    None => continue,
                };

                // Parse amount (stored as LE bytes in hex)
                let amount = Self::parse_amount(&output.amount_commitment);

                // Check if this output belongs to us using stealth address detection
                if let Some(subaddress_index) = self.check_ownership(&target_key, &public_key) {
                    owned.push(OwnedUtxo {
                        tx_hash,
                        output_index: output.output_index,
                        amount,
                        created_at: block.height,
                        target_key,
                        public_key,
                        subaddress_index,
                        // Tags are not available from blockchain scanning - they would
                        // need to come from the sender or an external privacy service.
                        // For now, assume anonymous (no cluster attribution).
                        cluster_tags: None,
                    });
                }
            }
        }

        owned
    }

    /// Check if an output belongs to this wallet using stealth address
    /// derivation
    ///
    /// Returns `Some(subaddress_index)` if the output belongs to us, `None`
    /// otherwise.
    fn check_ownership(&self, target_key: &[u8; 32], public_key: &[u8; 32]) -> Option<u64> {
        let view_private = self.account_key.view_private_key();

        // Parse keys
        let public_key_point = RistrettoPublic::try_from(&public_key[..]).ok()?;
        let target_key_point = RistrettoPublic::try_from(&target_key[..]).ok()?;

        // Recover the spend public key that would correspond to this output
        let recovered_spend_key =
            recover_public_subaddress_spend_key(view_private, &target_key_point, &public_key_point);

        // Check against default subaddress (index 0)
        let default_subaddr = self.account_key.default_subaddress();
        let default_spend = default_subaddr.spend_public_key();
        if recovered_spend_key.to_bytes() == default_spend.to_bytes() {
            return Some(0);
        }

        // Check against change subaddress (index 1)
        let change_subaddr = self.account_key.change_subaddress();
        let change_spend = change_subaddr.spend_public_key();
        if recovered_spend_key.to_bytes() == change_spend.to_bytes() {
            return Some(1);
        }

        None
    }

    /// Parse a 32-byte key from hex string
    fn parse_key(hex_str: &str) -> Option<[u8; 32]> {
        let bytes = hex::decode(hex_str).ok()?;
        if bytes.len() >= 32 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes[..32]);
            Some(key)
        } else {
            None
        }
    }

    /// Parse a 32-byte hash from hex string
    fn parse_hash(hex_str: &str) -> Option<[u8; 32]> {
        Self::parse_key(hex_str)
    }

    /// Parse amount from commitment hex (stored as LE bytes)
    fn parse_amount(hex_str: &str) -> u64 {
        if let Ok(bytes) = hex::decode(hex_str) {
            if bytes.len() >= 8 {
                u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0; 8]))
            } else {
                0
            }
        } else {
            0
        }
    }
}

/// Quantum-private wallet scanner for finding PQ outputs using ML-KEM
/// decapsulation
#[cfg(feature = "pq")]
pub struct PqWalletScanner<'a> {
    pq_account_key: QuantumSafeAccountKey,
    keys: &'a WalletKeys,
}

#[cfg(feature = "pq")]
impl<'a> PqWalletScanner<'a> {
    /// Create a new PQ scanner for the given wallet keys
    pub fn new(keys: &'a WalletKeys) -> Self {
        Self {
            pq_account_key: keys.pq_account_key(),
            keys,
        }
    }

    /// Scan block outputs for quantum-private UTXOs belonging to this wallet
    ///
    /// Uses ML-KEM decapsulation to check ownership:
    /// 1. Parse ciphertext and target_key from output
    /// 2. Decapsulate shared secret using our KEM keypair
    /// 3. Verify target key matches expected value
    pub fn scan_pq_outputs(&self, block_outputs: &[BlockOutputs]) -> Vec<PqOwnedUtxo> {
        let mut owned = Vec::new();

        for block in block_outputs {
            for output in &block.outputs {
                // Skip non-PQ outputs
                if !output.is_pq_output {
                    continue;
                }

                // Parse PQ ciphertext
                let ciphertext_bytes = match output
                    .pq_ciphertext
                    .as_ref()
                    .and_then(|s| hex::decode(s).ok())
                {
                    Some(bytes) => bytes,
                    None => continue,
                };

                let ciphertext = match MlKem768Ciphertext::from_bytes(ciphertext_bytes.as_slice()) {
                    Ok(ct) => ct,
                    Err(_) => continue,
                };

                // Parse target key
                let target_key = match WalletScanner::parse_key(&output.target_key) {
                    Some(k) => k,
                    None => continue,
                };

                let target_key_point = match RistrettoPublic::try_from(&target_key[..]) {
                    Ok(pk) => pk,
                    Err(_) => continue,
                };

                let tx_hash = match WalletScanner::parse_key(&output.tx_hash) {
                    Some(h) => h,
                    None => continue,
                };

                // Parse amount
                let amount = WalletScanner::parse_amount(&output.amount_commitment);

                // Check ownership against subaddresses
                if let Some(subaddress_index) =
                    self.check_pq_ownership(&ciphertext, &target_key_point, output.output_index)
                {
                    owned.push(PqOwnedUtxo {
                        tx_hash,
                        output_index: output.output_index,
                        amount,
                        created_at: block.height,
                        kem_ciphertext: ciphertext_bytes,
                        target_key,
                        subaddress_index,
                    });
                }
            }
        }

        owned
    }

    /// Check if a PQ output belongs to this wallet
    fn check_pq_ownership(
        &self,
        ciphertext: &MlKem768Ciphertext,
        target_key: &RistrettoPublic,
        output_index: u32,
    ) -> Option<u64> {
        let kem_keypair = self.pq_account_key.pq_kem_keypair();
        let account_key = self.keys.account_key();

        // Check against default subaddress (index 0)
        let default_subaddr = account_key.default_subaddress();
        let default_spend = default_subaddr.spend_public_key();
        if check_pq_output_ownership(
            kem_keypair,
            default_spend,
            ciphertext,
            target_key,
            output_index,
        ) {
            return Some(0);
        }

        // Check against change subaddress (index 1)
        let change_subaddr = account_key.change_subaddress();
        let change_spend = change_subaddr.spend_public_key();
        if check_pq_output_ownership(
            kem_keypair,
            change_spend,
            ciphertext,
            target_key,
            output_index,
        ) {
            return Some(1);
        }

        None
    }
}

/// Sync wallet UTXOs with the network
pub async fn sync_wallet(
    rpc: &mut RpcPool,
    keys: &WalletKeys,
    from_height: u64,
) -> Result<(Vec<OwnedUtxo>, u64)> {
    // Get current chain height
    let chain_info = rpc.get_chain_info().await?;
    let current_height = chain_info.height;

    if from_height >= current_height {
        return Ok((vec![], current_height));
    }

    let scanner = WalletScanner::new(keys);
    let mut all_utxos = Vec::new();

    // Scan in batches of 100 blocks
    const BATCH_SIZE: u64 = 100;
    let mut height = from_height;

    while height < current_height {
        let end_height = (height + BATCH_SIZE).min(current_height);

        let outputs = rpc.get_outputs(height, end_height).await?;
        let owned = scanner.scan_outputs(&outputs);
        all_utxos.extend(owned);

        height = end_height;
    }

    Ok((all_utxos, current_height))
}

/// Result of syncing both classical and PQ UTXOs
#[cfg(feature = "pq")]
pub struct SyncResult {
    /// Classical (non-PQ) UTXOs
    pub classical_utxos: Vec<OwnedUtxo>,
    /// Quantum-private UTXOs (ML-KEM encapsulated)
    pub pq_utxos: Vec<PqOwnedUtxo>,
    /// Current chain height
    pub height: u64,
}

/// Sync wallet UTXOs with the network, returning both classical and PQ outputs
///
/// This scans the blockchain for both:
/// - Classical stealth address outputs (ECDH-based)
/// - Quantum-private outputs (ML-KEM-based)
#[cfg(feature = "pq")]
pub async fn sync_wallet_all(
    rpc: &mut RpcPool,
    keys: &WalletKeys,
    from_height: u64,
) -> Result<SyncResult> {
    // Get current chain height
    let chain_info = rpc.get_chain_info().await?;
    let current_height = chain_info.height;

    if from_height >= current_height {
        return Ok(SyncResult {
            classical_utxos: vec![],
            pq_utxos: vec![],
            height: current_height,
        });
    }

    let classical_scanner = WalletScanner::new(keys);
    let pq_scanner = PqWalletScanner::new(keys);

    let mut all_classical = Vec::new();
    let mut all_pq = Vec::new();

    // Scan in batches of 100 blocks
    const BATCH_SIZE: u64 = 100;
    let mut height = from_height;

    while height < current_height {
        let end_height = (height + BATCH_SIZE).min(current_height);

        let outputs = rpc.get_outputs(height, end_height).await?;

        // Scan for both types of outputs
        let classical = classical_scanner.scan_outputs(&outputs);
        let pq = pq_scanner.scan_pq_outputs(&outputs);

        all_classical.extend(classical);
        all_pq.extend(pq);

        height = end_height;
    }

    Ok(SyncResult {
        classical_utxos: all_classical,
        pq_utxos: all_pq,
        height: current_height,
    })
}

/// Format an amount in picocredits as CAD
pub fn format_amount(picocredits: u64) -> String {
    let cad = picocredits as f64 / PICOCREDITS_PER_CAD as f64;
    format!("{:.6} CAD", cad)
}

/// Parse a BTH amount string to picocredits
pub fn parse_amount(cad: &str) -> Result<u64> {
    let value: f64 = cad
        .trim()
        .trim_end_matches(" CAD")
        .trim_end_matches("CAD")
        .parse()
        .map_err(|_| anyhow!("Invalid amount format"))?;

    if value < 0.0 {
        return Err(anyhow!("Amount cannot be negative"));
    }

    let picocredits = (value * PICOCREDITS_PER_CAD as f64) as u64;
    Ok(picocredits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_amount() {
        assert_eq!(format_amount(1_000_000_000_000), "1.000000 CAD");
        assert_eq!(format_amount(500_000_000_000), "0.500000 CAD");
        assert_eq!(format_amount(1_000_000), "0.000001 CAD");
    }

    #[test]
    fn test_parse_amount() {
        assert_eq!(parse_amount("1.0").unwrap(), 1_000_000_000_000);
        assert_eq!(parse_amount("0.5").unwrap(), 500_000_000_000);
        assert_eq!(parse_amount("1.0 CAD").unwrap(), 1_000_000_000_000);
    }

    #[test]
    fn test_utxo_selection() {
        let keys = WalletKeys::from_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art"
        ).unwrap();

        let utxos = vec![
            OwnedUtxo {
                tx_hash: [1u8; 32],
                output_index: 0,
                amount: 1_000_000_000_000, // 1 CAD
                created_at: 1,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
            OwnedUtxo {
                tx_hash: [2u8; 32],
                output_index: 0,
                amount: 500_000_000_000, // 0.5 CAD
                created_at: 2,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                subaddress_index: 0,
                cluster_tags: None,
            },
        ];

        let builder = TransactionBuilder::new(keys, utxos, 100);
        assert_eq!(builder.balance(), 1_500_000_000_000);
    }

    #[test]
    fn test_transaction_signing_hash() {
        let tx1 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0u8; 64],
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        let tx2 = Transaction::new(
            vec![TxInput {
                tx_hash: [1u8; 32],
                output_index: 0,
                signature: vec![0xff; 64], // Different signature
            }],
            vec![TxOutput {
                amount: 1000,
                recipient_view_key: [2u8; 32],
                recipient_spend_key: [3u8; 32],
                output_public_key: [4u8; 32],
            }],
            100,
            1,
        );

        // Signing hash should be the same regardless of signature content
        assert_eq!(tx1.signing_hash(), tx2.signing_hash());
    }

    #[cfg(feature = "pq")]
    mod pq_tests {
        use super::*;
        use crate::rpc_pool::TxOutput as RpcTxOutput;

        const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

        #[test]
        fn test_pq_owned_utxo_serialization() {
            let utxo = PqOwnedUtxo {
                tx_hash: [1u8; 32],
                output_index: 0,
                amount: 1_000_000_000_000,
                created_at: 100,
                kem_ciphertext: vec![0u8; 1088],
                target_key: [2u8; 32],
                subaddress_index: 0,
            };

            // Test serialization roundtrip
            let serialized = bincode::serialize(&utxo).expect("serialize");
            let deserialized: PqOwnedUtxo = bincode::deserialize(&serialized).expect("deserialize");

            assert_eq!(utxo.tx_hash, deserialized.tx_hash);
            assert_eq!(utxo.output_index, deserialized.output_index);
            assert_eq!(utxo.amount, deserialized.amount);
            assert_eq!(utxo.created_at, deserialized.created_at);
            assert_eq!(utxo.kem_ciphertext, deserialized.kem_ciphertext);
            assert_eq!(utxo.target_key, deserialized.target_key);
            assert_eq!(utxo.subaddress_index, deserialized.subaddress_index);
        }

        #[test]
        fn test_pq_owned_utxo_id() {
            let utxo = PqOwnedUtxo {
                tx_hash: [42u8; 32],
                output_index: 7,
                amount: 1_000_000,
                created_at: 500,
                kem_ciphertext: vec![0u8; 1088],
                target_key: [0u8; 32],
                subaddress_index: 0,
            };

            let id = utxo.id();
            assert_eq!(id.tx_hash, [42u8; 32]);
            assert_eq!(id.output_index, 7);
        }

        #[test]
        fn test_pq_scanner_skips_non_pq_outputs() {
            let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
            let scanner = PqWalletScanner::new(&keys);

            // Create a non-PQ output
            let outputs = vec![BlockOutputs {
                height: 100,
                outputs: vec![RpcTxOutput {
                    tx_hash: hex::encode([1u8; 32]),
                    output_index: 0,
                    target_key: hex::encode([2u8; 32]),
                    public_key: hex::encode([3u8; 32]),
                    amount_commitment: hex::encode(1000u64.to_le_bytes()),
                    pq_ciphertext: None,
                    is_pq_output: false, // Not a PQ output
                }],
            }];

            let result = scanner.scan_pq_outputs(&outputs);
            assert!(result.is_empty(), "Should not find any PQ outputs");
        }

        #[test]
        fn test_pq_scanner_rejects_invalid_ciphertext() {
            let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
            let scanner = PqWalletScanner::new(&keys);

            // Create a PQ output with invalid ciphertext (wrong size)
            let outputs = vec![BlockOutputs {
                height: 100,
                outputs: vec![RpcTxOutput {
                    tx_hash: hex::encode([1u8; 32]),
                    output_index: 0,
                    target_key: hex::encode([2u8; 32]),
                    public_key: hex::encode([3u8; 32]),
                    amount_commitment: hex::encode(1000u64.to_le_bytes()),
                    pq_ciphertext: Some(hex::encode([0u8; 100])), // Wrong size
                    is_pq_output: true,
                }],
            }];

            let result = scanner.scan_pq_outputs(&outputs);
            assert!(result.is_empty(), "Should reject invalid ciphertext size");
        }

        #[test]
        fn test_sync_result_fields() {
            let result = SyncResult {
                classical_utxos: vec![OwnedUtxo {
                    tx_hash: [1u8; 32],
                    output_index: 0,
                    amount: 1_000_000_000_000,
                    created_at: 100,
                    target_key: [0u8; 32],
                    public_key: [0u8; 32],
                    subaddress_index: 0,
                    cluster_tags: None,
                }],
                pq_utxos: vec![PqOwnedUtxo {
                    tx_hash: [2u8; 32],
                    output_index: 0,
                    amount: 500_000_000_000,
                    created_at: 101,
                    kem_ciphertext: vec![0u8; 1088],
                    target_key: [0u8; 32],
                    subaddress_index: 0,
                }],
                height: 1000,
            };

            assert_eq!(result.classical_utxos.len(), 1);
            assert_eq!(result.pq_utxos.len(), 1);
            assert_eq!(result.height, 1000);

            let classical_total: u64 = result.classical_utxos.iter().map(|u| u.amount).sum();
            let pq_total: u64 = result.pq_utxos.iter().map(|u| u.amount).sum();

            assert_eq!(classical_total, 1_000_000_000_000);
            assert_eq!(pq_total, 500_000_000_000);
        }
    }
}
