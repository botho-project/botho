// Copyright (c) 2024 Botho Foundation

//! Transaction validation for consensus.
//!
//! This module provides separate validation logic for:
//! - Minting transactions (PoW-based coinbase rewards)
//! - Transfer transactions (UTXO-based value transfers)

use crate::{
    block::{calculate_block_reward, MintingTx},
    ledger::ChainState,
    network::PROTOCOL_VERSION,
    transaction::Transaction,
};
use std::sync::{Arc, RwLock};
use tracing::{debug, warn};

/// ML-KEM-768 encapsulation ciphertext length in bytes (FIPS 203).
///
/// Mirrors `bth_crypto_pq::ML_KEM_768_CIPHERTEXT_BYTES`, redeclared here so the
/// consensus enforcement compiles under `--no-default-features` (the
/// `bth-crypto-pq` crate is an optional, `pq`-gated dependency). Covered by a
/// cross-check test against the crypto crate when the `pq` feature is on.
pub const ML_KEM_768_CIPHERTEXT_BYTES: usize = 1088;

/// Protocol epoch (major version) at which universal ML-KEM enforcement
/// activates — the fresh-genesis 6.0.0 reset that ships hybrid stealth on every
/// output (ADR 0008, #954/#958).
const KEM_ENFORCEMENT_PROTOCOL_MAJOR: u64 = 6;

/// Parse the major component of a semantic-version string in a `const` context.
const fn protocol_major(version: &str) -> u64 {
    let bytes = version.as_bytes();
    let mut i = 0;
    let mut major = 0u64;
    while i < bytes.len() {
        let b = bytes[i];
        if b < b'0' || b > b'9' {
            break;
        }
        major = major * 10 + (b - b'0') as u64;
        i += 1;
    }
    major
}

/// Whether consensus REQUIRES a valid hybrid ML-KEM ciphertext on every
/// value-transfer, minting-reward, and lottery-payout output.
///
/// This is a **compile-time constant** — a pure function of two constants baked
/// into the binary, evaluated with the transaction value — NOT a runtime read
/// of the chain tip. That property is load-bearing: [`validate_transfer_tx`]
/// and [`validate_minting_tx_intrinsic`] are the SCP consensus validity gates,
/// whose no-fork invariant requires validity to be a pure function of the value
/// (issues #419 / #451). A tip-relative gate here could partition the quorum.
///
/// Enforcement is active iff BOTH hold:
/// - the binary is built with post-quantum crypto (`pq`, on by default). A
///   `--no-default-features` build has no lattice stack, produces classical
///   outputs, and therefore must NOT reject them; and
/// - the protocol epoch is >= 6.0.0 (the fresh-genesis reset that ships
///   universal ML-KEM). A pre-reset binary (`PROTOCOL_VERSION` < 6.0.0) runs
///   the classical chain and never enforces, so a node briefly running this
///   code before the reset cannot reject its own legitimate ciphertext-less
///   outputs. This is the "protocol epoch" cutover gate (issue #973).
pub const KEM_CIPHERTEXT_ENFORCED: bool =
    cfg!(feature = "pq") && protocol_major(PROTOCOL_VERSION) >= KEM_ENFORCEMENT_PROTOCOL_MAJOR;

/// Enforce that a stealth output carries a valid hybrid ML-KEM ciphertext.
///
/// A no-op when [`KEM_CIPHERTEXT_ENFORCED`] is false (pre-reset / no-PQ
/// binaries). Otherwise the ciphertext must be present and exactly
/// [`ML_KEM_768_CIPHERTEXT_BYTES`] long. The ciphertext's *cryptographic*
/// validity is not (and cannot be) checked here — only the recipient holding
/// the ML-KEM secret can decapsulate — so structural presence + length is the
/// consensus-enforceable invariant.
fn check_kem_ciphertext(kem_ciphertext: &Option<Vec<u8>>) -> Result<(), ValidationError> {
    if !KEM_CIPHERTEXT_ENFORCED {
        return Ok(());
    }
    match kem_ciphertext {
        Some(ct) if ct.len() == ML_KEM_768_CIPHERTEXT_BYTES => Ok(()),
        Some(ct) => Err(ValidationError::InvalidKemCiphertext { len: ct.len() }),
        None => Err(ValidationError::MissingKemCiphertext),
    }
}

