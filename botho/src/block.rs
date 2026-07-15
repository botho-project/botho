use bth_account_keys::PublicAddress;
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_ring_signature::onetime_keys::{create_tx_out_public_key, create_tx_out_target_key};
use bth_transaction_types::{ClusterId, ClusterTagVector, Network};
use bth_util_from_random::FromRandom;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::transaction::{Transaction, TxOutput};

/// The coinbase reward occupies output index 0 within its (single-output)
/// minting transaction.
///
/// This index is bound into the hybrid ML-KEM one-time-key derivation
/// (whitepaper §4.2, issue #958), so the minter must scan its own reward at
/// this same index. Lottery payouts reuse the winning UTXO's keys and index
/// verbatim, so they are unaffected by this constant.
pub const MINTING_OUTPUT_INDEX: u32 = 0;

/// Genesis block magic bytes for mainnet (stored in prev_block_hash).
/// ASCII: "BOTHO_MAINNET_GENESIS_V1" padded to 32 bytes
pub const MAINNET_GENESIS_MAGIC: [u8; 32] = [
    0x42, 0x4F, 0x54, 0x48, 0x4F, 0x5F, 0x4D, 0x41, // BOTHO_MA
    0x49, 0x4E, 0x4E, 0x45, 0x54, 0x5F, 0x47, 0x45, // INNET_GE
    0x4E, 0x45, 0x53, 0x49, 0x53, 0x5F, 0x56, 0x31, // NESIS_V1
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // padding
];

/// Genesis block magic bytes for testnet (stored in prev_block_hash).
/// ASCII: "BOTHO_TESTNET_GENESIS_V1" padded to 32 bytes
pub const TESTNET_GENESIS_MAGIC: [u8; 32] = [
    0x42, 0x4F, 0x54, 0x48, 0x4F, 0x5F, 0x54, 0x45, // BOTHO_TE
    0x53, 0x54, 0x4E, 0x45, 0x54, 0x5F, 0x47, 0x45, // STNET_GE
    0x4E, 0x45, 0x53, 0x49, 0x53, 0x5F, 0x56, 0x31, // NESIS_V1
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // padding
];

/// Block header containing PoW fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Block version
    pub version: u32,

    /// Hash of the previous block (32 bytes)
    pub prev_block_hash: [u8; 32],

    /// Merkle root of transactions (32 bytes)
    pub tx_root: [u8; 32],

    /// Block timestamp (unix seconds)
    pub timestamp: u64,

    /// Block height
    pub height: u64,

    /// Minting difficulty target
    pub difficulty: u64,

    /// PoW nonce (the minting solution)
    pub nonce: u64,

    /// Minter's view public key
    pub minter_view_key: [u8; 32],

    /// Minter's spend public key
    pub minter_spend_key: [u8; 32],
}

impl BlockHeader {
    /// Compute the hash of this block header
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.version.to_le_bytes());
        hasher.update(self.prev_block_hash);
        hasher.update(self.tx_root);
        hasher.update(self.timestamp.to_le_bytes());
        hasher.update(self.height.to_le_bytes());
        hasher.update(self.difficulty.to_le_bytes());
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.minter_view_key);
        hasher.update(self.minter_spend_key);
        hasher.finalize().into()
    }

    /// Compute the PoW hash (what minters are trying to get below target).
    ///
    /// This is **RandomX** (ASIC/GPU-resistant CPU PoW) over the preimage
    /// `nonce ‖ prev_block_hash ‖ minter_view_key ‖ minter_spend_key`, keyed by
    /// the chain-deterministic seed for this block's height (see
    /// [`crate::pow`]). Verification uses RandomX **light mode**; it produces
    /// the identical hash a miner's fast mode produces.
    ///
    /// The seed key is a pure function of `self.height`, so this remains a pure
    /// function of the header — no chain lookup is required and every node
    /// agrees on the result.
    pub fn pow_hash(&self) -> [u8; 32] {
        let seed = crate::pow::seed_key_for_height(self.height);
        let preimage = crate::pow::pow_preimage(
            self.nonce,
            &self.prev_block_hash,
            &self.minter_view_key,
            &self.minter_spend_key,
        );
        crate::pow::verify_pow_hash(&seed, &preimage)
    }

    /// Check if PoW is valid (hash < difficulty target).
    ///
    /// Big-endian read of the first 8 bytes of the RandomX output, compared to
    /// the difficulty target (lower = better PoW) — same convention as the
    /// legacy SHA-256 PoW.
    pub fn is_valid_pow(&self) -> bool {
        let hash = self.pow_hash();
        crate::pow::pow_value(&hash) < self.difficulty
    }

    /// Create header for genesis block (defaults to testnet for backward
    /// compatibility)
    pub fn genesis() -> Self {
        Self::genesis_for_network(Network::Testnet)
    }

    /// Create header for genesis block for a specific network.
    ///
    /// Each network has a unique genesis block with different magic bytes
    /// in the prev_block_hash field, ensuring chain separation.
    pub fn genesis_for_network(network: Network) -> Self {
        let magic = match network {
            Network::Mainnet => MAINNET_GENESIS_MAGIC,
            Network::Testnet => TESTNET_GENESIS_MAGIC,
        };

        Self {
            version: 1,
            prev_block_hash: magic, // Network-specific magic bytes
            tx_root: [0u8; 32],
            timestamp: 0,
            height: 0,
            difficulty: u64::MAX, // Genesis has no PoW requirement
            nonce: 0,
            minter_view_key: [0u8; 32],
            minter_spend_key: [0u8; 32],
        }
    }

    /// Check if this is a genesis block header by examining the magic bytes.
    pub fn is_genesis(&self) -> bool {
        self.height == 0
            && (self.prev_block_hash == MAINNET_GENESIS_MAGIC
                || self.prev_block_hash == TESTNET_GENESIS_MAGIC
                || self.prev_block_hash == [0u8; 32]) // Legacy genesis
    }

    /// Get the network this genesis header belongs to, if it's a genesis block.
    pub fn genesis_network(&self) -> Option<Network> {
        if self.height != 0 {
            return None;
        }
        if self.prev_block_hash == MAINNET_GENESIS_MAGIC {
            Some(Network::Mainnet)
        } else if self.prev_block_hash == TESTNET_GENESIS_MAGIC {
            Some(Network::Testnet)
        } else if self.prev_block_hash == [0u8; 32] {
            // Legacy genesis defaults to testnet
            Some(Network::Testnet)
        } else {
            None
        }
    }
}

/// A minting transaction (coinbase) that creates new coins via PoW.
///
/// Uses CryptoNote-style stealth addresses for minter privacy:
/// - `target_key`: One-time public key that only the minter can identify and
///   spend
/// - `public_key`: Ephemeral DH public key for minter to derive shared secret
///
/// Even if the same minter wins multiple blocks, their rewards are unlinkable.
///
/// Also includes the minter's public address (view_key, spend_key) for:
/// - PoW binding: The proof of work is tied to the minter's identity
/// - Block header: Required for block construction and verification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MintingTx {
    /// Block height this reward is for
    pub block_height: u64,

    /// Reward amount in picocredits
    pub reward: u64,

    /// Minter's view public key (for PoW binding and block header)
    pub minter_view_key: [u8; 32],

    /// Minter's spend public key (for PoW binding and block header)
    pub minter_spend_key: [u8; 32],

    /// One-time target key: `Hs(r * C) * G + D`
    /// This is the stealth spend public key that only the minter can identify.
    pub target_key: [u8; 32],

    /// Ephemeral public key: `r * D`
    /// Used by minter to derive the shared secret for detecting ownership.
    pub public_key: [u8; 32],

    /// ML-KEM-768 encapsulation ciphertext (1,088 bytes) for the hybrid
    /// post-quantum stealth envelope (issue #958, whitepaper §4.2).
    ///
    /// Coinbase has no external sender, so the minter encapsulates a shared
    /// secret against its OWN published ML-KEM-768 key and folds it — together
    /// with the classical DH secret — into `target_key`. The minter recovers
    /// its reward by decapsulating this ciphertext with its ML-KEM secret key.
    /// `None` on classical (pre-6.0.0-format) coinbases, which hash and
    /// serialize exactly as before.
    #[serde(default)]
    pub kem_ciphertext: Option<Vec<u8>>,

    // PoW proof fields
    /// Previous block hash this minting tx builds on
    pub prev_block_hash: [u8; 32],

    /// Difficulty target at time of minting
    pub difficulty: u64,

    /// PoW nonce (the solution)
    pub nonce: u64,

    /// Timestamp when minted
    pub timestamp: u64,
}

impl MintingTx {
    /// Create a new minting transaction with stealth output for the given
    /// minter address.
    pub fn new(
        block_height: u64,
        reward: u64,
        minter_address: &PublicAddress,
        prev_block_hash: [u8; 32],
        difficulty: u64,
        timestamp: u64,
    ) -> Self {
        // Store minter's public address for PoW binding
        let minter_view_key = minter_address.view_public_key().to_bytes();
        let minter_spend_key = minter_address.spend_public_key().to_bytes();

        // Create the hybrid stealth keys for the reward output, encapsulating
        // to the minter's own published ML-KEM key (see
        // [`MintingTx::coinbase_stealth_fields`]).
        let (target_key, public_key, kem_ciphertext) =
            Self::coinbase_stealth_fields(minter_address);

        Self {
            block_height,
            reward,
            minter_view_key,
            minter_spend_key,
            target_key,
            public_key,
            kem_ciphertext,
            prev_block_hash,
            difficulty,
            nonce: 0,
            timestamp,
        }
    }

    /// Build the coinbase reward's stealth output fields
    /// `(target_key, public_key, kem_ciphertext)` for `minter_address`.
    ///
    /// A coinbase has no external sender, so the minter encapsulates a shared
    /// secret against its OWN published ML-KEM-768 key and folds it into the
    /// one-time `target_key` (the #957 hybrid construction, reused via
    /// [`TxOutput::new_hybrid_to_address`]). This lets the minter scan and
    /// spend its reward while keeping the classical DH stealth privacy.
    ///
    /// The reward always occupies [`MINTING_OUTPUT_INDEX`] within its
    /// single-output minting transaction, and that index is bound into the
    /// hybrid derivation.
    ///
    /// Falls back to a classical stealth output (`kem_ciphertext == None`) when
    /// `minter_address` publishes no well-formed ML-KEM key (a pre-v2 / test
    /// address, or a `--no-default-features` build). Consensus enforcement that
    /// every output carry a ciphertext lands in a later sub-issue (#958 step
    /// 7).
    pub(crate) fn coinbase_stealth_fields(
        minter_address: &PublicAddress,
    ) -> ([u8; 32], [u8; 32], Option<Vec<u8>>) {
        match TxOutput::new_hybrid_to_address(
            0,
            minter_address,
            MINTING_OUTPUT_INDEX,
            None,
            ClusterTagVector::empty(),
        ) {
            Ok(out) => (out.target_key, out.public_key, out.kem_ciphertext),
            Err(_) => {
                // Address publishes no ML-KEM key: emit a classical stealth
                // output so pre-v2 / test addresses keep working.
                let tx_private_key = RistrettoPrivate::from_random(&mut OsRng);
                let target_key = create_tx_out_target_key(&tx_private_key, minter_address);
                let public_key =
                    create_tx_out_public_key(&tx_private_key, minter_address.spend_public_key());
                (target_key.to_bytes(), public_key.to_bytes(), None)
            }
        }
    }

