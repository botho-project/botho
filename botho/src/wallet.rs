use anyhow::Result;
use bip39::{Language, Mnemonic};
use bth_account_keys::{AccountKey, PublicAddress};
use bth_core::slip10::Slip10KeyGenerator;
use bth_crypto_keys::RistrettoSignature;
use rand::rngs::OsRng;
use rand::seq::SliceRandom;

#[cfg(feature = "pq")]
use bth_account_keys::{QuantumSafeAccountKey, QuantumSafePublicAddress};

use crate::ledger::Ledger;
use crate::transaction::{
    RingMember, RingTxInput, Transaction, TxInputs, TxOutput, Utxo, UtxoId, MIN_RING_SIZE,
};

#[cfg(feature = "pq")]
use crate::transaction_pq::{
    QuantumPrivateTransaction, QuantumPrivateTxInput, QuantumPrivateTxOutput,
};

/// Wallet manages a single account derived from a BIP39 mnemonic
pub struct Wallet {
    account_key: AccountKey,
    #[cfg(feature = "pq")]
    pq_account_key: QuantumSafeAccountKey,
    mnemonic_phrase: String,
}

impl Wallet {
    /// Create a wallet from a mnemonic phrase
    ///
    /// All keys (classical and post-quantum) derive from the same mnemonic,
    /// ensuring a single unified identity. The classical keys use SLIP-10
    /// derivation (BIP39 compliant), while PQ keys use HKDF from the mnemonic.
    pub fn from_mnemonic(mnemonic_phrase: &str) -> Result<Self> {
        let mnemonic = Mnemonic::from_phrase(mnemonic_phrase, Language::English)
            .map_err(|e| anyhow::anyhow!("Invalid mnemonic: {}", e))?;

        // Derive classical keys via SLIP-10 (standard BIP39 path)
        let slip10_key = mnemonic.derive_slip10_key(0);
        let account_key = AccountKey::from(slip10_key);

        // Derive PQ keys and create unified quantum-safe account
        // IMPORTANT: Uses the SAME classical keys to maintain single identity
        #[cfg(feature = "pq")]
        let pq_account_key = {
            use bth_crypto_pq::derive_pq_keys;
            let pq_keys = derive_pq_keys(mnemonic_phrase.as_bytes());
            QuantumSafeAccountKey::from_parts(account_key.clone(), pq_keys)
        };

        Ok(Self {
            account_key,
            #[cfg(feature = "pq")]
            pq_account_key,
            mnemonic_phrase: mnemonic_phrase.to_string(),
        })
    }

    /// Get the default public address for receiving funds
    pub fn default_address(&self) -> PublicAddress {
        self.account_key.default_subaddress()
    }

    /// Get the account key (needed for transaction signing)
    pub fn account_key(&self) -> &AccountKey {
        &self.account_key
    }

    /// Format the public address as a string for display
    pub fn address_string(&self) -> String {
        let addr = self.default_address();
        // Use hex encoding of the view and spend public keys
        format!(
            "view:{}\nspend:{}",
            hex::encode(addr.view_public_key().to_bytes()),
            hex::encode(addr.spend_public_key().to_bytes())
        )
    }

    /// Get the quantum-safe public address for receiving funds
    #[cfg(feature = "pq")]
    pub fn quantum_safe_address(&self) -> QuantumSafePublicAddress {
        self.pq_account_key.default_subaddress()
    }

    /// Format the quantum-safe public address as a string for display
    #[cfg(feature = "pq")]
    pub fn quantum_safe_address_string(&self) -> String {
        self.quantum_safe_address().to_address_string()
    }

    /// Get the quantum-safe account key
    #[cfg(feature = "pq")]
    pub fn pq_account_key(&self) -> &QuantumSafeAccountKey {
        &self.pq_account_key
    }