/// Validation errors for transactions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    // Minting transaction errors
    InvalidPoW,
    WrongPrevBlockHash,
    WrongBlockHeight,
    WrongDifficulty,
    WrongReward {
        expected: u64,
        got: u64,
    },
    TimestampTooFarInFuture,
    TimestampBeforeParent,

    // Transfer transaction errors
    NoInputs,
    NoOutputs,
    ZeroAmountOutput,
    DuplicateKeyImage,
    InputNotFound,
    InputAlreadySpent,
    InvalidSignature,
    InsufficientFunds {
        input: u64,
        output: u64,
        fee: u64,
    },
    StaleTransaction,

    // Universal ML-KEM enforcement errors (6.0.0, #958/#973)
    /// A value-transfer, minting-reward, or lottery-payout output required a
    /// hybrid ML-KEM ciphertext but none was present.
    MissingKemCiphertext,
    /// A stealth output's ML-KEM ciphertext was present but not the required
    /// [`ML_KEM_768_CIPHERTEXT_BYTES`] length.
    InvalidKemCiphertext {
        len: usize,
    },

    // General errors
    DeserializationFailed(String),
    ChainStateUnavailable,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPoW => write!(f, "Invalid proof of work"),
            Self::WrongPrevBlockHash => write!(f, "Wrong previous block hash"),
            Self::WrongBlockHeight => write!(f, "Wrong block height"),
            Self::WrongDifficulty => write!(f, "Wrong difficulty target"),
            Self::WrongReward { expected, got } => {
                write!(f, "Wrong reward: expected {}, got {}", expected, got)
            }
            Self::TimestampTooFarInFuture => write!(f, "Timestamp too far in future"),
            Self::TimestampBeforeParent => write!(f, "Timestamp before parent block"),
            Self::NoInputs => write!(f, "Transaction has no inputs"),
            Self::NoOutputs => write!(f, "Transaction has no outputs"),
            Self::ZeroAmountOutput => write!(f, "Transaction has zero-amount output"),
            Self::DuplicateKeyImage => write!(f, "Transaction has duplicate key images"),
            Self::InputNotFound => write!(f, "Input UTXO not found"),
            Self::InputAlreadySpent => write!(f, "Input already spent"),
            Self::InvalidSignature => write!(f, "Invalid signature"),
            Self::InsufficientFunds { input, output, fee } => {
                write!(
                    f,
                    "Insufficient funds: input {} < output {} + fee {}",
                    input, output, fee
                )
            }
            Self::StaleTransaction => write!(f, "Transaction is stale"),
            Self::MissingKemCiphertext => {
                write!(f, "Output is missing its required ML-KEM ciphertext")
            }
            Self::InvalidKemCiphertext { len } => write!(
                f,
                "Output ML-KEM ciphertext has wrong length: {} (expected {})",
                len, ML_KEM_768_CIPHERTEXT_BYTES
            ),
            Self::DeserializationFailed(e) => write!(f, "Deserialization failed: {}", e),
            Self::ChainStateUnavailable => write!(f, "Chain state unavailable"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// Maximum allowed timestamp drift from current time (2 hours)
const MAX_FUTURE_TIMESTAMP_SECS: u64 = 2 * 60 * 60;

/// Transaction validator with access to chain state
pub struct TransactionValidator {
    /// Current chain state (shared with ledger)
    chain_state: Arc<RwLock<ChainState>>,
}

impl TransactionValidator {
    /// Create a new transaction validator
    pub fn new(chain_state: Arc<RwLock<ChainState>>) -> Self {
        Self { chain_state }
    }

    /// Validate the *intrinsic* properties of a minting transaction — those
    /// that are a pure function of the transaction itself and do NOT depend on
    /// the local chain tip.
    ///
    /// This is the validity check that SCP consensus uses (see
    /// [`validate_from_bytes_intrinsic`](Self::validate_from_bytes_intrinsic)).
    ///
    /// # SAFETY — why this MUST be tip-agnostic (issue #419 / #417 Finding 1)
    ///
    /// SCP's agreement (no-fork) theorem requires validity to be a *pure
    /// function of the value*: a value that is valid for one honest node must
    /// be valid for all honest nodes. SCP silently DROPS any peer message that
    /// carries a value the local node cannot validate (`slot.rs`
    /// `handle_messages`), never entering it into `self.M`. If validity
    /// depended on the local tip, then under the fast-slot PoW race two
    /// minters would each drop the peer's value as "invalid against my
    /// tip", partition the quorum into two single-node voting instances,
    /// and each externalize its OWN block — a fork at the same height.
    ///
    /// Therefore the only checks here are ones that every honest node agrees on
    /// regardless of which tip it currently holds:
    /// - structural well-formedness (implicit: deserialized `MintingTx`),
    /// - PoW solution meets the difficulty *stated in the tx itself*
    ///   (`verify_pow` hashes the tx's own fields against `tx.difficulty`),
    /// - timestamp is not absurdly far in the future (a wall-clock bound, not a
    ///   tip-relative bound; honest nodes share approximately the same clock).
    ///
    /// The tip-relative checks (`prev_block_hash == tip`, `height == tip + 1`,
    /// `difficulty == chain difficulty`, `reward == emission(height,
    /// total_mined)`, `timestamp >= parent timestamp`) are NOT performed here.
    /// They are enforced unconditionally at block-apply time in
    /// `LedgerStore::add_block`, so a genuinely stale or fraudulent block can
    /// never be appended to the ledger even though its minting tx is a valid
    /// consensus *value*.
    pub fn validate_minting_tx_intrinsic(tx: &MintingTx) -> Result<(), ValidationError> {
        debug!(
            height = tx.block_height,
            "Validating minting transaction (intrinsic / tip-agnostic)"
        );

        // Universal ML-KEM (6.0.0, #958/#973): the coinbase reward output must
        // carry a valid hybrid ML-KEM ciphertext (encapsulated to the minter's
        // own published KEM key). A pure structural property of the value, so it
        // holds the SCP no-fork invariant. Gated by [`KEM_CIPHERTEXT_ENFORCED`].
        check_kem_ciphertext(&tx.kem_ciphertext)?;

        // Timestamp must not be absurdly far in the future. This is a bound on
        // a property of the value relative to wall-clock time, NOT relative to
        // the local chain tip, so all honest nodes agree on it (within clock
        // skew). The tip-relative monotonicity check (timestamp >= parent) is
        // deferred to block-apply.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|_| {
                warn!("System time before UNIX epoch - cannot validate timestamps");
                ValidationError::ChainStateUnavailable
            })?;

        if tx.timestamp > now + MAX_FUTURE_TIMESTAMP_SECS {
            warn!(
                timestamp = tx.timestamp,
                now = now,
                "Minting tx timestamp too far in future"
            );
            return Err(ValidationError::TimestampTooFarInFuture);
        }

        // Verify PoW against the difficulty STATED IN THE TX (not the local
        // chain difficulty). `verify_pow` hashes only the tx's own fields, so
        // this is a pure function of the value. The check that the stated
        // difficulty equals the chain-expected difficulty is tip-relative and
        // is enforced at block-apply.
        if !tx.verify_pow() {
            warn!("Minting tx failed intrinsic PoW verification");
            return Err(ValidationError::InvalidPoW);
        }

        debug!(
            height = tx.block_height,
            "Minting transaction passed intrinsic validation"
        );
        Ok(())
    }

    /// Validate a minting transaction against the local chain tip.
    ///
    /// This performs BOTH the intrinsic checks
    /// ([`validate_minting_tx_intrinsic`](Self::validate_minting_tx_intrinsic))
    /// and the tip-relative checks. It is used by the gossip-ingest path
    /// (which only registers a peer minting tx that already builds on our tip)
    /// and is retained for completeness/testing. It MUST NOT be used as the
    /// SCP consensus validity function — see
    /// [`validate_minting_tx_intrinsic`](Self::validate_minting_tx_intrinsic).
    pub fn validate_minting_tx(&self, tx: &MintingTx) -> Result<(), ValidationError> {
        let state = self
            .chain_state
            .read()
            .map_err(|_| ValidationError::ChainStateUnavailable)?;

        debug!(height = tx.block_height, "Validating minting transaction");

        // Check cheap validations first before expensive PoW verification

        // 0. Universal ML-KEM (6.0.0, #958/#973): the coinbase reward output must
        // carry a valid hybrid ML-KEM ciphertext. Gated by the compile-time
        // [`KEM_CIPHERTEXT_ENFORCED`] cutover constant; a no-op on pre-reset /
        // no-PQ binaries.
        check_kem_ciphertext(&tx.kem_ciphertext)?;

        // 1. Check prev_block_hash matches current chain tip
        if tx.prev_block_hash != state.tip_hash {
            warn!(
                expected = hex::encode(&state.tip_hash[0..8]),
                got = hex::encode(&tx.prev_block_hash[0..8]),
                "Minting tx has wrong prev_block_hash"
            );
            return Err(ValidationError::WrongPrevBlockHash);
        }

        // 2. Check block_height is next expected
        let expected_height = state.height + 1;
        if tx.block_height != expected_height {
            warn!(
                expected = expected_height,
                got = tx.block_height,
                "Minting tx has wrong block height"
            );
            return Err(ValidationError::WrongBlockHeight);
        }

        // 3. Check difficulty matches current network difficulty
        if tx.difficulty != state.difficulty {
            warn!(
                expected = state.difficulty,
                got = tx.difficulty,
                "Minting tx has wrong difficulty"
            );
            return Err(ValidationError::WrongDifficulty);
        }

        // 4. Check reward matches block-based emission schedule
        // Block reward is calculated from height and total supply using
        // MonetaryPolicy with 5s block assumption.
        let expected_reward = calculate_block_reward(tx.block_height, state.total_mined);
        if tx.reward != expected_reward {
            warn!(
                expected = expected_reward,
                got = tx.reward,
                "Minting tx has wrong reward"
            );
            return Err(ValidationError::WrongReward {
                expected: expected_reward,
                got: tx.reward,
            });
        }

        // 5. Check timestamp is reasonable
        // Don't fallback to 0 on error - that would bypass future timestamp checks
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|_| {
                warn!("System time before UNIX epoch - cannot validate timestamps");
                ValidationError::ChainStateUnavailable
            })?;

        if tx.timestamp > now + MAX_FUTURE_TIMESTAMP_SECS {
            warn!(
                timestamp = tx.timestamp,
                now = now,
                "Minting tx timestamp too far in future"
            );
            return Err(ValidationError::TimestampTooFarInFuture);
        }

        // Check timestamp is not before parent block (monotonicity)
        if tx.timestamp < state.tip_timestamp {
            warn!(
                timestamp = tx.timestamp,
                parent_timestamp = state.tip_timestamp,
                "Minting tx timestamp before parent block"
            );
            return Err(ValidationError::TimestampBeforeParent);
        }

        // 6. Verify PoW (hash < difficulty) - expensive, so do last
        if !tx.verify_pow() {
            warn!("Minting tx failed PoW verification");
            return Err(ValidationError::InvalidPoW);
        }

        debug!(
            height = tx.block_height,
            "Minting transaction validated successfully"
        );
        Ok(())
    }

    /// Validate a transfer transaction's INTRINSIC (tip-agnostic) structure.
    ///
    /// SAFETY / no-fork invariant: this function is the SCP consensus validity
    /// gate for transfer values (via
    /// [`validate_from_bytes_intrinsic`](Self::validate_from_bytes_intrinsic)).
    /// SCP's agreement (no-fork) theorem requires validity to be a *pure
    /// function of the value*: a value valid for one honest node must be valid
    /// for all honest nodes. SCP silently DROPS any peer message carrying a
    /// value the local node cannot validate, so a tip-dependent check here can
    /// partition the quorum (issue #451; the same #417/#419 condition the
    /// minting `*_intrinsic` split fixed).
    ///
    /// Therefore the only checks here are ones every honest node agrees on
    /// regardless of which tip it currently holds:
    /// - inputs are non-empty,
    /// - outputs are non-empty,
    /// - no output has a zero amount.
    ///
    /// The former tip-relative staleness check (`created_at_height + MAX_TX_AGE
    /// < state.height`, removed in #451) is NOT performed here, because
    /// `state.height` is the local tip and two honest nodes straddling the
    /// boundary would disagree on validity. Stale-tx handling now lives where
    /// it cannot fork or halt the chain:
    /// - the mempool evicts old txs by wall-clock (`mempool.rs`
    ///   `MAX_TX_AGE_SECS`, ~1h), so stale txs are never proposed as values;
    /// - full UTXO existence / double-spend / signature checks happen at
    ///   mempool-admission and block-apply time, which have ledger access.
    ///
    /// This is a pure function of the transaction; it does NOT read
    /// `chain_state`.
    pub fn validate_transfer_tx(&self, tx: &Transaction) -> Result<(), ValidationError> {
        debug!("Validating transfer transaction (intrinsic / tip-agnostic)");

        // Structural well-formedness only. These are properties of the value
        // itself, so all honest nodes agree on them regardless of tip.
        if tx.inputs.is_empty() {
            return Err(ValidationError::NoInputs);
        }
        if tx.outputs.is_empty() {
            return Err(ValidationError::NoOutputs);
        }
        if tx.outputs.iter().any(|o| o.amount == 0) {
            return Err(ValidationError::ZeroAmountOutput);
        }

        // Universal ML-KEM (6.0.0, #958/#973): every value-transfer output must
        // carry a valid hybrid ML-KEM ciphertext. This is a pure structural
        // property of the value (present + correct length), so it holds the SCP
        // no-fork invariant. Gated by the compile-time [`KEM_CIPHERTEXT_ENFORCED`]
        // cutover constant; a no-op on pre-reset / no-PQ binaries.
        for output in &tx.outputs {
            check_kem_ciphertext(&output.kem_ciphertext)?;
        }

        // UTXO existence, double-spend, and signature verification are NOT done
        // here. They happen in mempool.add_tx() and at block-apply, which have
        // ledger access and verify:
        // - UTXO existence in ledger
        // - Signature validity against UTXO target_key
        // - Input sum >= output sum + fee
        // - No double-spend (key-image uniqueness)

        debug!("Transfer transaction passed intrinsic validation");
        Ok(())
    }

    /// Validate a transaction from its serialized form against the local tip.
    ///
    /// NOTE: for minting txs this includes tip-relative checks and so MUST NOT
    /// be used as the SCP consensus validity function. Use
    /// [`validate_from_bytes_intrinsic`](Self::validate_from_bytes_intrinsic)
    /// for the SCP path.
    pub fn validate_from_bytes(
        &self,
        tx_bytes: &[u8],
        is_minting_tx: bool,
    ) -> Result<(), ValidationError> {
        if is_minting_tx {
            let tx: MintingTx = bincode::deserialize(tx_bytes)
                .map_err(|e| ValidationError::DeserializationFailed(e.to_string()))?;
            self.validate_minting_tx(&tx)
        } else {
            let tx: Transaction = bincode::deserialize(tx_bytes)
                .map_err(|e| ValidationError::DeserializationFailed(e.to_string()))?;
            self.validate_transfer_tx(&tx)
        }
    }

    /// Validate a transaction from its serialized form using only INTRINSIC
    /// (tip-agnostic) checks. This is the validity function SCP consensus uses.
    ///
    /// For minting txs this delegates to
    /// [`validate_minting_tx_intrinsic`](Self::validate_minting_tx_intrinsic),
    /// dropping the tip-equality checks so that a peer's competing-but-valid
    /// minting value is never silently dropped by SCP (issue #419 / #417
    /// Finding 1). For transfer txs this delegates to
    /// [`validate_transfer_tx`](Self::validate_transfer_tx), which is now also
    /// fully tip-agnostic — its former `state.height` staleness check was
    /// removed in issue #451 so a transfer value valid for one honest node is
    /// valid for all (full UTXO/double-spend/signature validation happens at
    /// mempool/apply time; stale txs are evicted by the mempool's wall-clock
    /// age limit so they are never proposed as values).
    pub fn validate_from_bytes_intrinsic(
        &self,
        tx_bytes: &[u8],
        is_minting_tx: bool,
    ) -> Result<(), ValidationError> {
        if is_minting_tx {
            let tx: MintingTx = bincode::deserialize(tx_bytes)
                .map_err(|e| ValidationError::DeserializationFailed(e.to_string()))?;
            Self::validate_minting_tx_intrinsic(&tx)
        } else {
            let tx: Transaction = bincode::deserialize(tx_bytes)
                .map_err(|e| ValidationError::DeserializationFailed(e.to_string()))?;
            self.validate_transfer_tx(&tx)
        }
    }

    /// Update the chain state reference
    pub fn update_chain_state(&mut self, chain_state: Arc<RwLock<ChainState>>) {
        self.chain_state = chain_state;
    }
}