    /// Compute the PoW hash.
    ///
    /// RandomX over `nonce ‖ prev_block_hash ‖ minter_view_key ‖
    /// minter_spend_key`, keyed by the chain-deterministic seed for
    /// `self.block_height`. This MUST stay byte-for-byte consistent with
    /// [`BlockHeader::pow_hash`] — they use the same preimage and the same
    /// per-height seed (header `height` == minting tx `block_height`, enforced
    /// at block-apply time), so a header and its minting tx always agree on the
    /// PoW.
    pub fn pow_hash(&self) -> [u8; 32] {
        let seed = crate::pow::seed_key_for_height(self.block_height);
        let preimage = crate::pow::pow_preimage(
            self.nonce,
            &self.prev_block_hash,
            &self.minter_view_key,
            &self.minter_spend_key,
        );
        crate::pow::verify_pow_hash(&seed, &preimage)
    }

    /// Verify the PoW is valid
    pub fn verify_pow(&self) -> bool {
        let hash = self.pow_hash();
        crate::pow::pow_value(&hash) < self.difficulty
    }

    /// Get the PoW hash value as u64 (lower = better, used for priority in
    /// consensus)
    pub fn pow_priority(&self) -> u64 {
        let hash = self.pow_hash();
        // Invert so that lower hash = higher priority
        u64::MAX - crate::pow::pow_value(&hash)
    }

    /// Compute the hash of this minting transaction (for consensus)
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.block_height.to_le_bytes());
        hasher.update(self.reward.to_le_bytes());
        hasher.update(self.minter_view_key);
        hasher.update(self.minter_spend_key);
        hasher.update(self.target_key);
        hasher.update(self.public_key);
        // Bind the ML-KEM ciphertext into the coinbase's consensus identity so
        // the post-quantum envelope is committed (whitepaper §4.2, #958).
        // Classical coinbases (`None`) are skipped, so their hash is
        // byte-identical to the pre-6.0.0 layout — block-hash determinism and
        // the header↔minting-tx PoW link (which does not use this hash) are
        // preserved. Folded immediately after the stealth keys it accompanies,
        // before the PoW-proof fields.
        if let Some(ct) = &self.kem_ciphertext {
            hasher.update(ct);
        }
        hasher.update(self.prev_block_hash);
        hasher.update(self.difficulty.to_le_bytes());
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.timestamp.to_le_bytes());
        hasher.finalize().into()
    }

    /// The lottery emission share of this block's reward.
    ///
    /// A height-scheduled fraction of the block reward is routed into the
    /// redistribution lottery pool instead of the miner's coinbase
    /// (0 in the bootstrap epoch, ramping per halving — see
    /// `MonetaryPolicy::lottery_emission_bps`). Total emission is unchanged.
    pub fn lottery_emission_share(&self) -> u64 {
        crate::monetary::mainnet_policy().lottery_emission_share(self.block_height, self.reward)
    }

    /// The miner's coinbase payout: the block reward minus the lottery
    /// emission share.
    pub fn miner_payout(&self) -> u64 {
        self.reward.saturating_sub(self.lottery_emission_share())
    }

    /// Convert this minting transaction's output into a TxOutput for ledger
    /// storage.
    ///
    /// This allows the ledger to store minting rewards using the same UTXO
    /// format as regular transaction outputs. The amount is the miner's
    /// payout (reward minus the lottery emission share).
    ///
    /// Minting creates a **new cluster origin** - the output is tagged with
    /// 100% attribution to a new cluster derived from the minting tx hash.
    /// This is how coin lineage tracking begins.
    pub fn to_tx_output(&self) -> TxOutput {
        // Create a new cluster ID from the first 8 bytes of the minting tx hash
        let tx_hash = self.hash();
        let cluster_id = ClusterId(u64::from_le_bytes(tx_hash[0..8].try_into().unwrap()));

        TxOutput {
            amount: self.miner_payout(),
            target_key: self.target_key,
            public_key: self.public_key,
            e_memo: None, // Minting rewards don't have memos
            cluster_tags: ClusterTagVector::single(cluster_id),
            kem_ciphertext: self.kem_ciphertext.clone(),
        }
    }
}

/// Lottery payout output for a winning UTXO.
///
/// These are created when fees are redistributed via the lottery system.
/// Each winning UTXO receives a payout as a new stealth output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LotteryOutput {
    /// Transaction hash of the winning UTXO
    pub winner_tx_hash: [u8; 32],

    /// Output index within the transaction
    pub winner_output_index: u32,

    /// Payout amount in picocredits
    pub payout: u64,

    /// Stealth target key for the payout (same owner as winning UTXO)
    pub target_key: [u8; 32],

    /// Ephemeral public key for stealth derivation
    pub public_key: [u8; 32],

    /// ML-KEM-768 ciphertext (1,088 bytes) inherited from the winning UTXO's
    /// hybrid stealth envelope (issue #958, whitepaper §4.2).
    ///
    /// A lottery payout reuses the winning UTXO's one-time stealth keys
    /// (`target_key`/`public_key`) so it goes to the same owner; it therefore
    /// also reuses the winning UTXO's ML-KEM ciphertext, and the winner
    /// recovers the payout with the exact hybrid derivation (same shared
    /// secret, same output index) they used for the original UTXO. `None` when
    /// the winning UTXO is a classical (pre-6.0.0) output.
    #[serde(default)]
    pub kem_ciphertext: Option<Vec<u8>>,
}

impl LotteryOutput {
    /// Get the winner UTXO ID as a 36-byte array (tx_hash || output_index).
    pub fn winner_utxo_id(&self) -> [u8; 36] {
        let mut id = [0u8; 36];
        id[..32].copy_from_slice(&self.winner_tx_hash);
        id[32..].copy_from_slice(&self.winner_output_index.to_le_bytes());
        id
    }

    /// Create from a 36-byte UTXO ID.
    ///
    /// `kem_ciphertext` is the winning UTXO's ML-KEM ciphertext, reused
    /// verbatim so the payout carries the same hybrid stealth envelope as the
    /// output it redistributes to (issue #958). Pass `None` for a classical
    /// winning UTXO.
    pub fn from_utxo_id(
        utxo_id: [u8; 36],
        payout: u64,
        target_key: [u8; 32],
        public_key: [u8; 32],
        kem_ciphertext: Option<Vec<u8>>,
    ) -> Self {
        let mut tx_hash = [0u8; 32];
        tx_hash.copy_from_slice(&utxo_id[..32]);
        let output_index = u32::from_le_bytes(utxo_id[32..36].try_into().unwrap());
        Self {
            winner_tx_hash: tx_hash,
            winner_output_index: output_index,
            payout,
            target_key,
            public_key,
            kem_ciphertext,
        }
    }

    /// Compute a consensus identity hash over this lottery payout.
    ///
    /// Binds the ML-KEM ciphertext (when present) into the payout identity so
    /// the post-quantum envelope is committed alongside the stealth keys,
    /// mirroring [`MintingTx::hash`]. Classical payouts (`kem_ciphertext ==
    /// None`) hash over the classical fields only, so their identity is
    /// unchanged from the pre-6.0.0 layout.
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.winner_tx_hash);
        hasher.update(self.winner_output_index.to_le_bytes());
        hasher.update(self.payout.to_le_bytes());
        hasher.update(self.target_key);
        hasher.update(self.public_key);
        if let Some(ct) = &self.kem_ciphertext {
            hasher.update(ct);
        }
        hasher.finalize().into()
    }

    /// Convert to a TxOutput for ledger storage.
    ///
    /// Lottery payouts inherit the cluster tags from the winning UTXO.
    /// This is appropriate because the payout represents redistribution
    /// to the same identity that owns the winning UTXO.
    pub fn to_tx_output(&self, cluster_tags: ClusterTagVector) -> TxOutput {
        TxOutput {
            amount: self.payout,
            target_key: self.target_key,
            public_key: self.public_key,
            e_memo: None, // Lottery payouts don't need memos
            cluster_tags,
            kem_ciphertext: self.kem_ciphertext.clone(),
        }
    }
}

/// Summary of lottery drawing for a block.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct BlockLotterySummary {
    /// Total fees collected from transactions
    pub total_fees: u64,

    /// Amount distributed to lottery winners (80%)
    pub pool_distributed: u64,

    /// Amount burned (20%)
    pub amount_burned: u64,

    /// Seed used for verifiable randomness
    pub lottery_seed: [u8; 32],
}

/// A complete block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub minting_tx: MintingTx,
    /// Regular transactions included in this block
    pub transactions: Vec<Transaction>,
    /// Lottery payout outputs (fee redistribution to random UTXOs)
    #[serde(default)]
    pub lottery_outputs: Vec<LotteryOutput>,
    /// Lottery summary for validation
    #[serde(default)]
    pub lottery_summary: BlockLotterySummary,
}

impl Block {
    /// Create the genesis block (defaults to testnet for backward
    /// compatibility)
    pub fn genesis() -> Self {
        Self::genesis_for_network(Network::Testnet)
    }

    /// Create the genesis block for a specific network.
    ///
    /// Each network has a unique genesis block with different magic bytes,
    /// ensuring that mainnet and testnet chains are completely separate.
    pub fn genesis_for_network(network: Network) -> Self {
        let genesis_magic = match network {
            Network::Mainnet => MAINNET_GENESIS_MAGIC,
            Network::Testnet => TESTNET_GENESIS_MAGIC,
        };

        Self {
            header: BlockHeader::genesis_for_network(network),
            minting_tx: MintingTx {
                block_height: 0,
                reward: 0,
                // Genesis has no real minter - use zero keys
                minter_view_key: [0u8; 32],
                minter_spend_key: [0u8; 32],
                // Genesis has no stealth output - use zero keys
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                // Genesis coinbase is classical (no ML-KEM envelope)
                kem_ciphertext: None,
                prev_block_hash: genesis_magic, // Network-specific magic
                difficulty: u64::MAX,
                nonce: 0,
                timestamp: 0,
            },
            transactions: Vec::new(),
            lottery_outputs: Vec::new(),
            lottery_summary: BlockLotterySummary::default(),
        }
    }

    /// Check if this is a genesis block.
    pub fn is_genesis(&self) -> bool {
        self.header.is_genesis()
    }

    /// Get the network this genesis block belongs to, if it's a genesis block.
    pub fn genesis_network(&self) -> Option<Network> {
        self.header.genesis_network()
    }

    /// Get the block hash
    pub fn hash(&self) -> [u8; 32] {
        self.header.hash()
    }

    /// Get block height
    pub fn height(&self) -> u64 {
        self.header.height
    }

    /// Create a new block template for minting (without transactions)
    pub fn new_template(
        prev_block: &Block,
        minter_address: &PublicAddress,
        difficulty: u64,
        reward: u64,
    ) -> Self {
        Self::new_template_with_txs(prev_block, minter_address, difficulty, reward, Vec::new())
    }

    /// Create a new block template for minting with transactions.
    ///
    /// The minting reward output uses stealth addressing for minter privacy.
    /// Note: Lottery outputs should be added separately via
    /// `set_lottery_result`.
    pub fn new_template_with_txs(
        prev_block: &Block,
        minter_address: &PublicAddress,
        difficulty: u64,
        reward: u64,
        transactions: Vec<Transaction>,
    ) -> Self {
        let prev_hash = prev_block.hash();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let minter_view_key = minter_address.view_public_key().to_bytes();
        let minter_spend_key = minter_address.spend_public_key().to_bytes();

        // Compute transaction root from all transactions
        let tx_root = Self::compute_tx_root(&transactions);

        // Create stealth output for minting reward
        let minting_tx = MintingTx::new(
            prev_block.height() + 1,
            reward,
            minter_address,
            prev_hash,
            difficulty,
            timestamp,
        );

        Self {
            header: BlockHeader {
                version: 1,
                prev_block_hash: prev_hash,
                tx_root,
                timestamp,
                height: prev_block.height() + 1,
                difficulty,
                nonce: 0,
                minter_view_key,
                minter_spend_key,
            },
            minting_tx,
            transactions,
            lottery_outputs: Vec::new(),
            lottery_summary: BlockLotterySummary::default(),
        }
    }

