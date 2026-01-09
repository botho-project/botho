use anyhow::Result;
use bip39::{Language, Mnemonic, Seed};
use bth_account_keys::{AccountKey, PublicAddress};
use bth_cluster_tax::crypto::{CommittedTagVectorSecret, EntropyProof, EntropyProofBuilder};
use bth_cluster_tax::ClusterId as TaxClusterId;
use bth_core::slip10::Slip10KeyGenerator;
use bth_transaction_types::{ClusterTagVector, TAG_WEIGHT_SCALE};
use rand::{rngs::OsRng, seq::SliceRandom};
use std::collections::HashMap;
use tracing::{debug, warn};
use zeroize::Zeroizing;

#[cfg(feature = "pq")]
use bth_account_keys::{QuantumSafeAccountKey, QuantumSafePublicAddress};
#[cfg(feature = "pq")]
use bth_crypto_pq::{derive_pq_keys_from_seed, BIP39_SEED_SIZE};

use crate::{
    decoy_selection::GammaDecoySelector,
    ledger::Ledger,
    transaction::{ClsagRingInput, RingMember, Transaction, TxOutput, Utxo, MIN_RING_SIZE},
};

/// Default decay rate for cluster tags when transferring coins.
/// 5% decay per transaction (50,000 / 1,000,000 = 5%).
pub const DEFAULT_CLUSTER_DECAY_RATE: u32 = 50_000;

/// Estimated size of an entropy proof in bytes.
///
/// Based on the design spec (docs/design/entropy-proof-integration.md):
/// - 2 entropy commitments: 64 bytes
/// - Range proof: ~160 bytes (simplified Schnorr)
/// - Linkage proof: ~4 + 32*N + 64*N + 64 bytes (N = cluster count)
///
/// For typical transactions with 2-4 clusters: ~700-1000 bytes
/// We use 1024 bytes as a conservative estimate.
pub const ENTROPY_PROOF_SIZE_ESTIMATE: usize = 1024;

/// Transaction version indicating supported features.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransactionVersion {
    /// V2: Phase 1 committed tags
    /// - Committed cluster tags
    /// - Tag conservation proofs
    /// - No entropy proof
    #[default]
    V2,

    /// V3: Phase 2 entropy proofs
    /// - All V2 features
    /// - Entropy proof in ExtendedTxSignature
    /// - Entropy-weighted decay
    V3,
}

impl TransactionVersion {
    /// Check if this version supports entropy proofs.
    pub fn supports_entropy_proof(&self) -> bool {
        matches!(self, Self::V3)
    }
}

/// Configuration for transaction creation.
#[derive(Clone, Debug)]
pub struct TransactionConfig {
    /// Transaction version to create.
    /// V3 includes entropy proofs for full decay credit.
    pub version: TransactionVersion,

    /// Decay rate applied to tags (parts per TAG_WEIGHT_SCALE).
    /// Default: 50,000 (5%)
    pub decay_rate: u32,

    /// Whether to fall back to V2 if V3 entropy proof generation fails.
    /// Default: true
    pub fallback_on_proof_failure: bool,
}

impl Default for TransactionConfig {
    fn default() -> Self {
        Self {
            version: TransactionVersion::V3, // Default to V3 for full decay credit
            decay_rate: DEFAULT_CLUSTER_DECAY_RATE,
            fallback_on_proof_failure: true,
        }
    }
}

impl TransactionConfig {
    /// Create a V2 configuration (no entropy proof).
    pub fn v2() -> Self {
        Self {
            version: TransactionVersion::V2,
            ..Default::default()
        }
    }

    /// Create a V3 configuration (with entropy proof).
    pub fn v3() -> Self {
        Self {
            version: TransactionVersion::V3,
            ..Default::default()
        }
    }

    /// Set whether to fall back to V2 on proof failure.
    pub fn with_fallback(mut self, fallback: bool) -> Self {
        self.fallback_on_proof_failure = fallback;
        self
    }

    /// Set custom decay rate.
    pub fn with_decay_rate(mut self, decay_rate: u32) -> Self {
        self.decay_rate = decay_rate;
        self
    }
}

