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
use bth_transaction_clsag::{ClsagRingInput, RingMember, Transaction, TxOutput, MIN_RING_SIZE};
use bth_transaction_types::{ClusterId, ClusterTagEntry, ClusterTagVector};
use rand::{rngs::OsRng, seq::SliceRandom};
use serde::{Deserialize, Serialize};

use crate::{
    fee_estimation::StoredTags,
    keys::WalletKeys,
    ring_builder::fetch_decoy_ring_members,
    rpc_pool::{BlockOutputs, RpcPool},
};

// The real transaction format lives in `bth-transaction-clsag` (the same crate
// the node exposes as `botho::transaction`). Re-export the consensus fee floor
// so callers (e.g. `commands::send`) can enforce it without importing the crate
// directly.
pub use bth_transaction_clsag::MIN_TX_FEE;

#[cfg(feature = "pq")]
use bth_crypto_pq::MlKem768Ciphertext;
#[cfg(feature = "pq")]
use bth_crypto_ring_signature::pq_onetime_keys::check_pq_output_ownership;
#[cfg(feature = "pq")]
use sha2::{Digest, Sha256};

/// Picocredits per CAD
pub const PICOCREDITS_PER_CAD: u64 = 1_000_000_000_000;

/// Minimum transaction fee (legacy display constant).
///
/// The consensus-enforced floor is [`MIN_TX_FEE`] (100_000_000 picocredits);
/// transaction building must never submit a fee below it. This smaller value is
/// retained only for backward-compatible display code.
pub const MIN_FEE: u64 = 1_000_000; // 0.000001 CAD

/// Cluster-tag decay rate for output inheritance.
///
/// Matches the node's `DEFAULT_CLUSTER_DECAY_RATE` so outputs built by the CLI
/// wallet inherit ancestry identically to node-built transactions.
pub const DEFAULT_CLUSTER_DECAY_RATE: u32 = 50_000;

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

// NOTE: The wallet's own flat `botho-tx-v1` Transaction/TxInput/TxOutput types
// (with cryptographically-broken stealth outputs) have been replaced by the
// real CLSAG format from `bth-transaction-clsag`, imported at the top of this
// module. The legacy types are quarantined in `transaction_legacy.rs` per
// CLAUDE.md code-preservation. See issue #614.

/// Convert wallet-local [`StoredTags`] into the on-chain [`ClusterTagVector`]
/// carried by outputs. Cluster IDs and parts-per-million weights map 1:1.
fn stored_tags_to_cluster_vector(tags: &StoredTags) -> ClusterTagVector {
    let entries = tags
        .tags
        .iter()
        .map(|&(id, weight)| ClusterTagEntry {
            cluster_id: ClusterId(id),
            weight,
        })
        .collect();
    ClusterTagVector {
        entries,
        decay_state: None,
    }
}

/// Compute the merged, decayed cluster-tag vector inherited by all outputs of a
/// transaction that spends `selected`. Mirrors the node's
/// `Wallet::compute_inherited_tags` (amount-weighted merge at
/// [`DEFAULT_CLUSTER_DECAY_RATE`]).
fn inherited_cluster_tags(selected: &[OwnedUtxo]) -> ClusterTagVector {
    let inputs: Vec<(ClusterTagVector, u64)> = selected
        .iter()
        .map(|u| (stored_tags_to_cluster_vector(&u.tags()), u.amount))
        .collect();
    ClusterTagVector::merge_weighted(&inputs, DEFAULT_CLUSTER_DECAY_RATE)
}

/// Serialize a signed transaction to hex for submission via `tx_submit`.
///
/// Uses the same bincode encoding the node's `tx_submit` handler deserializes
/// into `botho::transaction::Transaction`.
pub fn to_tx_hex(tx: &Transaction) -> Result<String> {
    let bytes = bincode::serialize(tx).map_err(|e| anyhow!("Failed to serialize tx: {}", e))?;
    Ok(hex::encode(bytes))
}

/// Result of building a transfer transaction.
///
/// Contains the transaction and metadata needed for cluster tag propagation.
#[derive(Debug, Clone)]
pub struct TransferResult {
    /// The signed transaction ready for submission.
    pub transaction: Transaction,
    /// The actual fee (may be higher than requested if dust was absorbed).
    pub actual_fee: u64,
    /// The change output's public key, if a change output was created.
    /// This is used to track pending cluster tags for the change output.
    pub change_output_public_key: Option<[u8; 32]>,
    /// UTXOs that were selected as inputs for this transaction.
    /// Needed for computing blended cluster tags for the change output.
    pub selected_inputs: Vec<OwnedUtxo>,
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