    /// Set lottery results for this block.
    ///
    /// Should be called after block template creation to add lottery outputs
    /// based on the winning UTXOs.
    pub fn set_lottery_result(
        &mut self,
        lottery_outputs: Vec<LotteryOutput>,
        summary: BlockLotterySummary,
    ) {
        self.lottery_outputs = lottery_outputs;
        self.lottery_summary = summary;
    }

    /// Get total lottery payouts in this block.
    ///
    /// Saturating for the same reason as [`Block::total_fees`]: payouts in a
    /// gossiped block are attacker-influenced, and with `overflow-checks =
    /// true` on the release profile (#663) an unchecked `sum()` would panic
    /// on a crafted block instead of letting validation reject it.
    pub fn total_lottery_payouts(&self) -> u64 {
        self.lottery_outputs
            .iter()
            .fold(0u64, |acc, o| acc.saturating_add(o.payout))
    }

    /// Compute the transaction root committed by the block header.
    ///
    /// Currently a flat SHA-256 over the sequence of transaction hashes
    /// (not a Merkle tree). Sufficient to bind the transaction list to
    /// the header — and therefore to PoW — but does not support
    /// inclusion proofs.
    pub fn compute_tx_root(transactions: &[Transaction]) -> [u8; 32] {
        if transactions.is_empty() {
            return [0u8; 32];
        }

        let mut hasher = Sha256::new();
        for tx in transactions {
            hasher.update(tx.hash());
        }
        hasher.finalize().into()
    }

    /// Get total fees from all transactions.
    ///
    /// Per-transaction fees are attacker-influenced, so this accumulates with
    /// `saturating_add`: a crafted block whose fees sum past `u64::MAX` clamps
    /// to `u64::MAX` rather than wrapping silently (release, `overflow-checks =
    /// false`) or panicking (debug). See issue #599.
    ///
    /// Caller contract: this is *not* an authoritative fee total for consensus
    /// arithmetic. A saturated value only ever fails validation — it cannot
    /// match a block's declared lottery split — and any block with an
    /// overflowing fee total is rejected outright by the ledger via a checked
    /// accumulation in `add_block_inner` (`LedgerError::FeeOverflow`). Callers
    /// use it only for the "are there any fees at all?" gate and for lottery
    /// accounting that is separately re-checked against the block's summary.
    pub fn total_fees(&self) -> u64 {
        self.transactions
            .iter()
            .fold(0u64, |acc, tx| acc.saturating_add(tx.fee))
    }
}

/// Calculate block reward using block-height-based halving.
///
/// This is the authoritative reward calculation for minting transactions.
/// Uses `MonetaryPolicy` which assumes 5-second blocks. When actual blocks
/// are slower (up to 40s when idle), effective inflation is proportionally
/// lower.
///
/// # Arguments
/// * `height` - Current block height
/// * `total_supply` - Current total supply (for tail emission calculation)
///
/// # Returns
/// The block reward for the given height.
pub fn calculate_block_reward(height: u64, total_supply: u128) -> u64 {
    let policy = crate::monetary::mainnet_policy();

    // Check which phase we're in
    if policy.is_halving_phase(height) {
        // Phase 1: Halving schedule based on block height (independent of
        // supply — the halving branch ignores `total_supply` entirely).
        policy.halving_reward(height).unwrap_or(1)
    } else {
        // Phase 2: Calculate tail reward based on supply.
        //
        // By the time the chain reaches Phase 2 the real picocredit supply
        // (~1.2e21) far exceeds `u64::MAX`, so the `u128` entry point must be
        // used — truncating through the `u64` API would compute a wildly
        // wrong tail reward. The formula used to be reimplemented locally
        // while the cluster-tax crate was `u64`-only (#333); the #694
        // picocredit migration widened the crate itself, so this now
        // delegates to the single canonical implementation.
        calculate_tail_reward_u128(&policy, total_supply)
    }
}

/// `u128`-supply tail-reward computation.
///
/// Thin wrapper over
/// [`bth_cluster_tax::MonetaryPolicy::calculate_tail_reward_u128`],
/// kept so existing callers/tests retain their entry point. The `supply`
/// input is `u128` so realistic picocredit supplies above `u64::MAX` do not
/// truncate. See #333/#694.
fn calculate_tail_reward_u128(
    policy: &bth_cluster_tax::MonetaryPolicy,
    supply_at_transition: u128,
) -> u64 {
    policy.calculate_tail_reward_u128(supply_at_transition)
}

/// Dynamic block timing based on network load.
///
/// Adjusts block time to balance:
/// - Overhead efficiency (slower when idle)
/// - Finality latency (faster under load)
///
/// Uses discrete levels for stability and predictability.
///
/// # Relationship with Monetary Policy
///
/// This module controls **actual block production timing** (3-40s), while
/// `monetary.rs::mainnet_policy()` uses a **fixed 5s assumption** for economic
/// calculations (halving schedule, emission rate, inflation projections).
///
/// When actual blocks are slower than 5s (e.g., 20s during moderate load),
/// effective inflation is proportionally lower. This creates a natural
/// "inflation dampener" where busy networks get full emission and idle
/// networks preserve value.
///
/// See `docs/architecture.md#block-timing-architecture` for the full design.
pub mod dynamic_timing {
    use super::Block;

    // Superseded (issue #761): this constant was never consumed anywhere and
    // contradicted the live 3 s floor. The single authority for the minimum
    // block time is `crate::consensus::ConsensusConfig::MIN_BLOCK_TIME_SECS`
    // (= 3), which matches the fastest tier of `BLOCK_TIME_LEVELS` below (a
    // compile-time assertion at the bottom of this module keeps them in sync).
    // Kept commented-out rather than deleted per CLAUDE.md code preservation.
    //
    // /// Minimum block time (consensus floor - SCP needs time to complete)
    // pub const MIN_BLOCK_TIME: u64 = 5;

    /// Maximum block time (efficiency ceiling when idle)
    pub const MAX_BLOCK_TIME: u64 = 40;

    /// Number of recent blocks to analyze for load estimation
    pub const SMOOTHING_WINDOW: usize = 10;

    /// Block metadata overhead in bytes (header + minting_tx)
    pub const BLOCK_METADATA_SIZE: u64 = 476;

    /// Average transaction size estimate (CLSAG 1-in-2-out)
    pub const AVG_TX_SIZE: u64 = 2800;

    /// Discrete block time levels: (tx_rate_threshold, block_time_secs)
    /// Higher load → faster blocks, lower load → slower blocks
    pub const BLOCK_TIME_LEVELS: [(f64, u64); 5] = [
        (20.0, 3), // Very high load: 20+ tx/s → 3s blocks
        (5.0, 5),  // High load: 5+ tx/s → 5s blocks
        (1.0, 10), // Medium load: 1+ tx/s → 10s blocks
        (0.2, 20), // Low load: 0.2+ tx/s → 20s blocks
        (0.0, 40), // Idle: <0.2 tx/s → 40s blocks
    ];

    /// Compile-time assertion: the consensus floor
    /// (`ConsensusConfig::MIN_BLOCK_TIME_SECS`) must equal the fastest tier of
    /// `BLOCK_TIME_LEVELS` so the two can never drift apart again (issue
    /// #761). Only the u64 block-time component matters here; the f64 tx-rate
    /// thresholds are irrelevant to the floor.
    const _: () = assert!(
        crate::consensus::ConsensusConfig::MIN_BLOCK_TIME_SECS == BLOCK_TIME_LEVELS[0].1,
        "ConsensusConfig::MIN_BLOCK_TIME_SECS must match the fastest BLOCK_TIME_LEVELS tier"
    );

    /// Compile-time assertion: `MAX_BLOCK_TIME` must equal the slowest (idle)
    /// tier of `BLOCK_TIME_LEVELS`.
    const _: () = assert!(
        MAX_BLOCK_TIME == BLOCK_TIME_LEVELS[BLOCK_TIME_LEVELS.len() - 1].1,
        "MAX_BLOCK_TIME must match the slowest BLOCK_TIME_LEVELS tier"
    );

    /// Compute the target block time based on recent transaction load.
    ///
    /// This is deterministic from chain state, so all validators compute
    /// the same value for a given chain tip.
    ///
    /// # Arguments
    /// * `recent_blocks` - The last SMOOTHING_WINDOW blocks (newest last)
    ///
    /// # Returns
    /// Target block time in seconds
    pub fn compute_block_time(recent_blocks: &[Block]) -> u64 {
        if recent_blocks.len() < 2 {
            // Not enough data, use default
            return 20;
        }

        // Compute total transaction count in the window
        // (We use tx count rather than bytes since we'd need to serialize for exact
        // bytes)

        // Compute time span of the window
        let first_time = recent_blocks
            .first()
            .map(|b| b.header.timestamp)
            .unwrap_or(0);
        let last_time = recent_blocks
            .last()
            .map(|b| b.header.timestamp)
            .unwrap_or(0);
        let window_time = last_time.saturating_sub(first_time);

        if window_time == 0 {
            return 20; // Avoid division by zero
        }

        // Compute transaction rate (tx/sec)
        let total_txs: usize = recent_blocks.iter().map(|b| b.transactions.len()).sum();
        let tx_rate = total_txs as f64 / window_time as f64;

        // Find appropriate level
        for (threshold, block_time) in BLOCK_TIME_LEVELS {
            if tx_rate >= threshold {
                return block_time;
            }
        }

        MAX_BLOCK_TIME
    }

    /// Compute the overhead percentage at a given block time and tx rate.
    ///
    /// Returns the percentage of ledger space consumed by block metadata
    /// vs actual transaction data.
    pub fn compute_overhead_percent(block_time: u64, tx_rate: f64) -> f64 {
        let tx_bytes_per_block = tx_rate * block_time as f64 * AVG_TX_SIZE as f64;
        let total_bytes = BLOCK_METADATA_SIZE as f64 + tx_bytes_per_block;

        if total_bytes == 0.0 {
            return 100.0;
        }

        (BLOCK_METADATA_SIZE as f64 / total_bytes) * 100.0
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_block_time_levels() {
            // Verify levels are sorted descending by threshold
            let mut prev_threshold = f64::MAX;
            for (threshold, _) in BLOCK_TIME_LEVELS {
                assert!(
                    threshold < prev_threshold,
                    "Levels must be sorted descending"
                );
                prev_threshold = threshold;
            }
        }

        #[test]
        fn test_overhead_calculation() {
            // At 1 tx/s with 20s blocks: 20 txs per block
            // 20 * 2800 = 56000 bytes of tx data
            // 476 / (476 + 56000) = 0.84% overhead
            let overhead = compute_overhead_percent(20, 1.0);
            assert!(
                overhead < 1.0,
                "1 tx/s at 20s blocks should have <1% overhead"
            );

            // At 0.1 tx/s with 20s blocks: 2 txs per block
            // 2 * 2800 = 5600 bytes of tx data
            // 476 / (476 + 5600) = 7.8% overhead
            let overhead = compute_overhead_percent(20, 0.1);
            assert!(
                overhead > 5.0 && overhead < 10.0,
                "0.1 tx/s at 20s should be ~8% overhead"
            );
        }
    }
}

/// Difficulty as a monetary policy feedback controller.
///
/// Difficulty is the control variable that adjusts minting rate to hit targets:
///
/// **Phase 1 (Halving, ~10 years)**: High initial rewards to drive adoption
///   - Halving schedule based on cumulative transaction count
///   - Difficulty adjusts to hit target emission per tx-epoch
///
/// **Phase 2 (Tail emission)**: Sustainable 2% net inflation
///   - Net inflation = gross emission - fee burns
///   - Difficulty adjusts to maintain 2% target
///
/// The feedback loop:
/// ```text
///                        error
/// target_emission ──────────┐
///         rate              ▼
///                     ┌───────────┐
/// actual_emission ───>│ PI control│──> difficulty
///         rate        └───────────┘
/// ```
pub mod difficulty {
    use crate::node::minter::INITIAL_DIFFICULTY;