/// Result of entropy proof generation.
#[derive(Debug)]
pub enum EntropyProofResult {
    /// Proof generated successfully.
    Generated(EntropyProof),
    /// Proof generation skipped (V2 transaction or not enough entropy increase).
    Skipped,
    /// Proof generation failed, fell back to V2.
    Fallback(String),
}

#[cfg(feature = "pq")]
use crate::transaction_pq::{
    QuantumPrivateTransaction, QuantumPrivateTxInput, QuantumPrivateTxOutput,
};

/// Wallet manages a single account derived from a BIP39 mnemonic.
///
/// Security: The mnemonic phrase is stored in a `Zeroizing<String>` wrapper
/// that automatically overwrites the memory with zeros when dropped,
/// preventing the sensitive recovery phrase from persisting in memory.
pub struct Wallet {
    account_key: AccountKey,
    #[cfg(feature = "pq")]
    pq_account_key: QuantumSafeAccountKey,
    /// Mnemonic phrase wrapped in Zeroizing for secure memory cleanup on drop.
    #[allow(dead_code)] // Stored for potential future recovery/export features
    mnemonic_phrase: Zeroizing<String>,
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

        // Derive PQ keys first (before mnemonic is moved by SLIP-10 derivation)
        // PQ keys are derived from the full BIP39 seed (with PBKDF2 key stretching)
        #[cfg(feature = "pq")]
        let bip39_seed = Seed::new(&mnemonic, "");

        // Derive classical keys via SLIP-10 (standard BIP39 path)
        // This consumes the mnemonic
        let slip10_key = mnemonic.derive_slip10_key(0);
        let account_key = AccountKey::from(slip10_key);

        // Create unified quantum-safe account from BIP39 seed
        // IMPORTANT: Uses the SAME classical keys to maintain single identity
        #[cfg(feature = "pq")]
        let pq_account_key = {
            let seed_bytes: &[u8; BIP39_SEED_SIZE] = bip39_seed
                .as_bytes()
                .try_into()
                .expect("BIP39 seed is always 64 bytes");
            let pq_keys = derive_pq_keys_from_seed(seed_bytes);
            QuantumSafeAccountKey::from_parts(account_key.clone(), pq_keys)
        };

