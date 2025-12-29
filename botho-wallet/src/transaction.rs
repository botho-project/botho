//! Transaction Building and Signing
//!
//! Handles local transaction construction and signing for the thin wallet.
//! All signing happens locally - private keys never leave the wallet.

use anyhow::{anyhow, Result};
use bth_account_keys::PublicAddress;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::keys::WalletKeys;
use crate::rpc_pool::{BlockOutputs, RpcPool};

/// Picocredits per CAD
pub const PICOCREDITS_PER_CAD: u64 = 1_000_000_000_000;

/// Minimum transaction fee
pub const MIN_FEE: u64 = 1_000_000; // 0.000001 CAD

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
}

impl OwnedUtxo {
    /// Create a UTXO identifier
    pub fn id(&self) -> UtxoId {
        UtxoId {
            tx_hash: self.tx_hash,
            output_index: self.output_index,
        }
    }
}

/// UTXO identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UtxoId {
    pub tx_hash: [u8; 32],
    pub output_index: u32,
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

        let total_needed = amount.checked_add(fee)
            .ok_or_else(|| anyhow!("Amount overflow"))?;

        // Select UTXOs
        let (selected, total_selected) = self.select_utxos(total_needed)?;

        // Calculate change
        let change = total_selected.checked_sub(total_needed)
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

        // Add change output if non-dust
        if change > MIN_FEE {
            outputs.push(TxOutput::new(change, &self.keys.public_address()));
        }

        // Create transaction
        let mut tx = Transaction::new(inputs, outputs, fee, self.sync_height);

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

/// Wallet scanner for finding owned outputs
pub struct WalletScanner {
    spend_key: [u8; 32],
}

impl WalletScanner {
    /// Create a new scanner for the given wallet keys
    pub fn new(keys: &WalletKeys) -> Self {
        Self {
            spend_key: keys.spend_public_key_bytes(),
        }
    }

    /// Scan block outputs for UTXOs belonging to this wallet
    pub fn scan_outputs(&self, block_outputs: &[BlockOutputs]) -> Vec<OwnedUtxo> {
        let mut owned = Vec::new();

        for block in block_outputs {
            for output in &block.outputs {
                // Parse the public key from hex
                if let Ok(spend_key_bytes) = hex::decode(&output.public_key) {
                    if spend_key_bytes.len() >= 32 {
                        let mut key = [0u8; 32];
                        key.copy_from_slice(&spend_key_bytes[..32]);

                        if key == self.spend_key {
                            // This output belongs to us
                            if let Ok(tx_hash) = hex::decode(&output.tx_hash) {
                                if tx_hash.len() >= 32 {
                                    let mut hash = [0u8; 32];
                                    hash.copy_from_slice(&tx_hash[..32]);

                                    owned.push(OwnedUtxo {
                                        tx_hash: hash,
                                        output_index: output.output_index,
                                        amount: 0, // Would need to decrypt amount commitment
                                        created_at: block.height,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        owned
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
            },
            OwnedUtxo {
                tx_hash: [2u8; 32],
                output_index: 0,
                amount: 500_000_000_000, // 0.5 CAD
                created_at: 2,
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
}