    // --- Legacy constants for backward compatibility ---

    /// Legacy: blocks between adjustments (for old block-based code)
    pub const ADJUSTMENT_WINDOW: u64 = 180;

    /// Legacy: target block time for old adjustment logic
    pub const TARGET_BLOCK_TIME: u64 = 20;

    // --- Core constants ---

    /// Minimum difficulty (floor to prevent stuck chain)
    pub const MIN_DIFFICULTY: u64 = 1;

    /// Maximum difficulty (ceiling).
    ///
    /// Convention (see [`crate::block::BlockHeader::is_valid_pow`]): PoW is
    /// valid when `pow_value(hash) < difficulty`, so a HIGHER numeric
    /// difficulty admits MORE hashes and is therefore EASIER.
    /// `MAX_DIFFICULTY` is the *easiest* allowed PoW.
    ///
    /// M5 (#554): previously this equalled `INITIAL_DIFFICULTY`, which pinned
    /// the easiest-possible PoW to the genesis assumption. If real network
    /// hashrate fell below that assumption the chain could never ease
    /// difficulty past genesis, so block production slowed/stalled with no
    /// self-correction — a low-hashrate net could not self-heal. We raise
    /// the ceiling well above genesis so the time-based controller can ease
    /// PoW below the genesis difficulty and recover.
    ///
    /// The bound is kept finite (not `u64::MAX`) so PoW always retains *some*
    /// cost. `INITIAL_DIFFICULTY` is `2^64 / ~342` expected hashes/block, i.e.
    /// ~342 expected hashes per block at genesis. `MAX_EASE_MULTIPLE = 256`
    /// lets block production survive a ~256x hashrate collapse (e.g. a
    /// multi-node testnet dropping to a single weak miner) while still
    /// requiring real work: the easiest PoW still expects `342 / 256 ≈ 1.3`
    /// hashes/block. The product `INITIAL_DIFFICULTY * 256 ≈ 1.38e19` stays
    /// below `u64::MAX (1.84e19)`, so the ceiling is a genuine finite bound
    /// rather than a saturated `u64::MAX` (the `saturating_mul` below is
    /// belt-and-suspenders against recalibration).
    pub const MAX_EASE_MULTIPLE: u64 = 256;

    /// Maximum difficulty (ceiling). See [`MAX_EASE_MULTIPLE`].
    ///
    /// Computed as `INITIAL_DIFFICULTY * MAX_EASE_MULTIPLE`, saturating at
    /// `u64::MAX` so the constant is always well-defined regardless of the
    /// genesis calibration.
    pub const MAX_DIFFICULTY: u64 = INITIAL_DIFFICULTY.saturating_mul(MAX_EASE_MULTIPLE);

    /// Target inter-block time in seconds for the time-based difficulty
    /// controller (M5, #554). Mirrors `MonetaryPolicy::target_block_time_secs`
    /// (5 s) and the genesis difficulty calibration in
    /// [`crate::node::minter::INITIAL_DIFFICULTY`].
    pub const TARGET_BLOCK_TIME_SECS: u64 = 5;

    /// Upper bound on the observed inter-block time (seconds) the controller
    /// will act on, as a multiple of [`TARGET_BLOCK_TIME_SECS`]. Caps how
    /// far a single (consensus-bounded but still loose) timestamp gap can
    /// push difficulty in one step, complementing the multiplicative [0.5x,
    /// 2.0x] clamp. At 5 s target this admits gaps up to `5 * 12 = 60 s`.
    pub const MAX_OBSERVED_BLOCK_TIME_MULTIPLE: u64 = 12;

    /// Fixed-point scale for difficulty-adjustment ratios.
    ///
    /// `RATIO_SCALE` basis-point units == 1.0x. All difficulty math is integer
    /// (u128) basis-point arithmetic — CONSENSUS-CRITICAL: difficulty is
    /// hard-validated chain state (rejected on mismatch in
    /// `ledger::store::add_block` and minting-tx validation), so every node on
    /// every platform must compute the identical value. f64 must never enter
    /// this path (see audit cycle-6 C5, issue #552).
    pub const RATIO_SCALE: u128 = 10_000;

    /// Maximum adjustment factor per epoch (damping), expressed in basis points
    /// (`RATIO_SCALE` == 1.0x). `2 * RATIO_SCALE` == 2.0x. The per-epoch
    /// difficulty multiplier is clamped to
    /// `[RATIO_SCALE / 2, MAX_ADJUSTMENT_FACTOR_BPS]` == `[0.5x, 2.0x]`.
    pub const MAX_ADJUSTMENT_FACTOR_BPS: u128 = 2 * RATIO_SCALE;

    /// Transactions per difficulty adjustment epoch.
    /// Adjustment frequency scales with network usage.
    pub const ADJUSTMENT_TX_COUNT: u64 = 1000;

    /// Initial block reward (50 BTH in picocredits)
    /// Note: Block rewards are now calculated by `calculate_block_reward()`
    /// using MonetaryPolicy with block-height-based halving (5s block
    /// assumption).
    pub const INITIAL_REWARD: u64 = 50_000_000_000_000;