        Ok(Self {
            account_key,
            #[cfg(feature = "pq")]
            pq_account_key,
            mnemonic_phrase: Zeroizing::new(mnemonic_phrase.to_string()),
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

    /// Compute cluster wealth from a set of UTXOs.
    ///
    /// For progressive fee computation, we need to know the maximum cluster
    /// wealth among all clusters the wallet's coins are tagged with. Higher
    /// cluster wealth = higher fee multiplier (1x to 6x).
    ///
    /// The cluster wealth for cluster C is: W_C = Σ (utxo_value × tag_weight_C
    /// / TAG_WEIGHT_SCALE)
    ///
    /// This method returns the **maximum** cluster wealth across all clusters,
    /// as that determines the fee rate.
    pub fn compute_cluster_wealth(utxos: &[Utxo]) -> u64 {
        // Accumulate wealth per cluster: cluster_id -> total weighted value
        let mut cluster_wealths: HashMap<u64, u64> = HashMap::new();

        for utxo in utxos {
            let value = utxo.output.amount;
            for entry in &utxo.output.cluster_tags.entries {
                // Contribution = value × weight / TAG_WEIGHT_SCALE
                // Use u128 to avoid overflow during multiplication
                let contribution =
                    ((value as u128) * (entry.weight as u128) / (TAG_WEIGHT_SCALE as u128)) as u64;

                *cluster_wealths.entry(entry.cluster_id.0).or_insert(0) += contribution;
            }
        }

        // Return the maximum cluster wealth (determines fee rate)
        cluster_wealths.values().copied().max().unwrap_or(0)
    }

    /// Compute inherited cluster tags for transaction outputs.
    ///
    /// When creating a transaction, outputs inherit tags from inputs with
    /// decay. This ensures coin lineage is tracked through the transaction
    /// graph.
    ///
    /// # Arguments
    /// * `utxos` - The UTXOs being spent as inputs
    /// * `decay_rate` - Decay to apply (parts per TAG_WEIGHT_SCALE, e.g.,
    ///   50_000 = 5%)
    ///
    /// # Returns
    /// A ClusterTagVector representing the merged and decayed tags.
    pub fn compute_inherited_tags(utxos: &[Utxo], decay_rate: u32) -> ClusterTagVector {
        let inputs: Vec<(ClusterTagVector, u64)> = utxos
            .iter()
            .map(|u| (u.output.cluster_tags.clone(), u.output.amount))
            .collect();

        ClusterTagVector::merge_weighted(&inputs, decay_rate)
    }

    /// Estimate transaction size including entropy proof.
    ///
    /// This helps with fee calculation by accounting for the additional
    /// ~1KB entropy proof in V3 transactions.
    ///
    /// # Arguments
    /// * `num_inputs` - Number of transaction inputs
    /// * `num_outputs` - Number of transaction outputs
    /// * `version` - Transaction version (V2 or V3)
    ///
    /// # Returns
    /// Estimated transaction size in bytes.
    pub fn estimate_transaction_size(
        num_inputs: usize,
        num_outputs: usize,
        version: TransactionVersion,
    ) -> usize {
        // Base transaction size estimates (from design spec):
        // - Base overhead: ~100 bytes
        // - Per input (CLSAG signature): ~700 bytes
        // - Per output: ~100 bytes
        // - Tag conservation proof: ~500 bytes
        let base_size = 100 + (num_inputs * 700) + (num_outputs * 100) + 500;

        // Add entropy proof size for V3
        if version.supports_entropy_proof() {
            base_size + ENTROPY_PROOF_SIZE_ESTIMATE
        } else {
            base_size
        }
    }

    /// Estimate fee with entropy proof overhead.
    ///
    /// Calculates the recommended fee for a transaction, accounting for
    /// the additional entropy proof size in V3 transactions.
    ///
    /// # Arguments
    /// * `utxos` - UTXOs being spent (for cluster wealth calculation)
    /// * `num_outputs` - Number of outputs
    /// * `base_fee_per_byte` - Base fee per byte (before wealth multiplier)
    /// * `version` - Transaction version
    ///
    /// # Returns
    /// Recommended fee in atomic units.
    pub fn estimate_fee_with_entropy_proof(
        utxos: &[Utxo],
        num_outputs: usize,
        base_fee_per_byte: u64,
        version: TransactionVersion,
    ) -> u64 {
        let tx_size = Self::estimate_transaction_size(utxos.len(), num_outputs, version);
        let base_fee = (tx_size as u64) * base_fee_per_byte;

        // Apply progressive fee multiplier based on cluster wealth
        let max_wealth = Self::compute_cluster_wealth(utxos);

        // Fee multiplier: 1x for wealth < 1M, up to 6x for wealth >= 100M
        // Using simplified tier system
        let multiplier = if max_wealth >= 100_000_000 {
            6
        } else if max_wealth >= 10_000_000 {
            4
        } else if max_wealth >= 1_000_000 {
            2
        } else {
            1
        };

        base_fee * multiplier
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
    /// With stealth addresses, each UTXO has a unique one-time key. This
    /// method:
    /// 1. Looks up each UTXO being spent
    /// 2. Uses stealth scanning (belongs_to) to verify ownership
    /// 3. Recovers the one-time private key for signing
    /// 4. Signs with the one-time private key (not the wallet's main spend key)
    ///
    /// Note: This method only signs Simple inputs. Ring inputs use MLSAG
    /// signatures which are created during transaction construction.
    ///
    /// Note: This method is deprecated. All transactions now use CLSAG ring
    /// signatures which are signed during construction.
    ///
    /// Returns an error for all transaction types since Simple transactions
    /// have been removed in favor of privacy-by-default.
    #[deprecated(note = "All transactions now use ring signatures, signed during construction")]
    #[allow(dead_code)]
    pub fn sign_transaction(&self, _tx: &mut Transaction, _ledger: &Ledger) -> Result<()> {
        // All transaction types now use ring signatures
        Err(anyhow::anyhow!(
            "Ring signature transactions must be signed during construction. \
             Use create_private_transaction() instead."
        ))
    }

    /// Create a private (CLSAG) transaction for sender privacy.
    ///
    /// Uses CLSAG ring signatures with 20 decoys for sender anonymity.
    /// This is the recommended option for all private transactions.
    ///
    /// # Arguments
    /// * `utxos_to_spend` - The wallet's UTXOs to spend
    /// * `outputs` - Transaction outputs to create
    /// * `fee` - Transaction fee
    /// * `current_height` - Current blockchain height
    /// * `ledger` - Ledger for fetching decoy outputs
    ///
    /// # Returns
    /// A fully signed CLSAG transaction ready for broadcast
    pub fn create_clsag_transaction(
        &self,
        utxos_to_spend: &[Utxo],
        outputs: Vec<TxOutput>,
        fee: u64,
        current_height: u64,
        ledger: &Ledger,
    ) -> Result<Transaction> {
        self.create_private_transaction_impl(utxos_to_spend, outputs, fee, current_height, ledger)
    }

    /// Alias for `create_clsag_transaction` for backwards compatibility.
    ///
    /// Ring signatures hide which UTXO is actually being spent by mixing it
    /// with decoy outputs from the ledger. The signature proves ownership
    /// of one ring member without revealing which one.
    ///
    /// Uses OSPEAD-style gamma-weighted decoy selection to match real spending
    /// patterns, achieving 1-in-10+ effective anonymity with ring size 20.
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
        self.create_private_transaction_impl(utxos_to_spend, outputs, fee, current_height, ledger)
    }

    /// Internal implementation for CLSAG private transactions.
    fn create_private_transaction_impl(
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

        // Compute inherited cluster tags from inputs with default decay
        // All outputs inherit the same merged+decayed tag vector from inputs
        let inherited_tags =
            Self::compute_inherited_tags(utxos_to_spend, DEFAULT_CLUSTER_DECAY_RATE);

        // Apply inherited tags to all outputs
        let outputs: Vec<TxOutput> = outputs
            .into_iter()
            .map(|mut o| {
                o.cluster_tags = inherited_tags.clone();
                o
            })
            .collect();

        // Calculate total output amount for the signing message
        let total_output: u64 = outputs.iter().map(|o| o.amount).sum::<u64>() + fee;

        // Build a preliminary transaction to get the signing hash
        // We'll replace the inputs with real ring inputs after signing
        let preliminary_tx =
            Transaction::new_clsag(Vec::new(), outputs.clone(), fee, current_height);
        let signing_hash = preliminary_tx.signing_hash();

        // Number of decoys per ring (MIN_RING_SIZE - 1 since real input is included)
        let decoys_needed = MIN_RING_SIZE - 1;

        // Collect target keys of our real inputs to exclude from decoys
        let exclude_keys: Vec<[u8; 32]> =
            utxos_to_spend.iter().map(|u| u.output.target_key).collect();

        // Use OSPEAD gamma-weighted decoy selector for realistic age distribution
        let selector = GammaDecoySelector::new();
        let mut rng = OsRng;

        // Build ring inputs
        let mut ring_inputs = Vec::with_capacity(utxos_to_spend.len());

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

            // Calculate age of the real input for OSPEAD selection
            let real_input_age = current_height.saturating_sub(utxo.created_at);

            // Get OSPEAD-selected decoys for this input
            // Uses gamma distribution to match expected spending patterns
            let decoys = ledger
                .get_decoy_outputs_for_input(
                    decoys_needed,
                    &exclude_keys,
                    10, // min confirmations
                    real_input_age,
                    Some(&selector),
                    &mut rng,
                )
                .map_err(|e| anyhow::anyhow!("Failed to get decoy outputs: {}", e))?;

            if decoys.len() < decoys_needed {
                return Err(anyhow::anyhow!(
                    "Not enough decoy outputs in ledger. Need {}, found {}. \
                     The ledger needs at least {} confirmed outputs for private transactions.",
                    decoys_needed,
                    decoys.len(),
                    MIN_RING_SIZE
                ));
            }

            // Build ring: real output + decoys
            let mut ring: Vec<RingMember> = Vec::with_capacity(MIN_RING_SIZE);

            // Add the real input
            ring.push(RingMember::from_output(&utxo.output));

            // Add OSPEAD-selected decoys
            for decoy in &decoys {
                ring.push(RingMember::from_output(decoy));
            }

            // Shuffle ring and find the new position of the real input
            let real_target_key = utxo.output.target_key;

            // Create indices and shuffle them
            let mut indices: Vec<usize> = (0..ring.len()).collect();
            indices.shuffle(&mut rng);

            // Reorder ring according to shuffled indices
            let shuffled_ring: Vec<RingMember> = indices.iter().map(|&i| ring[i].clone()).collect();

            // Find where the real input ended up
            let real_index = shuffled_ring
                .iter()
                .position(|m| m.target_key == real_target_key)
                .ok_or_else(|| {
                    anyhow::anyhow!("Internal error: real input not found in ring after shuffle")
                })?;

            // Log effective anonymity for this ring (debug)
            // Note: Since TxOutput doesn't contain created_at, we use placeholder ages
            // for decoys. In practice, the OSPEAD selector already matched ages
            // appropriately.
            let ring_ages: Vec<u64> = vec![real_input_age]
                .into_iter()
                .chain(decoys.iter().map(|_| {
                    // Placeholder age - actual decoy ages were matched during selection
                    current_height.saturating_sub(100)
                }))
                .collect();
            let eff_anon = Ledger::effective_anonymity(&ring_ages, Some(&selector));
            debug!(
                "Ring effective anonymity: {:.2} (target: 5+ for 1-in-10)",
                eff_anon
            );

            // Create ring input with CLSAG signature
            let ring_input = ClsagRingInput::new(
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

        // Create the final transaction with CLSAG ring inputs
        let tx = Transaction::new_clsag(ring_inputs, outputs, fee, current_height);

        Ok(tx)
    }

    /// Create a V3 transaction with entropy proof for full decay credit.
    ///
    /// This method creates a transaction with an entropy proof that demonstrates
    /// the transaction creates sufficient entropy increase to qualify for full
    /// decay credit. If entropy proof generation fails and fallback is enabled,
    /// returns a V2 transaction without the proof.
    ///
    /// # Arguments
    /// * `utxos_to_spend` - The wallet's UTXOs to spend
    /// * `outputs` - Transaction outputs to create
    /// * `fee` - Transaction fee
    /// * `current_height` - Current blockchain height
    /// * `ledger` - Ledger for fetching decoy outputs
    /// * `config` - Transaction configuration (version, fallback behavior)
    ///
    /// # Returns
    /// A tuple of (Transaction, EntropyProofResult) where the proof result
    /// indicates whether the entropy proof was generated, skipped, or fell back.
    pub fn create_transaction_v3(
        &self,
        utxos_to_spend: &[Utxo],
        outputs: Vec<TxOutput>,
        fee: u64,
        current_height: u64,
        ledger: &Ledger,
        config: &TransactionConfig,
    ) -> Result<(Transaction, EntropyProofResult)> {
        if utxos_to_spend.is_empty() {
            return Err(anyhow::anyhow!("No UTXOs to spend"));
        }

        // Compute inherited cluster tags from inputs
        let inherited_tags = Self::compute_inherited_tags(utxos_to_spend, config.decay_rate);

        // Apply inherited tags to all outputs
        let outputs: Vec<TxOutput> = outputs
            .into_iter()
            .map(|mut o| {
                o.cluster_tags = inherited_tags.clone();
                o
            })
            .collect();

        // Try to generate entropy proof if V3 is requested
        let entropy_result = if config.version.supports_entropy_proof() {
            match self.generate_entropy_proof(utxos_to_spend, &outputs, config.decay_rate) {
                Ok(proof) => {
                    debug!("Entropy proof generated successfully");
                    EntropyProofResult::Generated(proof)
                }
                Err(e) => {
                    if config.fallback_on_proof_failure {
                        warn!("Entropy proof generation failed, falling back to V2: {}", e);
                        EntropyProofResult::Fallback(e.to_string())
                    } else {
                        return Err(anyhow::anyhow!(
                            "Entropy proof generation failed and fallback disabled: {}",
                            e
                        ));
                    }
                }
            }
        } else {
            debug!("V2 transaction requested, skipping entropy proof");
            EntropyProofResult::Skipped
        };

        // Create the base transaction (same as V2)
        let tx = self.create_private_transaction_impl(
            utxos_to_spend,
            outputs,
            fee,
            current_height,
            ledger,
        )?;

        Ok((tx, entropy_result))
    }

    /// Generate an entropy proof for a transaction.
    ///
    /// This creates a zero-knowledge proof that the entropy increase from
    /// inputs to outputs meets the minimum threshold for decay credit.
    ///
    /// # Arguments
    /// * `utxos` - Input UTXOs
    /// * `outputs` - Output TxOutputs (with tags already applied)
    /// * `decay_rate` - Decay rate applied to tags
    ///
    /// # Returns
    /// The entropy proof, or an error if generation fails.
    fn generate_entropy_proof(
        &self,
        utxos: &[Utxo],
        outputs: &[TxOutput],
        _decay_rate: u32,
    ) -> Result<EntropyProof> {
        let mut rng = OsRng;

        // Convert input UTXOs to CommittedTagVectorSecrets
        let input_secrets: Vec<CommittedTagVectorSecret> = utxos
            .iter()
            .map(|utxo| {
                Self::utxo_to_committed_tag_secret(utxo)
            })
            .collect();

        // Convert outputs to CommittedTagVectorSecrets (with decay applied)
        let output_secrets: Vec<CommittedTagVectorSecret> = outputs
            .iter()
            .map(|output| {
                Self::output_to_committed_tag_secret(output)
            })
            .collect();

        // Build the entropy proof
        let builder = EntropyProofBuilder::new(input_secrets, output_secrets);
        builder
            .prove(&mut rng)
            .ok_or_else(|| anyhow::anyhow!(
                "Entropy delta below threshold - transaction does not qualify for decay credit"
            ))
    }

    /// Convert a UTXO's tags to a CommittedTagVectorSecret.
    fn utxo_to_committed_tag_secret(utxo: &Utxo) -> CommittedTagVectorSecret {
        let mut tags = HashMap::new();
        for entry in &utxo.output.cluster_tags.entries {
            tags.insert(TaxClusterId(entry.cluster_id.0), entry.weight);
        }
        CommittedTagVectorSecret::from_plaintext(utxo.output.amount, &tags, &mut OsRng)
    }

    /// Convert a TxOutput's tags to a CommittedTagVectorSecret.
    fn output_to_committed_tag_secret(output: &TxOutput) -> CommittedTagVectorSecret {
        let mut tags = HashMap::new();
        for entry in &output.cluster_tags.entries {
            tags.insert(TaxClusterId(entry.cluster_id.0), entry.weight);
        }
        CommittedTagVectorSecret::from_plaintext(output.amount, &tags, &mut OsRng)
    }

    /// Create a quantum-private transaction for post-quantum security.
    ///
    /// Quantum-private transactions use hybrid classical + post-quantum
    /// cryptography:
    /// - Outputs: Classical stealth keys + ML-KEM-768 encapsulation
    /// - Inputs: Schnorr signature + ML-DSA-65 (Dilithium) signature
    ///
    /// This provides protection against "harvest now, decrypt later" attacks
    /// where adversaries archive blockchain data for future quantum
    /// cryptanalysis.
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
        let change = total_input
            .checked_sub(amount + fee)
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
        let preliminary_tx =
            QuantumPrivateTransaction::new(Vec::new(), outputs.clone(), fee, current_height);
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
            // We compute: shared_secret = SHA256("botho-pq-bridge" || target_key ||
            // public_key || view_private) This binds the PQ signature to the
            // specific output and the wallet's view key.
            let pq_shared_secret = {
                use sha2::{Digest, Sha256};
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
    use crate::transaction::UtxoId;
    use bth_transaction_types::{ClusterId, ClusterTagEntry};

    #[test]
    fn test_wallet_from_mnemonic() {
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        let wallet = Wallet::from_mnemonic(mnemonic).unwrap();
        let addr = wallet.default_address();
        // Just verify we get a valid address
        assert!(!addr.view_public_key().to_bytes().is_empty());
    }

    fn make_utxo(amount: u64, cluster_tags: ClusterTagVector) -> Utxo {
        Utxo {
            id: UtxoId::new([0u8; 32], 0),
            output: TxOutput {
                amount,
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                e_memo: None,
                cluster_tags,
            },
            created_at: 0,
        }
    }

    #[test]
    fn test_cluster_wealth_empty() {
        let utxos: Vec<Utxo> = vec![];
        assert_eq!(Wallet::compute_cluster_wealth(&utxos), 0);
    }

    #[test]
    fn test_cluster_wealth_single_cluster() {
        // Single UTXO with 100% in one cluster
        let tags = ClusterTagVector::single(ClusterId(42));
        let utxos = vec![make_utxo(1_000_000, tags)];

        // Wealth = 1_000_000 * 1_000_000 / 1_000_000 = 1_000_000
        assert_eq!(Wallet::compute_cluster_wealth(&utxos), 1_000_000);
    }

    #[test]
    fn test_cluster_wealth_multiple_utxos_same_cluster() {
        // Multiple UTXOs all in the same cluster
        let tags = ClusterTagVector::single(ClusterId(42));
        let utxos = vec![
            make_utxo(1_000_000, tags.clone()),
            make_utxo(2_000_000, tags.clone()),
            make_utxo(500_000, tags),
        ];

        // Total wealth = 1M + 2M + 0.5M = 3.5M
        assert_eq!(Wallet::compute_cluster_wealth(&utxos), 3_500_000);
    }

    #[test]
    fn test_cluster_wealth_multiple_clusters() {
        // UTXOs in different clusters - should return max
        let tags1 = ClusterTagVector::single(ClusterId(1));
        let tags2 = ClusterTagVector::single(ClusterId(2));
        let utxos = vec![
            make_utxo(1_000_000, tags1), // Cluster 1: 1M
            make_utxo(3_000_000, tags2), // Cluster 2: 3M
        ];

        // Max cluster wealth = 3M (cluster 2)
        assert_eq!(Wallet::compute_cluster_wealth(&utxos), 3_000_000);
    }

    #[test]
    fn test_cluster_wealth_partial_tags() {
        // UTXO with 50% in each of two clusters
        let mut tags = ClusterTagVector::empty();
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(1),
            weight: 500_000, // 50%
        });
        tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(2),
            weight: 500_000, // 50%
        });

        let utxos = vec![make_utxo(2_000_000, tags)];

        // Each cluster gets 2M * 50% = 1M
        assert_eq!(Wallet::compute_cluster_wealth(&utxos), 1_000_000);
    }