    /// Build and sign a CLSAG transaction (fetches decoy ring members via RPC).
    ///
    /// If the change amount is below `DUST_THRESHOLD`, it is added to the fee
    /// instead of creating an unspendable output.
    ///
    /// Returns just the transaction for simple use cases. For cluster tag
    /// propagation, use `build_transfer_with_metadata` instead.
    pub async fn build_transfer(
        &self,
        rpc: &mut RpcPool,
        recipient: &PublicAddress,
        amount: u64,
        fee: u64,
    ) -> Result<Transaction> {
        let result = self
            .build_transfer_with_metadata(rpc, recipient, amount, fee)
            .await?;
        Ok(result.transaction)
    }

    /// Build and sign a CLSAG transaction with full metadata.
    ///
    /// This is the live send path. For each selected input it fetches an
    /// age-similar decoy ring over RPC (`chain_getOutputs`), assembles a
    /// ring-size-20 CLSAG ring with the real input at a randomized position,
    /// and produces a transaction that bincode-round-trips through
    /// `botho::transaction::Transaction`.
    ///
    /// Returns `TransferResult` containing:
    /// - The signed transaction
    /// - The actual fee (may be higher if dust was absorbed)
    /// - The change output's public key (for cluster tag tracking, issue #249)
    /// - The selected input UTXOs (for computing blended tags)
    pub async fn build_transfer_with_metadata(
        &self,
        rpc: &mut RpcPool,
        recipient: &PublicAddress,
        amount: u64,
        fee: u64,
    ) -> Result<TransferResult> {
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

        // Exclude our own inputs from every ring's decoy pool.
        let exclude_keys: Vec<[u8; 32]> = selected.iter().map(|u| u.target_key).collect();

        // Fetch an age-similar decoy ring for each selected input. This is the
        // only asynchronous step; the CLSAG assembly below is deterministic
        // given the fetched rings, which keeps it unit-testable in isolation.
        let decoys_needed = MIN_RING_SIZE - 1;
        let mut decoy_rings: Vec<Vec<RingMember>> = Vec::with_capacity(selected.len());
        for utxo in &selected {
            let real_age = self.sync_height.saturating_sub(utxo.created_at);
            let decoys = fetch_decoy_ring_members(
                rpc,
                real_age,
                self.sync_height,
                &exclude_keys,
                decoys_needed,
            )
            .await?;
            decoy_rings.push(decoys);
        }

        self.build_signed_transaction(
            recipient,
            amount,
            fee,
            selected,
            total_selected,
            decoy_rings,
        )
    }