    /// Monetary policy phase (for display/informational purposes).
    /// Note: Block rewards are now calculated by `calculate_block_reward()`
    /// using MonetaryPolicy with block-height-based halving.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Phase {
        /// Halving phase with epoch number (0-indexed)
        Halving { epoch: u32 },
        /// Tail emission phase
        Tail,
    }

    /// Emission controller: difficulty adjustment based on emission rate.
    ///
    /// Tracks network state and adjusts difficulty to hit emission targets.
    /// Note: Block rewards are calculated separately by
    /// `calculate_block_reward()`. This controller focuses on difficulty
    /// adjustment only.
    #[derive(Debug, Clone)]
    pub struct EmissionController {
        // --- State ---
        /// Current PoW difficulty
        pub difficulty: u64,
        /// Cumulative transactions (for difficulty adjustment timing)
        pub total_tx: u64,
        /// Cumulative gross emission (picocredits minted). `u128`: mirrors
        /// `ChainState.total_mined` (restored via `from_chain_state`) and
        /// crosses `u64::MAX` over Phase 1 — keeping it `u64` would
        /// re-introduce the silent wrap #333 fixes.
        pub total_emitted: u128,
        /// Cumulative fees burned (picocredits). `u128` for the same reason as
        /// `total_emitted`.
        pub total_burned: u128,

        // --- Current epoch accumulators ---
        /// Tx in current adjustment epoch
        pub epoch_tx: u64,
        /// Emission in current epoch
        pub epoch_emission: u64,
        /// Burns in current epoch
        pub epoch_burns: u64,

        // --- Derived (for backward compatibility with persistence) ---
        /// Current block reward (informational, actual rewards use
        /// calculate_block_reward)
        pub current_reward: u64,
    }

    impl Default for EmissionController {
        fn default() -> Self {
            Self::new(INITIAL_DIFFICULTY)
        }
    }

    impl EmissionController {
        pub fn new(initial_difficulty: u64) -> Self {
            Self {
                difficulty: initial_difficulty,
                total_tx: 0,
                total_emitted: 0,
                total_burned: 0,
                epoch_tx: 0,
                epoch_emission: 0,
                epoch_burns: 0,
                current_reward: INITIAL_REWARD,
            }
        }

        /// Restore from persisted chain state
        pub fn from_chain_state(
            difficulty: u64,
            total_mined: u128,
            total_fees_burned: u128,
            total_tx: u64,
            epoch_tx: u64,
            epoch_emission: u64,
            epoch_burns: u64,
            current_reward: u64,
        ) -> Self {
            Self {
                difficulty,
                total_tx,
                total_emitted: total_mined,
                total_burned: total_fees_burned,
                epoch_tx,
                epoch_emission,
                epoch_burns,
                current_reward,
            }
        }

        /// Current monetary phase (deprecated - use MonetaryPolicy for
        /// block-based halving) This is kept for informational purposes
        /// and backward compatibility.
        pub fn phase(&self) -> Phase {
            // Use a simplified approximation based on total emission
            // The actual halving is now block-height-based via MonetaryPolicy
            let policy = crate::monetary::mainnet_policy();

            // Use u128 to avoid overflow with large values
            let initial_reward = INITIAL_REWARD as u128;
            let halving_interval = policy.halving_interval as u128;
            let halving_count = policy.halving_count as u128;
            let total_emitted = self.total_emitted;

            let total_halving_emission = initial_reward * halving_interval * halving_count;
            if total_emitted < total_halving_emission {
                // Rough epoch estimate based on emission
                let per_epoch = initial_reward * halving_interval;
                let epoch = (total_emitted / per_epoch) as u32;
                Phase::Halving {
                    epoch: epoch.min(policy.halving_count - 1),
                }
            } else {
                Phase::Tail
            }
        }

        /// Current block reward (informational - use calculate_block_reward()
        /// for actual rewards)
        pub fn block_reward(&self) -> u64 {
            self.current_reward
        }

        /// Net circulating supply (picocredits).
        pub fn net_supply(&self) -> u128 {
            self.total_emitted.saturating_sub(self.total_burned)
        }

        /// Target emission rate (picocredits per tx) for feedback control
        fn target_emission_per_tx(&self) -> u64 {
            // Expected transactions per block for emission rate calculation
            const EXPECTED_TX_PER_BLOCK: u64 = 20;

            // Use current_reward (which tracks the actual rewards being paid)
            // divided by expected tx per block
            if self.current_reward > 0 {
                self.current_reward / EXPECTED_TX_PER_BLOCK
            } else {
                1 // Fallback to prevent divide by zero in adjustment
            }
        }

        /// Record a finalized block and update the controller.
        ///
        /// M5 (#554): difficulty is now driven by observed **block time** vs
        /// [`TARGET_BLOCK_TIME_SECS`], NOT by transaction count.
        /// `observed_secs` is the inter-block time `block.timestamp -
        /// parent.timestamp` (the caller already holds both via chain
        /// state). A producer can no longer skew difficulty for free by
        /// stuffing or starving blocks with transactions, because the
        /// adjustment signal does not depend on `tx_count` at all.
        ///
        /// `observed_secs == None` (e.g. the very first recorded block after
        /// genesis, where there is no usable parent delta) leaves difficulty
        /// unchanged for that block.
        ///
        /// The emission/burn/tx totals are still accumulated for monetary
        /// reporting and persistence, but no longer gate difficulty.
        ///
        /// Returns (new_difficulty, new_block_reward). The returned
        /// block_reward is informational — actual rewards come from
        /// `calculate_block_reward()` based on block height.
        pub fn record_block(
            &mut self,
            tx_count: u64,
            reward_paid: u64,
            fees_burned: u64,
            observed_secs: Option<u64>,
        ) -> (u64, u64) {
            // Update totals (monetary accounting; independent of difficulty).
            self.total_tx += tx_count;
            self.total_emitted += reward_paid as u128;
            self.total_burned += fees_burned as u128;

            // Update epoch accumulators (retained for persistence/back-compat;
            // no longer used to trigger difficulty adjustment — see M5).
            self.epoch_tx += tx_count;
            self.epoch_emission += reward_paid;
            self.epoch_burns += fees_burned;

            // Track the reward for informational purposes.
            self.current_reward = reward_paid;

            // Time-driven difficulty adjustment: one step per block, off the
            // observed inter-block time. Producer-skew-resistant because it
            // ignores tx_count entirely.
            if let Some(observed) = observed_secs {
                self.difficulty = Self::compute_time_adjusted_difficulty(
                    self.difficulty,
                    observed,
                    TARGET_BLOCK_TIME_SECS,
                );
            }

            (self.difficulty, self.current_reward)
        }

        /// LEGACY (pre-M5, #554): emission-rate, tx-count-epoch difficulty
        /// adjustment. Preserved (CLAUDE.md: stash rather than delete) and kept
        /// integer-deterministic, but NO LONGER on the live block path — it was
        /// producer-skewable because the trigger and signal were tx-count
        /// driven. Retained so the prior controller can be exercised in tests
        /// and recovered if the time-based controller needs to be reverted.
        ///
        /// CONSENSUS-CRITICAL contract (when used): pure integer (u128
        /// basis-point) arithmetic — no f64 (audit cycle-6 C5, issue #552).
        #[allow(dead_code)]
        fn adjust_difficulty_emission_legacy(&mut self) {
            self.difficulty = Self::compute_adjusted_difficulty(
                self.difficulty,
                self.epoch_emission,
                self.epoch_tx,
                self.target_emission_per_tx(),
            );
            self.reset_epoch();
        }

        /// Pure, deterministic **time-based** difficulty-adjustment kernel
        /// (M5, #554).
        ///
        /// Adjusts difficulty so observed block time converges to
        /// `target_secs`, using the block.rs convention (HIGHER numeric
        /// difficulty = EASIER PoW; PoW valid iff `pow_value(hash) <
        /// difficulty`):
        ///   - blocks too SLOW (`observed > target`) → ease PoW → RAISE
        ///     difficulty
        ///   - blocks too FAST (`observed < target`) → harden PoW → LOWER
        ///     difficulty
        ///
        /// The multiplier is `observed / target` in basis points, clamped to
        /// `[0.5x, 2.0x]` for per-block damping, then applied and clamped to
        /// the absolute `[MIN_DIFFICULTY, MAX_DIFFICULTY]` bounds.
        /// Because `MAX_DIFFICULTY` now exceeds `INITIAL_DIFFICULTY`
        /// (#554), persistently slow blocks can ease PoW *below* the
        /// genesis difficulty so a low-hashrate network self-corrects.
        ///
        /// CONSENSUS-CRITICAL: pure integer (u128 basis-point) arithmetic — no
        /// f64. Difficulty is hard-validated chain state, so this must be
        /// bit-for-bit identical on every platform (preserves the #553/#552
        /// determinism property).
        ///
        /// `pub(crate)` so it can be exercised directly with exact integer
        /// inputs/outputs, independent of controller state.
        pub(crate) fn compute_time_adjusted_difficulty(
            current: u64,
            observed_secs: u64,
            target_secs: u64,
        ) -> u64 {
            // Undefined target → leave difficulty unchanged (defensive; the
            // constant is nonzero).
            if target_secs == 0 {
                return current.clamp(MIN_DIFFICULTY, MAX_DIFFICULTY);
            }

            // Cap how far a single (consensus-bounded but loose) timestamp gap
            // can move difficulty in one step. Combined with the [0.5x, 2.0x]
            // multiplier clamp below this bounds per-block movement even if a
            // producer pushes the timestamp to the future limit.
            let max_observed = target_secs.saturating_mul(MAX_OBSERVED_BLOCK_TIME_MULTIPLE);
            let observed = observed_secs.min(max_observed);

            // Multiplier in basis points (RATIO_SCALE == 1.0x):
            //   adjustment = observed / target
            //   observed > target (too slow) → > 1.0x → RAISE difficulty (easier)
            //   observed < target (too fast) → < 1.0x → LOWER difficulty (harder)
            // observed == 0 (two blocks at the same second) drives the
            // multiplier to 0 and is clamped up to the 0.5x floor below.
            let adjustment_bps: u128 = observed as u128 * RATIO_SCALE / target_secs as u128;

            // Clamp the multiplier to [0.5x, 2.0x] (per-block damping), matching
            // the emission controller's bounds.
            let adjustment_bps = adjustment_bps.clamp(RATIO_SCALE / 2, MAX_ADJUSTMENT_FACTOR_BPS);

            // Apply: new = current * adjustment_bps / RATIO_SCALE (u128 to avoid
            // intermediate overflow), then clamp to absolute difficulty bounds.
            let new_diff = current as u128 * adjustment_bps / RATIO_SCALE;
            let new_diff = u64::try_from(new_diff).unwrap_or(u64::MAX);
            new_diff.clamp(MIN_DIFFICULTY, MAX_DIFFICULTY)
        }

        /// Pure, deterministic difficulty-adjustment kernel.
        ///
        /// Separated out (and `pub(crate)`) so it can be exercised directly in
        /// tests with exact integer inputs/outputs, independent of controller
        /// state. Contains ZERO floating point.
        ///
        /// - `current` is the current difficulty.
        /// - `epoch_emission` / `epoch_tx` give the actual emission-per-tx.
        /// - `target` is `target_emission_per_tx()`.
        ///
        /// Returns the new difficulty, clamped to `[MIN_DIFFICULTY,
        /// MAX_DIFFICULTY]`.
        pub(crate) fn compute_adjusted_difficulty(
            current: u64,
            epoch_emission: u64,
            epoch_tx: u64,
            target: u64,
        ) -> u64 {
            // No epoch data or undefined target → leave difficulty unchanged.
            if epoch_tx == 0 || target == 0 {
                return current.clamp(MIN_DIFFICULTY, MAX_DIFFICULTY);
            }

            // Actual emission per tx this epoch (integer floor, same as before).
            let actual = epoch_emission / epoch_tx;

            // Adjustment multiplier in basis points (RATIO_SCALE == 1.0x):
            //   adjustment = target / actual
            // This is the integer form of the old `1.0 / (actual/target)`.
            //   actual > target → adjustment < 1.0x → LOWER difficulty (harder)
            //   actual < target → adjustment > 1.0x → HIGHER difficulty (easier)
            // When actual == 0 the multiplier saturates upward and is then
            // clamped to the max factor below (matches the f64 path, where a
            // near-zero ratio drove `1/ratio` to the MAX_ADJUSTMENT_FACTOR cap).
            let adjustment_bps: u128 = if actual == 0 {
                MAX_ADJUSTMENT_FACTOR_BPS
            } else {
                target as u128 * RATIO_SCALE / actual as u128
            };

            // Clamp the multiplier to [0.5x, 2.0x] (damping). The lower bound
            // is RATIO_SCALE / 2 (== 0.5x == 1 / MAX_ADJUSTMENT_FACTOR) and the
            // upper bound is MAX_ADJUSTMENT_FACTOR_BPS (== 2.0x), mirroring the
            // old `clamp(1.0 / MAX_ADJUSTMENT_FACTOR, MAX_ADJUSTMENT_FACTOR)`.
            let adjustment_bps = adjustment_bps.clamp(RATIO_SCALE / 2, MAX_ADJUSTMENT_FACTOR_BPS);

            // Apply: new = current * adjustment_bps / RATIO_SCALE (u128 to avoid
            // intermediate overflow), then clamp to absolute difficulty bounds.
            let new_diff = current as u128 * adjustment_bps / RATIO_SCALE;
            let new_diff = u64::try_from(new_diff).unwrap_or(u64::MAX);
            new_diff.clamp(MIN_DIFFICULTY, MAX_DIFFICULTY)
        }

        fn reset_epoch(&mut self) {
            self.epoch_tx = 0;
            self.epoch_emission = 0;
            self.epoch_burns = 0;
        }

        /// Deprecated: Halving is now block-height-based, not tx-based.
        /// Use MonetaryPolicy.halving_interval and block height to calculate
        /// blocks until halving. Returns 0 as a placeholder.
        #[deprecated(note = "Halving is now block-height-based via MonetaryPolicy")]
        pub fn tx_until_halving(&self) -> u64 {
            0
        }

        /// Estimated current inflation rate (bps)
        pub fn current_inflation_bps(&self) -> u64 {
            let supply = self.net_supply();
            if supply == 0 || self.total_tx == 0 {
                return 0;
            }
            // Net emission per tx, annualized assuming 10M tx/year
            let net_per_tx =
                self.total_emitted.saturating_sub(self.total_burned) / self.total_tx as u128;
            let annual = net_per_tx * 10_000_000;
            (annual * 10_000 / supply) as u64
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_initial_state() {
            let ctrl = EmissionController::new(1000);
            // Initial phase is Halving epoch 0 (based on zero emission)
            assert_eq!(ctrl.phase(), Phase::Halving { epoch: 0 });
            assert_eq!(ctrl.block_reward(), INITIAL_REWARD);
        }

        #[test]
        fn test_phase_estimation() {
            // Phase is now estimated from total_emitted, not total_tx
            // Note: phase() is deprecated for EmissionController - use MonetaryPolicy
            // instead
            let mut ctrl = EmissionController::new(1000);

            // At zero emission, should be epoch 0
            assert_eq!(ctrl.phase(), Phase::Halving { epoch: 0 });

            // The phase() method approximates based on emission vs expected
            // schedule With very large halving intervals (12.6M
            // blocks), we need significant emission to advance
            // phases. For practical purposes, phase 0 is expected
            // for any reasonable emission amount in tests.
        }

        #[test]
        fn test_tail_phase_threshold() {
            // Note: With block-based halving (5s block assumption), the halving interval
            // is 12.6M blocks. The total emission for all halvings exceeds u64::MAX
            // (3.15 * 10^21 picocredits), so we cannot represent tail phase emission
            // in a u64 total_emitted field.
            //
            // This test verifies the phase() method correctly handles the math using u128
            // and that very high emission values are still in halving phase (since the
            // threshold is larger than u64::MAX).
            let mut ctrl = EmissionController::new(1000);
            ctrl.total_emitted = u64::MAX as u128;

            // With the current constants, even u64::MAX is still in halving phase
            // because total_halving_emission > u64::MAX
            let phase = ctrl.phase();
            assert!(matches!(phase, Phase::Halving { .. }));
        }

        #[test]
        fn test_difficulty_adjustment() {
            let mut ctrl = EmissionController::new(1000);

            // M5 (#554): tx accumulation no longer drives difficulty. Recording
            // blocks without a block-time signal (None) accumulates totals but
            // leaves difficulty untouched — the old tx-count epoch trigger is
            // gone (it was producer-skewable).
            for _ in 0..10 {
                ctrl.record_block(100, INITIAL_REWARD, 0, None);
            }
            assert_eq!(ctrl.total_tx, 1000);
            assert_eq!(
                ctrl.difficulty, 1000,
                "tx count alone must never move difficulty (M5)"
            );
        }

        /// LEGACY (pre-M5): direction of the now-retired emission-rate loop,
        /// exercised via the preserved pure kernel
        /// `compute_adjusted_difficulty` (the stateful tx-count trigger
        /// was removed in #554 as producer-skewable). Kept to guard the
        /// legacy kernel against drift in case it is ever revived.
        #[test]
        fn test_emission_kernel_adjustment_direction_legacy() {
            let start_difficulty = INITIAL_DIFFICULTY / 2;

            // Over-emitting (actual >> target) → harder → lower numeric.
            // actual = INITIAL_REWARD/1 = INITIAL_REWARD; target = reward/20.
            let fast = EmissionController::compute_adjusted_difficulty(
                start_difficulty,
                INITIAL_REWARD,
                1,
                INITIAL_REWARD / 20,
            );
            assert!(
                fast < start_difficulty,
                "over-emitting must lower the numeric difficulty (harder), got {} from {}",
                fast,
                start_difficulty
            );

            // Under-emitting (actual < target) → easier → higher numeric.
            // actual = 100/1000 = 0; target = 100/20 = 5.
            let slow =
                EmissionController::compute_adjusted_difficulty(start_difficulty, 100, 1000, 5);
            assert!(
                slow > start_difficulty,
                "under-emitting must raise the numeric difficulty (easier), got {} from {}",
                slow,
                start_difficulty
            );
        }

        // === C5 (#552): integer, f64-free difficulty controller ===

        /// Determinism: the same (difficulty, epoch_emission, epoch_tx, target)
        /// inputs must produce the identical output, every time, on any
        /// platform. Integer-only math makes this exact (the whole point of
        /// eliminating f64 from hard-validated consensus state).
        #[test]
        fn test_compute_adjusted_difficulty_deterministic() {
            // A spread of representative inputs (current, emission, tx, target).
            let cases = [
                (10_000u64, 1_000_000u64, 1_000u64, 50u64),
                (5_000, 0, 1_000, 50),
                (1_234, 999_999, 777, 13),
                (INITIAL_DIFFICULTY, INITIAL_REWARD, 1, 1),
                (42, 100, 1_000, 5),
            ];
            for &(cur, emit, tx, target) in &cases {
                let first = EmissionController::compute_adjusted_difficulty(cur, emit, tx, target);
                // Recompute many times; must be byte-identical every time.
                for _ in 0..32 {
                    let again =
                        EmissionController::compute_adjusted_difficulty(cur, emit, tx, target);
                    assert_eq!(
                        first, again,
                        "non-deterministic output for ({cur},{emit},{tx},{target})"
                    );
                }
            }
        }

        /// Exact integer outputs at representative inputs. Pinning the precise
        /// values guards against silent algorithm drift and proves the math is
        /// integer (no rounding ambiguity). Hand-derived:
        ///   adjustment_bps = clamp(target * 10000 / actual, 5000, 20000)
        ///   new = clamp(current * adjustment_bps / 10000, MIN, MAX)
        #[test]
        fn test_compute_adjusted_difficulty_exact_values() {
            // actual = 1_000_000 / 1000 = 1000; target = 50.
            // adj = clamp(50*10000/1000, 5000, 20000) = clamp(500, ..) = 5000.
            // new = 10000 * 5000 / 10000 = 5000.
            assert_eq!(
                EmissionController::compute_adjusted_difficulty(10_000, 1_000_000, 1_000, 50),
                5_000
            );

            // Balanced: actual == target == 50 → adj = 10000 (1.0x) → unchanged.
            // emission/tx = 50_000/1000 = 50.
            assert_eq!(
                EmissionController::compute_adjusted_difficulty(8_000, 50_000, 1_000, 50),
                8_000
            );

            // Under-emit: actual = 20_000/1000 = 20 < target 50.
            // adj = clamp(50*10000/20, 5000, 20000) = clamp(25000,..) = 20000.
            // new = 8000 * 20000 / 10000 = 16000, but MAX_DIFFICULTY clamps it.
            let mid = INITIAL_DIFFICULTY / 2;
            let raised = EmissionController::compute_adjusted_difficulty(mid, 20_000, 1_000, 50);
            assert!(raised > mid && raised <= MAX_DIFFICULTY);

            // actual == 0 → max upward multiplier (2.0x), then clamps.
            let from = MIN_DIFFICULTY.max(3);
            let up = EmissionController::compute_adjusted_difficulty(from, 0, 1_000, 50);
            assert_eq!(up, (from as u128 * 2).min(MAX_DIFFICULTY as u128) as u64);
        }

        /// Direction parity with the prior f64 intent in the block.rs
        /// convention (LOWER numeric difficulty = HARDER):
        ///   emitting too fast (actual > target) → harder → lower difficulty
        ///   emitting too slow (actual < target) → easier → higher difficulty
        #[test]
        fn test_compute_adjusted_difficulty_direction() {
            let mid = INITIAL_DIFFICULTY / 2;

            // Too fast: actual = 100_000/1000 = 100 >> target 5 → harder.
            let faster = EmissionController::compute_adjusted_difficulty(mid, 100_000, 1_000, 5);
            assert!(faster < mid, "over-emit must lower difficulty (harder)");

            // Too slow: actual = 1000/1000 = 1 < target 50 → easier.
            let slower = EmissionController::compute_adjusted_difficulty(mid, 1_000, 1_000, 50);
            assert!(slower > mid, "under-emit must raise difficulty (easier)");
        }

        /// Clamp bounds: the per-epoch multiplier is limited to [0.5x, 2.0x]
        /// and the result is limited to [MIN_DIFFICULTY, MAX_DIFFICULTY].
        #[test]
        fn test_compute_adjusted_difficulty_clamps() {
            // Extreme over-emission would push toward 0; multiplier floored at
            // 0.5x. current = 10_000, actual huge → adj = 5000 → new = 5000.
            assert_eq!(
                EmissionController::compute_adjusted_difficulty(10_000, u64::MAX / 2, 1, 1),
                5_000
            );

            // Extreme under-emission floored at 2.0x: actual = 0.
            // current small so 2.0x stays under MAX.
            let small = MAX_DIFFICULTY / 4;
            assert_eq!(
                EmissionController::compute_adjusted_difficulty(small, 0, 1_000, 1_000_000),
                (small * 2).min(MAX_DIFFICULTY)
            );

            // Absolute floor: starting at 1, 0.5x → 0, clamped up to
            // MIN_DIFFICULTY.
            assert_eq!(
                EmissionController::compute_adjusted_difficulty(1, u64::MAX / 2, 1, 1),
                MIN_DIFFICULTY
            );

            // Absolute ceiling: huge 2.0x push clamps to MAX_DIFFICULTY.
            assert_eq!(
                EmissionController::compute_adjusted_difficulty(MAX_DIFFICULTY, 0, 1, u64::MAX),
                MAX_DIFFICULTY
            );
        }

        /// No-op guards: zero epoch_tx or zero target leave difficulty
        /// unchanged (only re-clamped to absolute bounds).
        #[test]
        fn test_compute_adjusted_difficulty_noop_guards() {
            assert_eq!(
                EmissionController::compute_adjusted_difficulty(1234, 5_000, 0, 50),
                1234
            );
            assert_eq!(
                EmissionController::compute_adjusted_difficulty(1234, 5_000, 1_000, 0),
                1234
            );
        }

        /// M5 (#554): the stateful `record_block` time-path agrees exactly with
        /// the pure time-based kernel, and `None` (no parent delta) is a no-op.
        #[test]
        fn test_record_block_time_path_matches_kernel() {
            let start = INITIAL_DIFFICULTY / 2;

            // No block-time signal → no adjustment.
            let mut ctrl = EmissionController::new(start);
            ctrl.record_block(7, INITIAL_REWARD, 0, None);
            assert_eq!(
                ctrl.difficulty, start,
                "difficulty must not change without an observed block time"
            );

            // With an observed block time, the stateful result must equal the
            // pure kernel on the same inputs.
            let observed = TARGET_BLOCK_TIME_SECS * 2;
            let mut ctrl2 = EmissionController::new(start);
            ctrl2.record_block(7, INITIAL_REWARD, 0, Some(observed));
            let expected = EmissionController::compute_time_adjusted_difficulty(
                start,
                observed,
                TARGET_BLOCK_TIME_SECS,
            );
            assert_eq!(ctrl2.difficulty, expected);
        }

        #[test]
        fn test_fee_burn_tracking() {
            let mut ctrl = EmissionController::new(1000);
            ctrl.record_block(10, 1000, 100, None);

            assert_eq!(ctrl.total_burned, 100);
            assert_eq!(ctrl.net_supply(), 900);
        }

        #[test]
        fn test_current_reward_tracks_paid() {
            let mut ctrl = EmissionController::new(1000);

            // Record a block with specific reward
            ctrl.record_block(5, 12345, 100, None);

            // current_reward should track the last reward paid
            assert_eq!(ctrl.current_reward, 12345);
        }

        // === M5 (#554): time-based, producer-skew-resistant difficulty ===

        /// Producer-skew resistance: difficulty must NOT depend on tx_count.
        /// Recording blocks at the target block time leaves difficulty exactly
        /// unchanged regardless of whether each block carries 0, 1, or a huge
        /// number of transactions — so a producer cannot move difficulty by
        /// stuffing or starving blocks with txs (the core M5 bug).
        #[test]
        fn test_difficulty_independent_of_tx_count() {
            let start = INITIAL_DIFFICULTY;
            let on_target = Some(TARGET_BLOCK_TIME_SECS);

            // Empty blocks at target time.
            let mut starved = EmissionController::new(start);
            for _ in 0..50 {
                starved.record_block(0, INITIAL_REWARD, 0, on_target);
            }

            // Stuffed blocks at the SAME (target) time.
            let mut stuffed = EmissionController::new(start);
            for _ in 0..50 {
                stuffed.record_block(100_000, INITIAL_REWARD, 0, on_target);
            }

            assert_eq!(
                starved.difficulty, stuffed.difficulty,
                "tx count must not affect difficulty"
            );
            assert_eq!(
                starved.difficulty, start,
                "on-target block time must leave difficulty unchanged"
            );
        }

        /// Time-based direction: slow blocks ease PoW (raise numeric
        /// difficulty), fast blocks harden PoW (lower numeric
        /// difficulty). Driven through the stateful `record_block` path
        /// with `observed_secs`.
        #[test]
        fn test_record_block_time_direction() {
            let start = INITIAL_DIFFICULTY / 2;

            // Slow: observed 2x target → easier (higher numeric).
            let mut slow = EmissionController::new(start);
            slow.record_block(7, INITIAL_REWARD, 0, Some(TARGET_BLOCK_TIME_SECS * 2));
            assert!(
                slow.difficulty > start,
                "slow blocks must raise difficulty (ease PoW), got {} from {}",
                slow.difficulty,
                start
            );

            // Fast: observed half target → harder (lower numeric).
            let mut fast = EmissionController::new(start);
            fast.record_block(7, INITIAL_REWARD, 0, Some(TARGET_BLOCK_TIME_SECS / 2));
            assert!(
                fast.difficulty < start,
                "fast blocks must lower difficulty (harden PoW), got {} from {}",
                fast.difficulty,
                start
            );

            // None (no parent delta) leaves difficulty unchanged.
            let mut noop = EmissionController::new(start);
            noop.record_block(7, INITIAL_REWARD, 0, None);
            assert_eq!(noop.difficulty, start);
        }

        /// A low-hashrate network must be able to ease PoW BELOW genesis. With
        /// `MAX_DIFFICULTY == INITIAL_DIFFICULTY` (the old floor) this was
        /// impossible and the chain could not self-heal. Persistently slow
        /// blocks starting AT genesis must push difficulty strictly above
        /// `INITIAL_DIFFICULTY` (= easier than genesis).
        #[test]
        fn test_ease_below_genesis() {
            // Sanity: the ceiling was raised above genesis.
            assert!(
                MAX_DIFFICULTY > INITIAL_DIFFICULTY,
                "MAX_DIFFICULTY must exceed INITIAL_DIFFICULTY for self-healing"
            );

            let mut ctrl = EmissionController::new(INITIAL_DIFFICULTY);
            // Network far slower than target (capped observed time), every block.
            let slow = Some(TARGET_BLOCK_TIME_SECS * MAX_OBSERVED_BLOCK_TIME_MULTIPLE);
            for _ in 0..20 {
                ctrl.record_block(1, INITIAL_REWARD, 0, slow);
            }
            assert!(
                ctrl.difficulty > INITIAL_DIFFICULTY,
                "PoW must be able to ease below genesis (difficulty {} should \
                 exceed INITIAL_DIFFICULTY {})",
                ctrl.difficulty,
                INITIAL_DIFFICULTY
            );
            assert!(ctrl.difficulty <= MAX_DIFFICULTY);
        }

        /// Determinism of the time-based kernel: identical inputs → identical
        /// output, every time (integer-only; no f64). Preserves the #553/#552
        /// property for the new control path.
        #[test]
        fn test_compute_time_adjusted_difficulty_deterministic() {
            let cases = [
                (INITIAL_DIFFICULTY, 5u64, 5u64),
                (INITIAL_DIFFICULTY / 2, 10, 5),
                (INITIAL_DIFFICULTY, 2, 5),
                (1_000_000, 0, 5),
                (12_345, 60, 5),
                (INITIAL_DIFFICULTY, 5, 0), // degenerate target
            ];
            for &(cur, obs, tgt) in &cases {
                let first = EmissionController::compute_time_adjusted_difficulty(cur, obs, tgt);
                for _ in 0..32 {
                    assert_eq!(
                        first,
                        EmissionController::compute_time_adjusted_difficulty(cur, obs, tgt),
                        "non-deterministic output for ({cur},{obs},{tgt})"
                    );
                }
            }
        }

        /// Exact integer outputs for the time-based kernel. Hand-derived:
        ///   observed   = min(observed_secs, target * MAX_OBSERVED_MULTIPLE)
        ///   adjustment = clamp(observed * 10000 / target, 5000, 20000)
        ///   new        = clamp(current * adjustment / 10000, MIN, MAX)
        #[test]
        fn test_compute_time_adjusted_difficulty_exact_values() {
            // On target: observed == target → 1.0x → unchanged.
            assert_eq!(
                EmissionController::compute_time_adjusted_difficulty(8_000, 5, 5),
                8_000
            );

            // 2x slow → 2.0x → doubled (within bounds).
            assert_eq!(
                EmissionController::compute_time_adjusted_difficulty(8_000, 10, 5),
                16_000
            );

            // Fast (observed 2 < target 5): raw multiplier 2*10000/5 = 4000,
            // floored to the 0.5x clamp (5000) → 8000 * 5000/10000 = 4000.
            assert_eq!(
                EmissionController::compute_time_adjusted_difficulty(8_000, 2, 5),
                4_000
            );

            // observed == 0 → multiplier clamped up to 0.5x floor → halved.
            assert_eq!(
                EmissionController::compute_time_adjusted_difficulty(8_000, 0, 5),
                4_000
            );

            // Very slow beyond the observed cap → cap applies, still 2.0x max.
            // observed capped to 5 * 12 = 60; 60*10000/5 = 120000 → clamp 20000.
            assert_eq!(
                EmissionController::compute_time_adjusted_difficulty(8_000, 10_000, 5),
                16_000
            );
        }

        /// Clamp bounds for the time-based kernel: multiplier ∈ [0.5x, 2.0x],
        /// result ∈ [MIN_DIFFICULTY, MAX_DIFFICULTY].
        #[test]
        fn test_compute_time_adjusted_difficulty_clamps() {
            // Absolute floor: tiny difficulty, fast blocks → clamp to MIN.
            assert_eq!(
                EmissionController::compute_time_adjusted_difficulty(1, 0, 5),
                MIN_DIFFICULTY
            );

            // Absolute ceiling: at MAX, slow blocks → stays at MAX.
            assert_eq!(
                EmissionController::compute_time_adjusted_difficulty(MAX_DIFFICULTY, 1_000, 5),
                MAX_DIFFICULTY
            );

            // Degenerate target leaves difficulty unchanged (re-clamped).
            assert_eq!(
                EmissionController::compute_time_adjusted_difficulty(1234, 10, 0),
                1234
            );
        }
    }

    // --- Legacy functions for backward compatibility ---

    /// Legacy: Calculate difficulty adjustment based on block window.
    ///
    /// This is the old block-time-based adjustment. Prefer `EmissionController`
    /// for new code, which uses tx-count-based monetary policy.
    pub fn calculate_new_difficulty(
        current_difficulty: u64,
        window_start_time: u64,
        window_end_time: u64,
        blocks_in_window: u64,
    ) -> u64 {
        if blocks_in_window == 0 || window_end_time <= window_start_time {
            return current_difficulty;
        }

        let actual_time = window_end_time - window_start_time;
        let expected_time = blocks_in_window * TARGET_BLOCK_TIME;

        // Integer (u128 basis-point) math — no f64. Although this legacy helper
        // has no live callers, keeping it float-free prevents it from ever
        // re-introducing the C5 platform-divergence hazard if wired up later.
        // ratio = actual_time / expected_time, clamped to [0.5x, 2.0x].
        let ratio_bps = (actual_time as u128 * RATIO_SCALE / expected_time as u128)
            .clamp(RATIO_SCALE / 2, MAX_ADJUSTMENT_FACTOR_BPS);

        let new_difficulty = current_difficulty as u128 * ratio_bps / RATIO_SCALE;
        let new_difficulty = u64::try_from(new_difficulty).unwrap_or(u64::MAX);
        new_difficulty.clamp(MIN_DIFFICULTY, MAX_DIFFICULTY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_block() {
        // Default genesis is testnet
        let genesis = Block::genesis();
        assert_eq!(genesis.height(), 0);
        assert_eq!(genesis.header.prev_block_hash, TESTNET_GENESIS_MAGIC);
        assert!(genesis.is_genesis());
        assert_eq!(genesis.genesis_network(), Some(Network::Testnet));
    }

    #[test]
    fn test_genesis_blocks_per_network() {
        let testnet_genesis = Block::genesis_for_network(Network::Testnet);
        let mainnet_genesis = Block::genesis_for_network(Network::Mainnet);

        // Both are genesis blocks
        assert!(testnet_genesis.is_genesis());
        assert!(mainnet_genesis.is_genesis());

        // They have different magic bytes
        assert_eq!(
            testnet_genesis.header.prev_block_hash,
            TESTNET_GENESIS_MAGIC
        );
        assert_eq!(
            mainnet_genesis.header.prev_block_hash,
            MAINNET_GENESIS_MAGIC
        );
        assert_ne!(
            testnet_genesis.header.prev_block_hash,
            mainnet_genesis.header.prev_block_hash
        );

        // They produce different hashes
        assert_ne!(testnet_genesis.hash(), mainnet_genesis.hash());

        // Network detection works
        assert_eq!(testnet_genesis.genesis_network(), Some(Network::Testnet));
        assert_eq!(mainnet_genesis.genesis_network(), Some(Network::Mainnet));
    }

    #[test]
    fn test_genesis_magic_bytes_readable() {
        // Verify the magic bytes are human-readable
        let mainnet_str = std::str::from_utf8(&MAINNET_GENESIS_MAGIC[..24]).unwrap();
        let testnet_str = std::str::from_utf8(&TESTNET_GENESIS_MAGIC[..24]).unwrap();

        assert_eq!(mainnet_str, "BOTHO_MAINNET_GENESIS_V1");
        assert_eq!(testnet_str, "BOTHO_TESTNET_GENESIS_V1");
    }

    #[test]
    fn test_block_hash_deterministic() {
        let genesis = Block::genesis();
        let hash1 = genesis.hash();
        let hash2 = genesis.hash();
        assert_eq!(hash1, hash2);
    }

    // --- #599: fee-sum overflow is handled deterministically ---

    /// `total_fees()` must saturate rather than wrap silently (release) or
    /// panic (debug) when attacker-influenced per-tx fees sum past `u64::MAX`.
    /// This test is meaningful in BOTH build modes: in debug the old `.sum()`
    /// panicked here; in release it wrapped to a small value.
    #[test]
    fn test_total_fees_saturates_on_overflow() {
        let mut genesis = Block::genesis();
        // Two fees whose sum exceeds u64::MAX. A wrapping accumulator would
        // yield u64::MAX.wrapping_add(2) == 1; a saturating one yields u64::MAX.
        genesis.transactions = vec![
            Transaction::new_stub_with_fee(u64::MAX),
            Transaction::new_stub_with_fee(3),
        ];

        let total = genesis.total_fees();
        assert_eq!(
            total,
            u64::MAX,
            "fee total must clamp to u64::MAX, not wrap"
        );
        // The "any fees at all?" gate still fires on the saturated total.
        assert!(total > 0);
    }

    /// A non-overflowing fee total is still summed correctly after the switch
    /// to saturating accumulation.
    #[test]
    fn test_total_fees_normal_sum() {
        let mut genesis = Block::genesis();
        genesis.transactions = vec![
            Transaction::new_stub_with_fee(100),
            Transaction::new_stub_with_fee(250),
            Transaction::new_stub_with_fee(650),
        ];
        assert_eq!(genesis.total_fees(), 1000);
    }

    // Note: Block reward calculation uses calculate_block_reward() which is
    // based on block height via MonetaryPolicy (5s block assumption). Tests
    // for the halving schedule are in the monetary.rs and validation.rs
    // test modules.

    // --- #333: supply accumulators are u128 ---

    /// The `u128` tail-reward reimplementation in `calculate_block_reward`
    /// MUST stay bit-for-bit identical to the cluster-tax crate's `u64`
    /// version for any supply that fits in `u64`. This guards against the two
    /// formulas silently drifting apart.
    #[test]
    fn test_tail_reward_u128_matches_u64_in_range() {
        let policy = crate::monetary::mainnet_policy();
        for supply in [
            0u64,
            1,
            1_000_000_000_000,
            1_000_000_000_000_000_000,
            u64::MAX,
        ] {
            assert_eq!(
                calculate_tail_reward_u128(&policy, supply as u128),
                policy.calculate_tail_reward(supply),
                "tail reward mismatch at supply={supply}",
            );
        }
    }

    /// `calculate_block_reward` must compute the correct tail reward when the
    /// real picocredit supply exceeds `u64::MAX` (the regime the chain
    /// actually reaches). Truncating to `u64` here would be a consensus bug.
    #[test]
    fn test_block_reward_correct_past_u64_max() {
        let policy = crate::monetary::mainnet_policy();
        // First height in Phase 2 (tail emission).
        let tail_height = policy.tail_emission_start_height();
        assert!(!policy.is_halving_phase(tail_height));

        // Realistic Phase-2 supply: ~1.22e21 picocredits, far above u64::MAX
        // (~1.84e19). Computing the tail reward from a u64-truncated supply
        // would produce a wildly different (wrong) value.
        let real_supply: u128 = 1_220_000_000_000_000_000_000;
        assert!(real_supply > u64::MAX as u128);

        let reward = calculate_block_reward(tail_height, real_supply);

        // Recompute the expected reward independently in u128.
        let target_net = real_supply * policy.tail_inflation_bps as u128 / 10_000;
        let expected_burns = real_supply * policy.expected_fee_burn_rate_bps as u128 / 10_000;
        let secs_per_year: u128 = 365 * 24 * 3600;
        let blocks_per_year = secs_per_year / policy.target_block_time_secs as u128;
        let expected = ((target_net + expected_burns) / blocks_per_year).max(1) as u64;

        assert_eq!(reward, expected);

        // Sanity: a u64-truncating implementation would have used
        // (real_supply as u64), a completely different supply, yielding a
        // different reward — confirm the two differ so this test has teeth.
        let truncated_reward = policy.calculate_tail_reward(real_supply as u64);
        assert_ne!(
            reward, truncated_reward,
            "u128 path must differ from u64-truncated path at this supply",
        );
    }

    /// #663: with `overflow-checks = true` now on the release profile, the
    /// reward path must stay panic-free at the extreme corner — max height
    /// with a supply far past `u64::MAX`. The tail formula is pure `u128`
    /// arithmetic whose intermediates (supply × bps) stay orders of magnitude
    /// below `u128::MAX` at any reachable supply; this pins that invariant
    /// under checked release builds (`cargo test --release`).
    #[test]
    fn test_block_reward_no_overflow_at_max_height_and_supply() {
        let real_supply_cap: u128 = 1_220_000_000_000_000_000_000; // ~1.22e21 picocredits
        let reward = calculate_block_reward(u64::MAX, real_supply_cap);
        assert!(reward >= 1, "tail reward is floored at 1");
    }

    /// Phase-1 halving reward is height-driven and independent of supply, so
    /// passing a u128 supply above u64::MAX must not change it.
    #[test]
    fn test_halving_reward_ignores_large_supply() {
        let huge_supply: u128 = u64::MAX as u128 + 1_000_000;
        let at_zero = calculate_block_reward(0, 0);
        let at_zero_huge = calculate_block_reward(0, huge_supply);
        assert_eq!(at_zero, at_zero_huge);
        assert_eq!(at_zero, difficulty::INITIAL_REWARD); // 50 BTH at genesis
    }

    /// The cumulative emission accumulator must track exact picocredit totals
    /// past u64::MAX without wrapping. This is the core regression guard for
    /// #333: with `overflow-checks=false` in release, a u64 accumulator would
    /// silently wrap here.
    #[test]
    fn test_emission_controller_accumulates_past_u64_max() {
        use difficulty::EmissionController;
        let mut ctrl = EmissionController::new(1000);

        // Seed just below u64::MAX so the next reward crosses the boundary.
        let near_max = u64::MAX - 10;
        ctrl.total_emitted = near_max as u128;

        let reward = difficulty::INITIAL_REWARD; // 50 BTH = 5e13 pico
        ctrl.record_block(1, reward, 0, None);

        let expected = near_max as u128 + reward as u128;
        assert_eq!(ctrl.total_emitted, expected);
        assert!(
            ctrl.total_emitted > u64::MAX as u128,
            "must have crossed u64::MAX"
        );

        // net_supply also computed in u128 without wrapping.
        assert_eq!(ctrl.net_supply(), expected);
    }

    /// A classical (KEM-less) coinbase must hash byte-for-byte the same as the
    /// pre-6.0.0 layout: the `kem_ciphertext` fold is skipped when `None`, so
    /// existing minting-tx identities and block-hash determinism are preserved.
    #[test]
    fn classical_minting_hash_is_backcompat() {
        let tx = MintingTx {
            block_height: 11,
            reward: 600_000_000_000,
            minter_view_key: [1u8; 32],
            minter_spend_key: [2u8; 32],
            target_key: [3u8; 32],
            public_key: [4u8; 32],
            kem_ciphertext: None,
            prev_block_hash: [5u8; 32],
            difficulty: 1000,
            nonce: 7,
            timestamp: 42,
        };

        // Recompute the legacy digest (classical field order, no ciphertext).
        let mut hasher = Sha256::new();
        hasher.update(tx.block_height.to_le_bytes());
        hasher.update(tx.reward.to_le_bytes());
        hasher.update(tx.minter_view_key);
        hasher.update(tx.minter_spend_key);
        hasher.update(tx.target_key);
        hasher.update(tx.public_key);
        hasher.update(tx.prev_block_hash);
        hasher.update(tx.difficulty.to_le_bytes());
        hasher.update(tx.nonce.to_le_bytes());
        hasher.update(tx.timestamp.to_le_bytes());
        let expected: [u8; 32] = hasher.finalize().into();

        assert_eq!(
            tx.hash(),
            expected,
            "a None ciphertext must hash exactly as the legacy layout"
        );
        // Determinism.
        assert_eq!(tx.hash(), tx.hash());
    }

    /// A classical (KEM-less) lottery payout hashes over the classical fields
    /// only, so its identity matches the pre-6.0.0 layout.
    #[test]
    fn classical_lottery_hash_is_backcompat() {
        let out = LotteryOutput {
            winner_tx_hash: [1u8; 32],
            winner_output_index: 2,
            payout: 5,
            target_key: [3u8; 32],
            public_key: [4u8; 32],
            kem_ciphertext: None,
        };

        let mut hasher = Sha256::new();
        hasher.update(out.winner_tx_hash);
        hasher.update(out.winner_output_index.to_le_bytes());
        hasher.update(out.payout.to_le_bytes());
        hasher.update(out.target_key);
        hasher.update(out.public_key);
        let expected: [u8; 32] = hasher.finalize().into();

        assert_eq!(out.hash(), expected);
    }

    /// The lottery-payout hash binds the ML-KEM ciphertext: attaching or
    /// mutating it changes the payout identity.
    #[test]
    fn lottery_hash_binds_kem_ciphertext() {
        let classical = LotteryOutput {
            winner_tx_hash: [1u8; 32],
            winner_output_index: 2,
            payout: 5,
            target_key: [3u8; 32],
            public_key: [4u8; 32],
            kem_ciphertext: None,
        };
        let h_classical = classical.hash();

        let mut hybrid = classical.clone();
        hybrid.kem_ciphertext = Some(vec![0x11u8; 1088]);
        let h_hybrid = hybrid.hash();
        assert_ne!(
            h_classical, h_hybrid,
            "attaching a ciphertext must change the payout hash"
        );

        let mut hybrid_mutated = hybrid.clone();
        let mut ct = hybrid_mutated.kem_ciphertext.take().unwrap();
        ct[0] ^= 0xFF;
        hybrid_mutated.kem_ciphertext = Some(ct);
        assert_ne!(
            h_hybrid,
            hybrid_mutated.hash(),
            "mutating the ciphertext must change the payout hash"
        );
    }

    #[cfg(feature = "pq")]
    mod pq_kem_tests {
        use super::*;
        use bth_account_keys::AccountKey;
        use bth_crypto_keys::RistrettoPublic;
        use bth_crypto_pq::{MlKem768KeyPair, ML_KEM_768_CIPHERTEXT_BYTES};
        use rand_chacha::ChaCha20Rng;
        use rand_core::SeedableRng;

        /// A deterministic minter/winner: a classical account plus an ML-KEM
        /// keypair, and a `PublicAddress` that PUBLISHES that KEM key (address
        /// format v2). The holder can both receive to `addr` and scan/spend
        /// with `(account, kem)`.
        fn minter(seed: u8) -> (AccountKey, MlKem768KeyPair, PublicAddress) {
            let mut rng = ChaCha20Rng::from_seed([seed; 32]);
            let account = AccountKey::random(&mut rng);
            let kem = MlKem768KeyPair::from_seed(&[seed ^ 0x5A; 32]);
            let addr = account
                .default_subaddress()
                .with_pq_keys(kem.public_key().as_bytes().to_vec(), Vec::new());
            (account, kem, addr)
        }

        /// The minting reward output carries a valid 1,088-byte ML-KEM
        /// ciphertext encapsulated to the minter's own published KEM key, and
        /// the minter recovers (scans + spends) its reward.
        #[test]
        fn minting_reward_carries_ciphertext_and_minter_recovers() {
            let (account, kem, addr) = minter(1);
            let tx = MintingTx::new(11, 600_000_000_000, &addr, [7u8; 32], 1000, 42);

            let ct = tx
                .kem_ciphertext
                .as_ref()
                .expect("coinbase must carry an ML-KEM ciphertext");
            assert_eq!(
                ct.len(),
                ML_KEM_768_CIPHERTEXT_BYTES,
                "ciphertext must be exactly 1,088 bytes"
            );

            // to_tx_output propagates the ciphertext unchanged.
            let out = tx.to_tx_output();
            assert_eq!(out.kem_ciphertext.as_deref(), Some(ct.as_slice()));

            // The minter scans its reward at MINTING_OUTPUT_INDEX and recovers
            // the one-time spend key (belongs_to_hybrid + recover).
            let idx = out
                .belongs_to_hybrid(&account, &kem, MINTING_OUTPUT_INDEX)
                .expect("minter must detect its own reward");
            let spend = out
                .recover_spend_key_hybrid(&account, &kem, idx, MINTING_OUTPUT_INDEX)
                .expect("minter must recover the one-time spend key");
            assert_eq!(
                RistrettoPublic::from(&spend).to_bytes(),
                out.target_key,
                "onetime_private_key * G must equal target_key"
            );
        }

        /// `coinbase_stealth_fields` encapsulates to a published KEM key, and
        /// falls back to a classical (KEM-less) output for an address that
        /// publishes no KEM key.
        #[test]
        fn coinbase_stealth_fields_encapsulates_or_falls_back() {
            let (_a, _k, addr) = minter(9);
            let (_tk, _pk, ct) = MintingTx::coinbase_stealth_fields(&addr);
            assert_eq!(
                ct.as_ref().map(|c| c.len()),
                Some(ML_KEM_768_CIPHERTEXT_BYTES)
            );

            // A KEM-less address (empty published key) falls back to classical.
            let classical_addr = PublicAddress::from_random(&mut OsRng);
            let (_tk2, _pk2, ct2) = MintingTx::coinbase_stealth_fields(&classical_addr);
            assert!(
                ct2.is_none(),
                "an address with no published KEM key must fall back to classical"
            );
        }

        /// A lottery payout reuses the winning UTXO's hybrid envelope
        /// (target/public keys AND ciphertext), and the winner — who holds the
        /// KEM secret — decapsulates and recovers the payout.
        #[test]
        fn lottery_payout_reuses_envelope_and_winner_recovers() {
            let (account, kem, addr) = minter(2);
            let output_index = 3u32;

            // The winning UTXO: a hybrid output the winner already owns.
            let winning = TxOutput::new_hybrid_to_address(
                50_000,
                &addr,
                output_index,
                None,
                ClusterTagVector::empty(),
            )
            .expect("winning UTXO is a hybrid output");

            // Build the payout reusing the winner's stealth envelope. The UTXO
            // id embeds the original output index in its trailing 4 bytes.
            let mut utxo_id = [0u8; 36];
            utxo_id[..32].copy_from_slice(&[0xABu8; 32]);
            utxo_id[32..].copy_from_slice(&output_index.to_le_bytes());
            let payout = LotteryOutput::from_utxo_id(
                utxo_id,
                900,
                winning.target_key,
                winning.public_key,
                winning.kem_ciphertext.clone(),
            );

            let ct = payout
                .kem_ciphertext
                .as_ref()
                .expect("payout must carry a ciphertext");
            assert_eq!(ct.len(), ML_KEM_768_CIPHERTEXT_BYTES);
            assert_eq!(
                payout.kem_ciphertext, winning.kem_ciphertext,
                "payout must reuse the winning UTXO's ciphertext verbatim"
            );
            assert_eq!(payout.winner_output_index, output_index);

            // The winner detects + recovers the payout with the same hybrid
            // derivation (same shared secret, same output index).
            let out = payout.to_tx_output(ClusterTagVector::empty());
            let idx = out
                .belongs_to_hybrid(&account, &kem, output_index)
                .expect("winner must detect the lottery payout");
            let spend = out
                .recover_spend_key_hybrid(&account, &kem, idx, output_index)
                .expect("winner must recover the payout spend key");
            assert_eq!(RistrettoPublic::from(&spend).to_bytes(), out.target_key);
        }

        /// `MintingTx::hash()` binds the ML-KEM ciphertext: mutating or
        /// dropping it changes the coinbase identity, while the
        /// PoW/header link (which does not use this hash) is
        /// unaffected.
        #[test]
        fn minting_hash_binds_kem_ciphertext_and_pow_link_unaffected() {
            let (_a, _k, addr) = minter(4);
            let tx = MintingTx::new(11, 600_000_000_000, &addr, [7u8; 32], 1000, 42);
            let base = tx.hash();
            assert_eq!(base, tx.hash(), "hash must be deterministic");

            // Mutating the ciphertext changes the hash.
            let mut mutated = tx.clone();
            let mut ct = mutated.kem_ciphertext.take().unwrap();
            ct[0] ^= 0xFF;
            mutated.kem_ciphertext = Some(ct);
            assert_ne!(base, mutated.hash(), "hash must bind the ciphertext");

            // Dropping the ciphertext changes the hash.
            let mut dropped = tx.clone();
            dropped.kem_ciphertext = None;
            assert_ne!(base, dropped.hash());

            // The PoW hash (header↔minting-tx link) does NOT depend on the
            // ciphertext, so folding it in cannot desync header and minting tx.
            assert_eq!(
                tx.pow_hash(),
                dropped.pow_hash(),
                "kem_ciphertext must not affect the PoW/header link"
            );
        }
    }
}