    #[test]
    fn test_inherited_tags_single_input() {
        let tags = ClusterTagVector::single(ClusterId(42));
        let utxos = vec![make_utxo(1_000_000, tags)];

        // No decay
        let inherited = Wallet::compute_inherited_tags(&utxos, 0);
        assert_eq!(inherited.get_weight(ClusterId(42)), TAG_WEIGHT_SCALE);

        // 10% decay
        let inherited = Wallet::compute_inherited_tags(&utxos, 100_000);
        assert_eq!(inherited.get_weight(ClusterId(42)), 900_000); // 90%
    }

    #[test]
    fn test_inherited_tags_multiple_inputs() {
        // Two equal-value inputs from different clusters
        let tags1 = ClusterTagVector::single(ClusterId(1));
        let tags2 = ClusterTagVector::single(ClusterId(2));
        let utxos = vec![make_utxo(1_000_000, tags1), make_utxo(1_000_000, tags2)];

        let inherited = Wallet::compute_inherited_tags(&utxos, 0);
        // Each cluster should have 50%
        assert_eq!(inherited.get_weight(ClusterId(1)), 500_000);
        assert_eq!(inherited.get_weight(ClusterId(2)), 500_000);
    }

    #[test]
    fn test_transaction_version_supports_entropy() {
        assert!(!TransactionVersion::V2.supports_entropy_proof());
        assert!(TransactionVersion::V3.supports_entropy_proof());
    }