    /// Assemble and CLSAG-sign the transaction from pre-fetched decoy rings.
    ///
    /// Split out from the RPC decoy fetch so the full crypto assembly can be
    /// exercised in tests with fixture UTXOs and ring members (the live path
    /// sources `decoy_rings` from [`Self::build_transfer_with_metadata`]).
    ///
    /// `decoy_rings[i]` supplies the decoys for `selected[i]`; each must hold
    /// at least `MIN_RING_SIZE - 1` members.
    pub fn build_signed_transaction(
        &self,
        recipient: &PublicAddress,
        amount: u64,
        fee: u64,
        selected: Vec<OwnedUtxo>,
        total_selected: u64,
        decoy_rings: Vec<Vec<RingMember>>,
    ) -> Result<TransferResult> {
        if decoy_rings.len() != selected.len() {
            return Err(anyhow!(
                "internal error: {} decoy rings for {} inputs",
                decoy_rings.len(),
                selected.len()
            ));
        }

        let total_needed = amount
            .checked_add(fee)
            .ok_or_else(|| anyhow!("Amount overflow"))?;
        let change = total_selected
            .checked_sub(total_needed)
            .ok_or_else(|| anyhow!("Insufficient funds"))?;

        // All outputs inherit the same merged+decayed cluster-tag vector from
        // the inputs (matches the node's create_private_transaction).
        let inherited_tags = inherited_cluster_tags(&selected);

        // Recipient output (real stealth address via Ristretto DH).
        let mut outputs = vec![TxOutput::new_with_cluster_tags(
            amount,
            recipient,
            None,
            inherited_tags.clone(),
        )];

        // Change: if above dust, create a change output back to ourselves;
        // otherwise absorb the dust into the fee to avoid an unspendable UTXO.
        let (actual_fee, change_output_public_key) = if change >= DUST_THRESHOLD {
            let change_output = TxOutput::new_with_cluster_tags(
                change,
                &self.keys.public_address(),
                None,
                inherited_tags.clone(),
            );
            let change_key = change_output.public_key;
            outputs.push(change_output);
            (fee, Some(change_key))
        } else {
            (fee + change, None)
        };

        // Preliminary tx (no inputs) yields the signing hash bound by all
        // outputs, the fee, and the height — the CLSAG message.
        let preliminary =
            Transaction::new_clsag(Vec::new(), outputs.clone(), actual_fee, self.sync_height);
        let signing_hash = preliminary.signing_hash();

        let account_key = self.keys.account_key();
        let mut rng = OsRng;
        let mut ring_inputs = Vec::with_capacity(selected.len());

        for (utxo, decoys) in selected.iter().zip(decoy_rings.into_iter()) {
            if decoys.len() < MIN_RING_SIZE - 1 {
                return Err(anyhow!(
                    "insufficient decoys for ring: need {}, have {}",
                    MIN_RING_SIZE - 1,
                    decoys.len()
                ));
            }

            // Recover the one-time private key for this UTXO.
            let onetime_private = utxo.recover_spend_key(account_key).ok_or_else(|| {
                anyhow!(
                    "Failed to recover spend key for UTXO {}",
                    hex::encode(&utxo.tx_hash[0..8])
                )
            })?;

            // Real ring member: transparent commitment over the spent amount.
            let real_output = TxOutput {
                amount: utxo.amount,
                target_key: utxo.target_key,
                public_key: utxo.public_key,
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
                kem_ciphertext: None,
            };
            let real_member = RingMember::from_output(&real_output);

            // Assemble ring = real + decoys, then randomize the real input's
            // position (CLSAG requires the signer index to be secret).
            let mut ring: Vec<RingMember> = Vec::with_capacity(MIN_RING_SIZE);
            ring.push(real_member);
            ring.extend(decoys.into_iter().take(MIN_RING_SIZE - 1));
            ring.shuffle(&mut rng);

            let real_index = ring
                .iter()
                .position(|m| m.target_key == utxo.target_key)
                .ok_or_else(|| anyhow!("internal error: real input missing from ring"))?;

            let ring_input = ClsagRingInput::new(
                ring,
                real_index,
                &onetime_private,
                utxo.amount,
                &signing_hash,
                &mut rng,
            )
            .map_err(|e| anyhow!("Failed to create ring signature: {}", e))?;

            ring_inputs.push(ring_input);
        }

        let tx = Transaction::new_clsag(ring_inputs, outputs, actual_fee, self.sync_height);

        Ok(TransferResult {
            transaction: tx,
            actual_fee,
            change_output_public_key,
            selected_inputs: selected,
        })
    }