    /// Sign all inputs of a transaction using stealth address keys
    ///
    /// With stealth addresses, each UTXO has a unique one-time key. This method:
    /// 1. Looks up each UTXO being spent
    /// 2. Uses stealth scanning (belongs_to) to verify ownership
    /// 3. Recovers the one-time private key for signing
    /// 4. Signs with the one-time private key (not the wallet's main spend key)
    ///
    /// Note: This method only signs Simple inputs. Ring inputs use MLSAG
    /// signatures which are created during transaction construction.
    ///
    /// Returns an error if:
    /// - A referenced UTXO doesn't exist
    /// - The wallet doesn't own the UTXO (stealth detection fails)
    /// - The transaction uses Ring inputs (not supported by this method)
    pub fn sign_transaction(&self, tx: &mut Transaction, ledger: &Ledger) -> Result<()> {
        let signing_hash = tx.signing_hash();

        let inputs = match &mut tx.inputs {
            TxInputs::Simple(inputs) => inputs,
            TxInputs::Ring(_) => {
                return Err(anyhow::anyhow!(
                    "Ring signature transactions must be signed during construction"
                ));
            }
        };

        for input in inputs {
            // Look up the UTXO being spent
            let utxo_id = UtxoId::new(input.tx_hash, input.output_index);
            let utxo = ledger
                .get_utxo(&utxo_id)
                .map_err(|e| anyhow::anyhow!("Failed to get UTXO: {}", e))?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "UTXO not found: {}:{}",
                        hex::encode(&input.tx_hash[0..8]),
                        input.output_index
                    )
                })?;

            // Use stealth detection to verify ownership and get subaddress index
            let subaddress_index = utxo.output.belongs_to(&self.account_key).ok_or_else(|| {
                anyhow::anyhow!(
                    "UTXO {}:{} does not belong to this wallet",
                    hex::encode(&input.tx_hash[0..8]),
                    input.output_index
                )
            })?;

            // Recover the one-time private key for this stealth output
            let onetime_private = utxo
                .output
                .recover_spend_key(&self.account_key, subaddress_index)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Failed to recover spend key for UTXO {}:{}",
                        hex::encode(&input.tx_hash[0..8]),
                        input.output_index
                    )
                })?;

            // Sign the transaction with the one-time private key
            let signature: RistrettoSignature =
                onetime_private.sign_schnorrkel(b"botho-tx-v1", &signing_hash);

            // Store the 64-byte signature
            let sig_bytes: &[u8] = signature.as_ref();
            input.signature = sig_bytes.to_vec();
        }

        Ok(())
    }

    /// Create a private (ring signature) transaction for sender privacy.
    ///
    /// Ring signatures hide which UTXO is actually being spent by mixing it
    /// with decoy outputs from the ledger. The signature proves ownership
    /// of one ring member without revealing which one.
    ///
    /// # Arguments
    /// * `utxos_to_spend` - The wallet's UTXOs to spend
    /// * `outputs` - Transaction outputs to create
    /// * `fee` - Transaction fee
    /// * `current_height` - Current blockchain height
    /// * `ledger` - Ledger for fetching decoy outputs
    ///
    /// # Returns
    /// A fully signed private transaction ready for broadcast
    pub fn create_private_transaction(
        &self,
        utxos_to_spend: &[Utxo],
        outputs: Vec<TxOutput>,
        fee: u64,
        current_height: u64,
        ledger: &Ledger,
    ) -> Result<Transaction> {
        if utxos_to_spend.is_empty() {
            return Err(anyhow::anyhow!("No UTXOs to spend"));
        }

        // Calculate total output amount for the signing message
        let total_output: u64 = outputs.iter().map(|o| o.amount).sum::<u64>() + fee;

        // Build a preliminary transaction to get the signing hash
        // We'll replace the inputs with real ring inputs after signing
        let preliminary_tx = Transaction::new_private(Vec::new(), outputs.clone(), fee, current_height);
        let signing_hash = preliminary_tx.signing_hash();

        // Number of decoys per ring (MIN_RING_SIZE - 1 since real input is included)
        let decoys_needed = MIN_RING_SIZE - 1;

        // Collect target keys of our real inputs to exclude from decoys
        let exclude_keys: Vec<[u8; 32]> = utxos_to_spend
            .iter()
            .map(|u| u.output.target_key)
            .collect();

        // Get decoy outputs from ledger (need enough for all inputs)
        let total_decoys_needed = decoys_needed * utxos_to_spend.len();
        let decoys = ledger
            .get_decoy_outputs(total_decoys_needed, &exclude_keys, 10)
            .map_err(|e| anyhow::anyhow!("Failed to get decoy outputs: {}", e))?;

        if decoys.len() < total_decoys_needed {
            return Err(anyhow::anyhow!(
                "Not enough decoy outputs in ledger. Need {}, found {}. \
                 The ledger needs at least {} confirmed outputs for private transactions.",
                total_decoys_needed,
                decoys.len(),
                MIN_RING_SIZE
            ));
        }

        // Build ring inputs
        let mut ring_inputs = Vec::with_capacity(utxos_to_spend.len());
        let mut decoy_offset = 0;

        for utxo in utxos_to_spend {
            // Verify ownership and recover one-time private key
            let subaddress_index = utxo.output.belongs_to(&self.account_key).ok_or_else(|| {
                anyhow::anyhow!(
                    "UTXO does not belong to this wallet: {}",
                    hex::encode(&utxo.id.tx_hash[0..8])
                )
            })?;

            let onetime_private = utxo
                .output
                .recover_spend_key(&self.account_key, subaddress_index)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Failed to recover spend key for UTXO {}",
                        hex::encode(&utxo.id.tx_hash[0..8])
                    )
                })?;

            // Build ring: real output + decoys
            let mut ring: Vec<RingMember> = Vec::with_capacity(MIN_RING_SIZE);

            // Add the real input
            ring.push(RingMember::from_output(&utxo.output));

            // Add decoys
            for i in 0..decoys_needed {
                let decoy = &decoys[decoy_offset + i];
                ring.push(RingMember::from_output(decoy));
            }
            decoy_offset += decoys_needed;

            // Shuffle ring and find the new position of the real input
            let mut rng = OsRng;
            let real_target_key = utxo.output.target_key;

            // Create indices and shuffle them
            let mut indices: Vec<usize> = (0..ring.len()).collect();
            indices.shuffle(&mut rng);

            // Reorder ring according to shuffled indices
            let shuffled_ring: Vec<RingMember> = indices.iter().map(|&i| ring[i].clone()).collect();

            // Find where the real input ended up
            // This should always succeed since we just added it, but handle gracefully
            let real_index = shuffled_ring
                .iter()
                .position(|m| m.target_key == real_target_key)
                .ok_or_else(|| anyhow::anyhow!(
                    "Internal error: real input not found in ring after shuffle"
                ))?;

            // Create ring input with MLSAG signature
            let ring_input = RingTxInput::new(
                shuffled_ring,
                real_index,
                &onetime_private,
                utxo.output.amount,
                total_output,
                &signing_hash,
                &mut rng,
            )
            .map_err(|e| anyhow::anyhow!("Failed to create ring signature: {}", e))?;

            ring_inputs.push(ring_input);
        }

        // Create the final transaction with ring inputs
        let tx = Transaction::new_private(ring_inputs, outputs, fee, current_height);

        Ok(tx)
    }

    /// Create a quantum-private transaction for post-quantum security.
    ///
    /// Quantum-private transactions use hybrid classical + post-quantum cryptography:
    /// - Outputs: Classical stealth keys + ML-KEM-768 encapsulation
    /// - Inputs: Schnorr signature + ML-DSA-65 (Dilithium) signature
    ///
    /// This provides protection against "harvest now, decrypt later" attacks where
    /// adversaries archive blockchain data for future quantum cryptanalysis.
    ///
    /// # Arguments
    /// * `utxos_to_spend` - The wallet's UTXOs to spend
    /// * `recipient` - Recipient's quantum-safe public address
    /// * `amount` - Amount to send
    /// * `fee` - Transaction fee
    /// * `current_height` - Current blockchain height
    ///
    /// # Returns
    /// A fully signed quantum-private transaction ready for broadcast
    #[cfg(feature = "pq")]
    pub fn create_quantum_private_transaction(
        &self,
        utxos_to_spend: &[Utxo],
        recipient: &QuantumSafePublicAddress,
        amount: u64,
        fee: u64,
        current_height: u64,
    ) -> Result<QuantumPrivateTransaction> {

        if utxos_to_spend.is_empty() {
            return Err(anyhow::anyhow!("No UTXOs to spend"));
        }

        // Calculate total input value
        let total_input: u64 = utxos_to_spend.iter().map(|u| u.output.amount).sum();
        let change = total_input.checked_sub(amount + fee)
            .ok_or_else(|| anyhow::anyhow!("Insufficient funds for amount + fee"))?;

        // Build outputs
        let mut outputs = Vec::new();

        // Output to recipient
        outputs.push(QuantumPrivateTxOutput::new(amount, recipient));

        // Change output (if any)
        if change > 0 {
            let change_addr = self.quantum_safe_address();
            outputs.push(QuantumPrivateTxOutput::new(change, &change_addr));
        }

        // Build a preliminary transaction to get signing hash
        let preliminary_tx = QuantumPrivateTransaction::new(
            Vec::new(),
            outputs.clone(),
            fee,
            current_height,
        );
        let signing_hash = preliminary_tx.signing_hash();

        // Build and sign inputs
        let mut inputs = Vec::new();

        for utxo in utxos_to_spend {
            // Verify ownership and recover classical one-time private key
            let subaddress_index = utxo.output.belongs_to(&self.account_key).ok_or_else(|| {
                anyhow::anyhow!(
                    "UTXO does not belong to this wallet: {}",
                    hex::encode(&utxo.id.tx_hash[0..8])
                )
            })?;

            let onetime_private = utxo
                .output
                .recover_spend_key(&self.account_key, subaddress_index)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Failed to recover spend key for UTXO {}",
                        hex::encode(&utxo.id.tx_hash[0..8])
                    )
                })?;

            // For PQ inputs, we need to derive the PQ one-time keypair.
            // Since existing UTXOs don't have PQ ciphertexts yet, we use a
            // deterministic derivation from the output's key material.
            // This provides forward secrecy: new quantum-private outputs will
            // have proper ML-KEM encapsulation.
            //
            // We compute: shared_secret = SHA256("botho-pq-bridge" || target_key || public_key || view_private)
            // This binds the PQ signature to the specific output and the wallet's view key.
            let pq_shared_secret = {
                use sha2::{Sha256, Digest};
                let view_private_bytes = self.account_key.view_private_key().to_bytes();
                let mut hasher = Sha256::new();
                hasher.update(b"botho-pq-bridge-v1");
                hasher.update(&utxo.output.target_key);
                hasher.update(&utxo.output.public_key);
                hasher.update(&view_private_bytes);
                let hash: [u8; 32] = hasher.finalize().into();
                hash
            };

            // Create quantum-private input with dual signatures
            let input = QuantumPrivateTxInput::new(
                utxo.id.tx_hash,
                utxo.id.output_index,
                &signing_hash,
                &onetime_private,
                &pq_shared_secret,
            );

            inputs.push(input);
        }

        // Create the final transaction
        let tx = QuantumPrivateTransaction::new(inputs, outputs, fee, current_height);

        Ok(tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_from_mnemonic() {
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        let wallet = Wallet::from_mnemonic(mnemonic).unwrap();
        let addr = wallet.default_address();
        // Just verify we get a valid address
        assert!(!addr.view_public_key().to_bytes().is_empty());
    }
}