    #[test]
    fn test_transaction_config_defaults() {
        let config = TransactionConfig::default();
        assert_eq!(config.version, TransactionVersion::V3);
        assert_eq!(config.decay_rate, DEFAULT_CLUSTER_DECAY_RATE);
        assert!(config.fallback_on_proof_failure);
    }

    #[test]
    fn test_transaction_config_v2() {
        let config = TransactionConfig::v2();
        assert_eq!(config.version, TransactionVersion::V2);
    }

    #[test]
    fn test_transaction_config_v3() {
        let config = TransactionConfig::v3();
        assert_eq!(config.version, TransactionVersion::V3);
    }

    #[test]
    fn test_estimate_transaction_size_v2() {
        // V2: 2 inputs, 2 outputs
        // Base: 100 + (2 * 700) + (2 * 100) + 500 = 2200
        let size_v2 = Wallet::estimate_transaction_size(2, 2, TransactionVersion::V2);
        assert_eq!(size_v2, 2200);
    }

    #[test]
    fn test_estimate_transaction_size_v3() {
        // V3: 2 inputs, 2 outputs + entropy proof
        // Base: 100 + (2 * 700) + (2 * 100) + 500 = 2200
        // + ENTROPY_PROOF_SIZE_ESTIMATE (1024) = 3224
        let size_v3 = Wallet::estimate_transaction_size(2, 2, TransactionVersion::V3);
        assert_eq!(size_v3, 2200 + ENTROPY_PROOF_SIZE_ESTIMATE);
    }