    /// Select input UTXOs to cover `target` using a largest-first algorithm.
    ///
    /// Returns the selected UTXOs and their total value. Exposed for tests and
    /// callers that want to preview which inputs a transfer would spend.
    pub fn select_inputs(&self, target: u64) -> Result<(Vec<OwnedUtxo>, u64)> {
        self.select_utxos(target)
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
                    // Convert cluster tags from RPC format to StoredTags
                    let cluster_tags = Self::parse_cluster_tags(&output.cluster_tags);

                    owned.push(OwnedUtxo {
                        tx_hash,
                        output_index: output.output_index,
                        amount,
                        created_at: block.height,
                        target_key,
                        public_key,
                        subaddress_index,
                        cluster_tags,
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

    /// Parse cluster tags from RPC format to StoredTags.
    ///
    /// RPC returns cluster tags as an array of [cluster_id, weight] pairs.
    /// Returns Some(StoredTags) if any tags are present, None otherwise.
    fn parse_cluster_tags(rpc_tags: &[[u64; 2]]) -> Option<StoredTags> {
        if rpc_tags.is_empty() {
            return None;
        }

        // Convert [cluster_id, weight] pairs to (u64, u32) tuples
        let tags: Vec<(u64, u32)> = rpc_tags
            .iter()
            .map(|&[id, weight]| (id, weight as u32))
            .collect();

        Some(StoredTags { tags })
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

/// Apply pending change tags to discovered UTXOs.
///
/// When a change output is discovered during sync, this function looks up
/// its pending tags (stored during transaction creation) and applies them.
/// This ensures cluster attribution properly propagates through change outputs.
///
/// Returns true if any pending tags were applied (indicating the pending tags
/// structure should be saved).
pub fn apply_pending_change_tags(
    utxos: &mut [OwnedUtxo],
    pending_tags: &mut crate::fee_estimation::PendingChangeTags,
) -> bool {
    let mut applied_any = false;

    for utxo in utxos.iter_mut() {
        // Look up pending tags by output public key
        if let Some(tags) = pending_tags.find_and_remove(&utxo.public_key) {
            utxo.cluster_tags = Some(tags);
            applied_any = true;
        }
    }

    applied_any
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

    // ------------------------------------------------------------------
    // CLSAG transaction-building tests (issue #614).
    //
    // These drive the real crypto path: fixture UTXOs owned by a wallet plus
    // fixture decoy ring members are assembled into a signed CLSAG tx via
    // `build_signed_transaction` (the RPC decoy fetch is bypassed so the
    // assembly is deterministic and CI-runnable). Live-testnet send is
    // deferred to the operator.
    // ------------------------------------------------------------------

    const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
    // A distinct 24-word phrase for the recipient wallet.
    const RECIPIENT_MNEMONIC: &str = "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title";

    /// Create a UTXO that `keys` genuinely owns, by generating a real stealth
    /// output to the wallet's default subaddress and capturing its keys.
    fn owned_fixture_utxo(keys: &WalletKeys, amount: u64, created_at: u64) -> OwnedUtxo {
        let out = TxOutput::new(amount, &keys.public_address());
        let utxo = OwnedUtxo {
            tx_hash: [9u8; 32],
            output_index: 0,
            amount,
            created_at,
            target_key: out.target_key,
            public_key: out.public_key,
            subaddress_index: 0,
            cluster_tags: None,
        };
        // Sanity: the wallet must be able to recover the spend key.
        assert!(
            utxo.recover_spend_key(keys.account_key()).is_some(),
            "fixture UTXO must be spendable by the wallet"
        );
        utxo
    }

    /// Build a ring of `n` random-but-valid decoy members.
    fn fixture_decoys(n: usize) -> Vec<RingMember> {
        let decoy_keys = WalletKeys::from_mnemonic(RECIPIENT_MNEMONIC).unwrap();
        (0..n)
            .map(|i| {
                let out = TxOutput::new(1_000 + i as u64, &decoy_keys.public_address());
                RingMember::from_output(&out)
            })
            .collect()
    }

    #[test]
    fn test_build_signed_transaction_roundtrips_and_verifies() {
        use bth_transaction_clsag::Transaction as ClsagTx;

        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let recipient_keys = WalletKeys::from_mnemonic(RECIPIENT_MNEMONIC).unwrap();
        let recipient = recipient_keys.public_address();

        let input_amount = 5_000_000_000_000u64; // 5 CAD
        let send_amount = 1_000_000_000_000u64; // 1 CAD
        let fee = MIN_TX_FEE;

        let utxo = owned_fixture_utxo(&keys, input_amount, 50);
        let builder = TransactionBuilder::new(keys.clone(), vec![utxo.clone()], 1_000);

        let decoy_rings = vec![fixture_decoys(MIN_RING_SIZE - 1)];
        let result = builder
            .build_signed_transaction(
                &recipient,
                send_amount,
                fee,
                vec![utxo],
                input_amount,
                decoy_rings,
            )
            .expect("build should succeed");

        let tx = &result.transaction;
        // Ring size is exactly MIN_RING_SIZE (20).
        assert_eq!(tx.inputs.len(), 1);
        assert_eq!(tx.inputs.clsag()[0].ring.len(), MIN_RING_SIZE);

        // Structure and balance/signature verification pass.
        assert!(tx.is_valid_structure().is_ok(), "structure must be valid");
        assert!(
            tx.verify_ring_signatures().is_ok(),
            "CLSAG ring signatures must verify"
        );

        // Bincode round-trip through the node's transaction type.
        let hex = to_tx_hex(tx).expect("serialize");
        let bytes = hex::decode(&hex).expect("hex decode");
        let decoded: ClsagTx = bincode::deserialize(&bytes).expect("deserialize as node tx");
        assert!(
            decoded.verify_ring_signatures().is_ok(),
            "round-tripped tx must still verify"
        );
        assert_eq!(decoded.fee, tx.fee);
        assert_eq!(decoded.outputs.len(), tx.outputs.len());
    }

    #[test]
    fn test_output_is_detectable_by_recipient() {
        // The exact bug class the curator found: outputs must be detectable by
        // the recipient's stealth-address scanner.
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let recipient_keys = WalletKeys::from_mnemonic(RECIPIENT_MNEMONIC).unwrap();
        let recipient = recipient_keys.public_address();

        let input_amount = 5_000_000_000_000u64;
        let send_amount = 1_000_000_000_000u64;

        let utxo = owned_fixture_utxo(&keys, input_amount, 50);
        let builder = TransactionBuilder::new(keys.clone(), vec![utxo.clone()], 1_000);
        let decoy_rings = vec![fixture_decoys(MIN_RING_SIZE - 1)];

        let result = builder
            .build_signed_transaction(
                &recipient,
                send_amount,
                MIN_TX_FEE,
                vec![utxo],
                input_amount,
                decoy_rings,
            )
            .expect("build should succeed");

        // The first output is the recipient output. The recipient's scanner
        // must detect ownership via the DH stealth protocol.
        let recipient_scanner = WalletScanner::new(&recipient_keys);
        let out = &result.transaction.outputs[0];
        assert_eq!(out.amount, send_amount);
        let owned = recipient_scanner.check_ownership(&out.target_key, &out.public_key);
        assert_eq!(
            owned,
            Some(0),
            "recipient must detect the output on their default subaddress"
        );

        // And the *sender* must NOT detect the recipient's output as their own.
        let sender_scanner = WalletScanner::new(&keys);
        assert_eq!(
            sender_scanner.check_ownership(&out.target_key, &out.public_key),
            None,
            "sender must not own the recipient's output"
        );

        // The change output (if any) must be detectable by the sender.
        if let Some(change_key) = result.change_output_public_key {
            let change_out = result
                .transaction
                .outputs
                .iter()
                .find(|o| o.public_key == change_key)
                .expect("change output present");
            assert!(
                sender_scanner
                    .check_ownership(&change_out.target_key, &change_out.public_key)
                    .is_some(),
                "sender must detect their own change output"
            );
        }
    }

    #[tokio::test]
    async fn test_young_input_rejected_cleanly() {
        // A UTXO younger than MIN_DECOY_AGE_BLOCKS must produce a clean,
        // user-facing error (no panic) before any ring is built. We reach the
        // young-input guard via the fetch path without needing a live node by
        // constructing the decoy fetch directly.
        use crate::ring_builder::fetch_decoy_ring_members;

        // real_age = 5 (< 10) — guard must fire before any RPC call, so passing
        // a placeholder RpcPool is never dereferenced. We assert the guard by
        // calling the (RPC-independent) prefix through a helper that mirrors the
        // build path: real_age computed from a fresh UTXO.
        let keys = WalletKeys::from_mnemonic(TEST_MNEMONIC).unwrap();
        let _utxo = owned_fixture_utxo(&keys, 5_000_000_000_000, 995); // height 1000 - 995 = age 5

        // The guard lives at the top of fetch_decoy_ring_members and returns
        // before touching `rpc`, but Rust still requires a &mut RpcPool. Build a
        // disconnected pool; the guard returns first.
        let mut rpc = RpcPool::new(crate::discovery::NodeDiscovery::new());
        let err = fetch_decoy_ring_members(&mut rpc, 5, 1_000, &[], MIN_RING_SIZE - 1)
            .await
            .expect_err("young input must error");
        let msg = err.to_string();
        assert!(
            msg.contains("too new") && msg.contains("confirmations"),
            "expected a clean young-input message, got: {msg}"
        );
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
                    cluster_tags: vec![],
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
                    cluster_tags: vec![],
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