/// Validation result for a batch of transactions
#[derive(Debug)]
pub struct BatchValidationResult {
    /// Valid transaction hashes
    pub valid: Vec<[u8; 32]>,
    /// Invalid transaction hashes with errors
    pub invalid: Vec<([u8; 32], ValidationError)>,
}

impl TransactionValidator {
    /// Validate multiple transactions, separating valid from invalid
    pub fn validate_batch(
        &self,
        txs: &[([u8; 32], Vec<u8>, bool)], // (hash, bytes, is_minting_tx)
    ) -> BatchValidationResult {
        let mut valid = Vec::new();
        let mut invalid = Vec::new();

        for (hash, bytes, is_minting_tx) in txs {
            match self.validate_from_bytes(bytes, *is_minting_tx) {
                Ok(()) => valid.push(*hash),
                Err(e) => invalid.push((*hash, e)),
            }
        }

        BatchValidationResult { valid, invalid }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{ClsagRingInput, RingMember, TxOutput, MIN_RING_SIZE, MIN_TX_FEE};
    use bth_transaction_types::ClusterTagVector;

    fn mock_chain_state() -> Arc<RwLock<ChainState>> {
        mock_chain_state_at(10)
    }

    fn mock_chain_state_at(height: u64) -> Arc<RwLock<ChainState>> {
        Arc::new(RwLock::new(ChainState {
            height,
            tip_hash: [0u8; 32],
            tip_timestamp: 1000000,
            difficulty: 1000,
            total_mined: 1_000_000_000_000,
            total_fees_burned: 0,
            // EmissionController fields
            total_tx: 0,
            epoch_tx: 0,
            epoch_emission: 0,
            epoch_burns: 0,
            current_reward: crate::block::difficulty::INITIAL_REWARD,
        }))
    }

    fn current_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Helper to create a well-formed 6.0.0 test output: carries a
    /// structurally-valid hybrid ML-KEM ciphertext so it passes universal
    /// enforcement ([`KEM_CIPHERTEXT_ENFORCED`]). Under a
    /// `--no-default-features` (no-PQ) build the ciphertext is simply
    /// ignored.
    fn test_output(amount: u64, id: u8) -> TxOutput {
        TxOutput {
            amount,
            target_key: [id; 32],
            public_key: [id.wrapping_add(1); 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
            kem_ciphertext: Some(vec![id; ML_KEM_768_CIPHERTEXT_BYTES]),
        }
    }

    /// A classical (ciphertext-less) test output — REJECTED under 6.0.0
    /// enforcement. Used by the reject-path tests.
    fn classical_output(amount: u64, id: u8) -> TxOutput {
        TxOutput {
            amount,
            target_key: [id; 32],
            public_key: [id.wrapping_add(1); 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
            kem_ciphertext: None,
        }
    }

    /// Helper to create a test ring member
    fn test_ring_member(id: u8) -> RingMember {
        RingMember {
            target_key: [id; 32],
            public_key: [id.wrapping_add(1); 32],
            commitment: [id.wrapping_add(2); 32],
        }
    }

    /// Helper to create a test CLSAG input with MIN_RING_SIZE members
    fn test_clsag_input(ring_id: u8) -> ClsagRingInput {
        let ring: Vec<RingMember> = (0..MIN_RING_SIZE)
            .map(|i| test_ring_member(ring_id.wrapping_add(i as u8)))
            .collect();
        ClsagRingInput {
            ring,
            key_image: [ring_id; 32],
            commitment_key_image: [ring_id.wrapping_add(100); 32],
            clsag_signature: vec![0u8; 32 + 32 * MIN_RING_SIZE],
            pseudo_output_amount: 0,
        }
    }

    #[test]
    fn test_minting_tx_wrong_height() {
        let validator = TransactionValidator::new(mock_chain_state());

        let tx = MintingTx {
            block_height: 5, // Wrong - should be 11
            reward: 600_000_000_000,
            minter_view_key: [0u8; 32],
            minter_spend_key: [0u8; 32],
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            // Valid ciphertext so enforcement passes and the height check is
            // reached (this test targets WrongBlockHeight, not the KEM gate).
            kem_ciphertext: Some(vec![0u8; ML_KEM_768_CIPHERTEXT_BYTES]),
            prev_block_hash: [0u8; 32],
            difficulty: 1000,
            nonce: 0,
            timestamp: 0,
        };

        let result = validator.validate_minting_tx(&tx);
        assert!(matches!(result, Err(ValidationError::WrongBlockHeight)));
    }

    #[test]
    fn test_transfer_tx_no_inputs() {
        let validator = TransactionValidator::new(mock_chain_state());

        // CLSAG transaction with empty inputs
        let tx = Transaction::new_clsag(vec![], vec![test_output(1000, 1)], MIN_TX_FEE, 10);
        let result = validator.validate_transfer_tx(&tx);
        assert!(matches!(result, Err(ValidationError::NoInputs)));
    }

    /// Issue #451 regression: the SCP transfer-tx validity gate
    /// (`validate_transfer_tx`) MUST be tip-agnostic.
    ///
    /// Two honest nodes at DIFFERENT heights straddling the old
    /// `created_at_height + MAX_TX_AGE` (100-block) boundary must AGREE on
    /// transfer-tx validity. With the old tip-dependent staleness check a tx
    /// created at height 10 validated at local height 10 but was dropped as
    /// `StaleTransaction` at local height 111 (10 + 100 + 1) — the #417-class
    /// asymmetric-validity fork condition. After the fix, validity is a pure
    /// function of the value and the result is identical regardless of tip.
    #[test]
    fn test_transfer_tx_validity_is_tip_agnostic() {
        // A well-formed transfer tx created at height 10.
        let tx = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![test_output(1000, 1)],
            MIN_TX_FEE,
            10, // created_at_height
        );

        // Node A: local tip at the creation height (well within the old window).
        let validator_low = TransactionValidator::new(mock_chain_state_at(10));
        let result_low = validator_low.validate_transfer_tx(&tx);

        // Node B: local tip far past the old 100-block staleness boundary
        // (10 + 100 + 1 = 111). Under the OLD check this returned
        // StaleTransaction; both nodes must now agree.
        let validator_high = TransactionValidator::new(mock_chain_state_at(111));
        let result_high = validator_high.validate_transfer_tx(&tx);

        assert!(
            result_low.is_ok(),
            "tx must be valid at the creation-height tip: {result_low:?}"
        );
        assert!(
            result_high.is_ok(),
            "tx must remain valid past the old staleness boundary (no asymmetric \
             drop / no #417-class fork): {result_high:?}"
        );
        assert_eq!(
            result_low.is_ok(),
            result_high.is_ok(),
            "transfer-tx validity must be identical regardless of local tip height"
        );
    }

    /// The intrinsic SCP path must yield the same result regardless of tip,
    /// even when invoked through `validate_from_bytes_intrinsic` (the exact
    /// entry point SCP's validity_fn uses).
    #[test]
    fn test_transfer_tx_intrinsic_path_tip_agnostic() {
        let tx = Transaction::new_clsag(
            vec![test_clsag_input(2)],
            vec![test_output(500, 2)],
            MIN_TX_FEE,
            5, // created_at_height
        );
        let bytes = bincode::serialize(&tx).expect("serialize tx");

        let low = TransactionValidator::new(mock_chain_state_at(5));
        let high = TransactionValidator::new(mock_chain_state_at(5 + 100 + 50));

        let r_low = low.validate_from_bytes_intrinsic(&bytes, false);
        let r_high = high.validate_from_bytes_intrinsic(&bytes, false);

        assert!(r_low.is_ok(), "intrinsic validity at low tip: {r_low:?}");
        assert!(r_high.is_ok(), "intrinsic validity at high tip: {r_high:?}");
    }

    #[test]
    fn test_minting_tx_correct_height() {
        let validator = TransactionValidator::new(mock_chain_state());

        let tx = MintingTx {
            block_height: 11,        // Correct - chain height is 10
            reward: 600_000_000_000, // Tail emission
            minter_view_key: [0u8; 32],
            minter_spend_key: [0u8; 32],
            target_key: [0u8; 32],
            public_key: [0u8; 32],
            kem_ciphertext: Some(vec![0u8; ML_KEM_768_CIPHERTEXT_BYTES]),
            prev_block_hash: [0u8; 32],
            difficulty: 1000,
            nonce: 0,
            timestamp: current_timestamp(),
        };

        // Should pass height check (may fail on other validation)
        let result = validator.validate_minting_tx(&tx);
        // Either passes or fails for different reason (not wrong height)
        match result {
            Err(ValidationError::WrongBlockHeight) => panic!("Should not fail on height"),
            _ => {} // Ok or other error is fine
        }
    }

    #[test]
    fn test_timestamp_check_safety() {
        // Test that current_timestamp() helper doesn't panic even with system time
        // issues
        let ts = current_timestamp();
        // Should return a reasonable value (not panic)
        assert!(ts > 0 || ts == 0); // Valid even if system clock is weird
    }

    #[test]
    fn test_transfer_tx_no_outputs() {
        let validator = TransactionValidator::new(mock_chain_state());

        // Transaction with inputs but no outputs
        let tx = Transaction::new_clsag(
            vec![test_clsag_input(1)],
            vec![], // No outputs
            MIN_TX_FEE,
            10,
        );

        let result = validator.validate_transfer_tx(&tx);
        assert!(matches!(result, Err(ValidationError::NoOutputs)));
    }

    #[test]
    fn test_validation_error_display() {
        let err = ValidationError::NoInputs;
        assert_eq!(format!("{}", err), "Transaction has no inputs");

        let err = ValidationError::NoOutputs;
        assert_eq!(format!("{}", err), "Transaction has no outputs");

        let err = ValidationError::WrongBlockHeight;
        assert_eq!(format!("{}", err), "Wrong block height");

        let err = ValidationError::WrongReward {
            expected: 100,
            got: 200,
        };
        assert!(format!("{}", err).contains("Wrong reward"));

        let err = ValidationError::TimestampTooFarInFuture;
        assert!(format!("{}", err).contains("future"));

        let err = ValidationError::InsufficientFunds {
            input: 100,
            output: 80,
            fee: 30,
        };
        assert!(format!("{}", err).contains("Insufficient funds"));

        let err = ValidationError::InvalidSignature;
        assert_eq!(format!("{}", err), "Invalid signature");
    }

    #[test]
    fn test_kem_validation_error_display() {
        let err = ValidationError::MissingKemCiphertext;
        assert!(format!("{}", err).contains("missing"));

        let err = ValidationError::InvalidKemCiphertext { len: 42 };
        let s = format!("{}", err);
        assert!(s.contains("42"));
        assert!(s.contains(&ML_KEM_768_CIPHERTEXT_BYTES.to_string()));
    }

    /// The compile-time cutover gate: universal ML-KEM enforcement is active
    /// exactly when the binary carries post-quantum crypto AND the protocol
    /// epoch is >= 6.0.0. Under the default (pq) build on 6.0.0 it must be ON.
    #[test]
    fn test_kem_enforcement_gate_active_on_reset() {
        assert_eq!(protocol_major("6.0.0"), 6);
        assert_eq!(protocol_major("5.9.9"), 5);
        assert_eq!(protocol_major("10.1.0"), 10);
        // The shipped constant tracks PROTOCOL_VERSION.
        assert_eq!(
            KEM_CIPHERTEXT_ENFORCED,
            cfg!(feature = "pq") && protocol_major(PROTOCOL_VERSION) >= 6
        );
    }

    // The following reject/accept tests only make sense when the cutover gate is
    // active, i.e. a post-quantum (`pq`) build. A `--no-default-features` build
    // produces classical outputs and must NOT reject them, which the gate
    // guarantees by staying OFF.
    #[cfg(feature = "pq")]
    mod kem_enforcement {
        use super::*;

        /// The local ciphertext-length constant must equal the crypto crate's
        /// canonical value (it is redeclared for `--no-default-features`).
        #[test]
        fn local_ciphertext_len_matches_crypto_crate() {
            assert_eq!(
                ML_KEM_768_CIPHERTEXT_BYTES,
                bth_crypto_pq::ML_KEM_768_CIPHERTEXT_BYTES
            );
        }

        /// A transfer tx whose output lacks a ciphertext is REJECTED.
        #[test]
        fn transfer_tx_missing_ciphertext_rejected() {
            let validator = TransactionValidator::new(mock_chain_state());
            let tx = Transaction::new_clsag(
                vec![test_clsag_input(1)],
                vec![classical_output(1000, 1)],
                MIN_TX_FEE,
                10,
            );
            assert!(matches!(
                validator.validate_transfer_tx(&tx),
                Err(ValidationError::MissingKemCiphertext)
            ));
        }

        /// A transfer tx whose output ciphertext is the wrong length is
        /// REJECTED.
        #[test]
        fn transfer_tx_wrong_length_ciphertext_rejected() {
            let validator = TransactionValidator::new(mock_chain_state());
            let mut out = test_output(1000, 1);
            out.kem_ciphertext = Some(vec![0u8; ML_KEM_768_CIPHERTEXT_BYTES - 1]);
            let tx = Transaction::new_clsag(vec![test_clsag_input(1)], vec![out], MIN_TX_FEE, 10);
            assert!(matches!(
                validator.validate_transfer_tx(&tx),
                Err(ValidationError::InvalidKemCiphertext {
                    len
                }) if len == ML_KEM_768_CIPHERTEXT_BYTES - 1
            ));
        }

        /// A transfer tx with a valid hybrid output is ACCEPTED.
        #[test]
        fn transfer_tx_valid_hybrid_accepted() {
            let validator = TransactionValidator::new(mock_chain_state());
            let tx = Transaction::new_clsag(
                vec![test_clsag_input(1)],
                vec![test_output(1000, 1)],
                MIN_TX_FEE,
                10,
            );
            assert!(validator.validate_transfer_tx(&tx).is_ok());
        }

        /// If ANY output lacks a ciphertext (even alongside valid ones) the tx
        /// is REJECTED.
        #[test]
        fn transfer_tx_mixed_outputs_rejected() {
            let validator = TransactionValidator::new(mock_chain_state());
            let tx = Transaction::new_clsag(
                vec![test_clsag_input(1)],
                vec![test_output(1000, 1), classical_output(500, 2)],
                MIN_TX_FEE,
                10,
            );
            assert!(matches!(
                validator.validate_transfer_tx(&tx),
                Err(ValidationError::MissingKemCiphertext)
            ));
        }

        /// A minting tx whose coinbase output lacks a ciphertext is REJECTED on
        /// BOTH the tip-relative and intrinsic (SCP) paths.
        #[test]
        fn minting_tx_missing_ciphertext_rejected() {
            let validator = TransactionValidator::new(mock_chain_state());
            let tx = MintingTx {
                block_height: 11,
                reward: 600_000_000_000,
                minter_view_key: [0u8; 32],
                minter_spend_key: [0u8; 32],
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                kem_ciphertext: None,
                prev_block_hash: [0u8; 32],
                difficulty: 1000,
                nonce: 0,
                timestamp: current_timestamp(),
            };
            assert!(matches!(
                validator.validate_minting_tx(&tx),
                Err(ValidationError::MissingKemCiphertext)
            ));
            assert!(matches!(
                TransactionValidator::validate_minting_tx_intrinsic(&tx),
                Err(ValidationError::MissingKemCiphertext)
            ));
        }

        /// A minting tx with a wrong-length coinbase ciphertext is REJECTED.
        #[test]
        fn minting_tx_wrong_length_ciphertext_rejected() {
            let tx = MintingTx {
                block_height: 11,
                reward: 600_000_000_000,
                minter_view_key: [0u8; 32],
                minter_spend_key: [0u8; 32],
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                kem_ciphertext: Some(vec![0u8; 7]),
                prev_block_hash: [0u8; 32],
                difficulty: 1000,
                nonce: 0,
                timestamp: current_timestamp(),
            };
            assert!(matches!(
                TransactionValidator::validate_minting_tx_intrinsic(&tx),
                Err(ValidationError::InvalidKemCiphertext { len: 7 })
            ));
        }

        /// End-to-end: a real 6.0.0 coinbase built via the production
        /// `MintingTx::new` path (encapsulating to a v2 address that publishes
        /// an ML-KEM key) carries a valid 1088-byte ciphertext, so the
        /// universal ML-KEM gate ACCEPTS it — any failure is on an
        /// unrelated check (e.g. unsolved PoW), never a KEM error.
        /// Avoids solving RandomX in-test.
        #[test]
        fn real_hybrid_coinbase_passes_kem_gate() {
            use bth_account_keys::AccountKey;
            use bth_crypto_pq::MlKem768KeyPair;
            use rand_chacha::ChaCha20Rng;
            use rand_core::SeedableRng;

            let mut rng = ChaCha20Rng::from_seed([9u8; 32]);
            let account = AccountKey::random(&mut rng);
            let kem = MlKem768KeyPair::from_seed(&[0x5A; 32]);
            let addr = account
                .default_subaddress()
                .with_pq_keys(kem.public_key().as_bytes().to_vec(), Vec::new());

            let tx = MintingTx::new(
                11,
                600_000_000_000,
                &addr,
                [0u8; 32],
                1000,
                current_timestamp(),
            );
            assert_eq!(
                tx.kem_ciphertext.as_ref().map(|c| c.len()),
                Some(ML_KEM_768_CIPHERTEXT_BYTES),
                "real 6.0.0 coinbase must carry a 1088-byte ciphertext"
            );
            // The KEM gate accepts it: the only way intrinsic validation fails is
            // an unrelated check (unsolved PoW), never a KEM error.
            match TransactionValidator::validate_minting_tx_intrinsic(&tx) {
                Ok(())
                | Err(ValidationError::InvalidPoW)
                | Err(ValidationError::TimestampTooFarInFuture) => {}
                Err(ValidationError::MissingKemCiphertext)
                | Err(ValidationError::InvalidKemCiphertext { .. }) => {
                    panic!("real hybrid coinbase must not fail the KEM gate")
                }
                other => panic!("unexpected validation result: {other:?}"),
            }
        }

        /// #978 acceptance heart: a transfer built by the BROWSER signer
        /// (`bth_wasm_signer`, the exact Rust the web wallet compiles to wasm)
        /// is ACCEPTED by the node's `validate_transfer_tx` universal-ML-KEM
        /// gate. This closes the loop the issue opened: before this change the
        /// browser emitted `kem_ciphertext: None` on every output and every
        /// browser send was rejected. Now the browser encapsulates against the
        /// recipient's (and its own, for change) published ML-KEM key, so both
        /// outputs carry a 1088-byte ciphertext and consensus accepts the tx.
        #[test]
        fn browser_built_transfer_passes_node_kem_gate() {
            use bth_account_keys::AccountKey;
            use bth_transaction_clsag::DEFAULT_RING_SIZE;
            use bth_wasm_signer::core::{
                build_and_sign_with_rng, DecoyOutput, RecipientAddress, SignRequest, SpendInput,
            };
            use rand_chacha::ChaCha20Rng;
            use rand_core::{RngCore, SeedableRng};

            let mut rng = ChaCha20Rng::from_seed([73u8; 32]);

            // Sender + recipient classical accounts, each with a published
            // ML-KEM key (their v2 addresses).
            let sender = AccountKey::random(&mut rng);
            let sender_kem = bth_crypto_pq::derive_pq_keys_from_seed(&[0x11; 64]);
            let recipient = AccountKey::random(&mut rng);
            let recipient_kem = bth_crypto_pq::derive_pq_keys_from_seed(&[0x22; 64]);

            // The sender's owned output + a full decoy ring.
            let owned_amount = 10_000_000_000u64;
            let owned = TxOutput::new(owned_amount, &sender.default_subaddress());
            let decoys: Vec<DecoyOutput> = (0..DEFAULT_RING_SIZE - 1)
                .map(|_| {
                    let acct = AccountKey::random(&mut rng);
                    let out = TxOutput::new(owned_amount, &acct.default_subaddress());
                    DecoyOutput {
                        target_key: hex::encode(out.target_key),
                        public_key: hex::encode(out.public_key),
                        amount: owned_amount,
                    }
                })
                .collect();

            let recipient_addr = recipient.default_subaddress();
            let req = SignRequest {
                spend_private_key: hex::encode(sender.spend_private_key().to_bytes()),
                view_private_key: hex::encode(sender.view_private_key().to_bytes()),
                inputs: vec![SpendInput {
                    target_key: hex::encode(owned.target_key),
                    public_key: hex::encode(owned.public_key),
                    amount: owned_amount,
                    subaddress_index: 0,
                    decoys,
                }],
                recipient: RecipientAddress {
                    spend_public_key: hex::encode(recipient_addr.spend_public_key().to_bytes()),
                    view_public_key: hex::encode(recipient_addr.view_public_key().to_bytes()),
                    kem_public_key: hex::encode(recipient_kem.kem_keypair.public_key().as_bytes()),
                },
                sender_kem_public_key: hex::encode(sender_kem.kem_keypair.public_key().as_bytes()),
                amount: 4_000_000_000,
                fee: MIN_TX_FEE,
                created_at_height: 1000,
            };

            // Build + CLSAG-sign exactly as the browser does.
            let mut sign_rng = ChaCha20Rng::from_seed({
                let mut s = [0u8; 32];
                rng.fill_bytes(&mut s);
                s
            });
            let tx = build_and_sign_with_rng(&req, &mut sign_rng)
                .expect("browser signer must build+sign the tx");

            // Every output carries a 1088-byte ML-KEM ciphertext.
            assert_eq!(tx.outputs.len(), 2, "recipient + change");
            for out in &tx.outputs {
                assert_eq!(
                    out.kem_ciphertext.as_ref().map(|c| c.len()),
                    Some(ML_KEM_768_CIPHERTEXT_BYTES),
                    "browser output must carry a 1088-byte ciphertext"
                );
            }

            // The node's SCP consensus validity gate ACCEPTS the browser-built
            // transfer. Enforcement is active in this build (pq + 6.0.0).
            assert!(KEM_CIPHERTEXT_ENFORCED, "test presumes 6.0.0 + pq build");
            let validator = TransactionValidator::new(mock_chain_state());
            validator
                .validate_transfer_tx(&tx)
                .expect("node must accept the browser-built transfer under 6.0.0");
        }
    }

    #[test]
    fn test_batch_validation_empty_bytes() {
        let validator = TransactionValidator::new(mock_chain_state());

        // Test with invalid (empty) bytes - should fail deserialization
        let invalid_bytes = vec![];

        let batch = vec![
            ([1u8; 32], invalid_bytes.clone(), false),
            ([2u8; 32], invalid_bytes, true), // Also test minting tx path
        ];

        let result = validator.validate_batch(&batch);

        // Both should fail deserialization
        assert_eq!(result.invalid.len(), 2);
        assert!(result.valid.is_empty());
    }
}