    #[test]
    fn test_estimate_fee_with_entropy_proof() {
        let tags = ClusterTagVector::single(ClusterId(42));
        let utxos = vec![make_utxo(500_000, tags)]; // Below 1M threshold

        let base_fee_per_byte = 10u64;

        // V2 fee (no entropy proof)
        let fee_v2 =
            Wallet::estimate_fee_with_entropy_proof(&utxos, 2, base_fee_per_byte, TransactionVersion::V2);
        // Size: 100 + 700 + 200 + 500 = 1500
        // Fee: 1500 * 10 * 1 (no multiplier for < 1M) = 15000
        assert_eq!(fee_v2, 15000);

        // V3 fee (with entropy proof)
        let fee_v3 =
            Wallet::estimate_fee_with_entropy_proof(&utxos, 2, base_fee_per_byte, TransactionVersion::V3);
        // Size: 1500 + 1024 = 2524
        // Fee: 2524 * 10 * 1 = 25240
        assert_eq!(fee_v3, 25240);
    }

    #[test]
    fn test_estimate_fee_with_high_wealth_multiplier() {
        // 10M wealth triggers 4x multiplier
        let tags = ClusterTagVector::single(ClusterId(42));
        let utxos = vec![make_utxo(10_000_000, tags)];

        let base_fee_per_byte = 10u64;

        let fee = Wallet::estimate_fee_with_entropy_proof(
            &utxos,
            2,
            base_fee_per_byte,
            TransactionVersion::V2,
        );

        // Size: 100 + 700 + 200 + 500 = 1500
        // Fee: 1500 * 10 * 4 (4x for >= 10M) = 60000
        assert_eq!(fee, 60000);
    }

    #[test]
    fn test_utxo_to_committed_tag_secret() {
        let tags = ClusterTagVector::single(ClusterId(42));
        let utxo = make_utxo(1_000_000, tags);

        let secret = Wallet::utxo_to_committed_tag_secret(&utxo);

        // Verify the secret has the correct total mass
        assert_eq!(secret.total_mass, 1_000_000);
    }
}
