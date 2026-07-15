use bth_account_keys::{AccountKey, PublicAddress};
use bth_cluster_tax::{LotteryCandidate, LotteryDrawConfig, TagVector};
use bth_transaction_types::{ClusterTagVector, Network, TAG_WEIGHT_SCALE};
use heed::{
    types::{Bytes, U64},
    Database, Env, EnvOpenOptions, RwTxn,
};
use rand::Rng;
use std::{collections::BTreeMap, fs, path::Path};
use tracing::{debug, info, warn};

use super::{ChainState, LedgerError};
use crate::{
    block::{calculate_block_reward, Block},
    consensus::{validate_block_lottery, LotteryFeeConfig, MAX_TX_AGE},
    decoy_selection::{DecoySelectionError, GammaDecoySelector, OutputCandidate},
    transaction::{RingMember, Transaction as BothoTransaction, TxOutput, Utxo, UtxoId},
};

/// Maximum allowed clock skew for a block timestamp relative to local time.
///
/// Matches the bound enforced on minting transactions in the SCP proposal
/// path (`consensus::validation::MAX_FUTURE_TIMESTAMP_SECS`).
const MAX_FUTURE_TIMESTAMP_SECS: u64 = 2 * 60 * 60;

/// Deterministic height-based staleness backstop (issue #451).
///
/// Returns the index of the first transfer tx in `block` that is too old
/// relative to the block's OWN height (`created_at_height + MAX_TX_AGE <
/// block.height()`), or `None` if all transfer txs are fresh enough.
///
/// This mirrors the staleness rule that the SCP transfer-validity gate used to
/// enforce against the local *current tip* (removed in #451 because tip-
/// dependence is the #417-class fork condition). Evaluated against the block's
/// own height — identical on every honest node applying block N — it is
/// deterministic and cannot diverge across nodes. Honest block-builders filter
/// these out at build time, so this is a defense-in-depth check that never
/// fires for blocks we build (no externalize-then-reject halt).
fn first_stale_transfer_tx(block: &Block) -> Option<usize> {
    let block_height = block.height();
    // `created_at_height` is attacker-influenced (it arrives in a gossiped
    // block, before signature validation), so the age bound must saturate:
    // with `overflow-checks = true` on the release profile (#663) an unchecked
    // `+` here would let a crafted `created_at_height` near `u64::MAX` panic
    // every validating node. Saturation preserves the rule's semantics — a
    // saturated sum is `u64::MAX`, which is never `< block_height`, so such a
    // tx is simply "not stale" here and is rejected by the later gates (C3
    // ring resolution) instead.
    block
        .transactions
        .iter()
        .position(|tx| tx.created_at_height.saturating_add(MAX_TX_AGE) < block_height)
}

/// Consensus decay rate for the cluster-tag inflation guard (issue #576).
///
/// Fixed at 0 (no decay credit) on purpose. The per-cluster bound this yields
/// — output mass <= input mass — is the *most permissive* conservation-of-mass
/// bound, so it never false-rejects an honestly built block regardless of how
/// much decay that block's wallet actually applied. Decay credit legitimately
/// ranges from 0 (no entropy proof / proof-required height) up to the base
/// rate depending on the entropy proof, so any nonzero rate here could reject a
/// valid zero-decay-credit block and halt consensus. The guard's sole job is to
/// reject *inflation* — outputs claiming more cluster mass than the inputs can
/// supply — which `decay_rate = 0` captures exactly.
const CONSENSUS_TAG_DECAY_RATE: u32 = 0;

/// Cluster-tag inflation guard (issue #576, decomposition item H2-B3;
/// input-bound tightened in issue #581).
///
/// Ports the integer/`BTreeMap` conservation-of-mass logic of
/// `bth_transaction_core::validation::validate_cluster_tag_inheritance` to the
/// node's `(ClusterTagVector, value)` representation so it can run at block
/// validation time. For each cluster present on the inputs, the summed output
/// tag *mass* (`Σ value · weight / TAG_WEIGHT_SCALE`) may not exceed the
/// (decayed) input mass plus a small rounding tolerance; otherwise the block is
/// rejected. New clusters that appear only on the outputs are permitted,
/// exactly as in the upstream validator (they may arise from background
/// attribution).
///
/// # Input bound: per-ring maximum, not global sum (issue #581)
///
/// `input_rings` is the set of resolved `(tags, value)` members grouped **by
/// ring** — one inner slice per transaction input. Exactly one member of each
/// ring is the real spent input; the rest are decoys. The node cannot tell
/// which, so it needs a node-agnostic upper bound on the real input's
/// per-cluster mass.
///
/// The bound used is, per cluster `c`,
/// `Σ over rings ( max over that ring's members of member.mass(c) )`.
/// Since the real input is one member of its ring, its cluster-`c` mass is
/// `≤` that ring's maximum, so the sum-of-per-ring-maxima is a valid upper
/// bound and the guard **never false-rejects a valid block** (liveness-safe).
/// It is also the *tightest* sound node-agnostic bound: a ring's contribution
/// cannot be lowered below its maximum without risking a false-reject in the
/// case where the real input *is* the maximum member.
///
/// This replaces the original bound (`Σ over ALL ring members`), which let an
/// attacker sourcing real cluster-`c`-tagged UTXOs as decoys inflate cluster
/// `c`'s ceiling to roughly `ring_size ×` the decoy mass and thereby attribute
/// more cluster mass to their output than their real input supplied. The
/// per-ring maximum removes the `ring_size` multiplier entirely: filling a
/// ring with `R` cluster-`c` decoys of mass `M` now raises the ceiling to `M`,
/// not `R·M`. The residual (a single decoy whose cluster-`c` mass exceeds the
/// real input's) is bounded by one UTXO's mass and can only be closed by a
/// consensus-visible binding to the real input (Phase-2 Pedersen-committed
/// tags with ZK inheritance proofs; see `cluster_tags.rs`).
///
/// Determinism (consensus fork safety): this is a pure function of the
/// transaction plus committed UTXO state. All accumulation uses `BTreeMap`
/// (sorted, deterministic iteration order) and integer arithmetic in `u128`
/// (overflow-safe vs. the upstream `u64` accumulation, since summed ring-member
/// mass can exceed `u64::MAX`). No `HashMap`/`HashSet` iteration order, no
/// `f64`, and no node-local state enter the computation, so a proposer and a
/// validator applying the same block reach an identical verdict.
fn check_cluster_tag_inheritance(
    input_rings: &[Vec<(ClusterTagVector, u64)>],
    outputs: &[TxOutput],
) -> Result<(), LedgerError> {
    fn accumulate<'a>(
        items: impl Iterator<Item = (u64, &'a ClusterTagVector)>,
    ) -> BTreeMap<u64, u128> {
        let mut masses: BTreeMap<u64, u128> = BTreeMap::new();
        for (value, tags) in items {
            for entry in &tags.entries {
                let mass = (value as u128) * (entry.weight as u128) / (TAG_WEIGHT_SCALE as u128);
                *masses.entry(entry.cluster_id.0).or_insert(0) += mass;
            }
        }
        masses
    }

    // Input bound (#581): sum over rings of the per-ring, per-cluster MAXIMUM
    // member mass. The real input is one member of its ring, so its cluster
    // mass is at most the ring's maximum for that cluster — a sound upper
    // bound that does not carry the old `ring_size` inflation multiplier.
    //
    // The maximum is taken over MEMBERS, and a single member's cluster mass is
    // the SUM of that member's entries for the cluster. Computing the
    // per-member mass first (rather than folding the max over raw entries)
    // keeps the bound correct — and never too low, which would false-reject a
    // valid block — even if a member's tag vector were to carry duplicate
    // cluster-id entries. We do not depend on the tag-vector uniqueness
    // invariant here: this is a consensus liveness property.
    let mut input_masses: BTreeMap<u64, u128> = BTreeMap::new();
    for ring in input_rings {
        let mut ring_max: BTreeMap<u64, u128> = BTreeMap::new();
        for (tags, value) in ring {
            // This member's total mass per cluster (sum of its own entries).
            let mut member_mass: BTreeMap<u64, u128> = BTreeMap::new();
            for entry in &tags.entries {
                let mass = (*value as u128) * (entry.weight as u128) / (TAG_WEIGHT_SCALE as u128);
                *member_mass.entry(entry.cluster_id.0).or_insert(0) += mass;
            }
            // Fold this member into the ring's per-cluster maximum.
            for (cluster, mass) in member_mass {
                let slot = ring_max.entry(cluster).or_insert(0);
                *slot = (*slot).max(mass);
            }
        }
        for (cluster, mass) in ring_max {
            *input_masses.entry(cluster).or_insert(0) += mass;
        }
    }

    let output_masses = accumulate(outputs.iter().map(|o| (o.amount, &o.cluster_tags)));

    let decay_factor = TAG_WEIGHT_SCALE.saturating_sub(CONSENSUS_TAG_DECAY_RATE) as u128;
    let scale = TAG_WEIGHT_SCALE as u128;

    for (cluster, &input_mass) in &input_masses {
        let expected = input_mass * decay_factor / scale;
        let actual = output_masses.get(cluster).copied().unwrap_or(0);
        // Allow some tolerance for per-entry integer rounding (mirrors upstream).
        let tolerance = (input_mass / 1000).max(1);

        if actual > expected + tolerance {
            return Err(LedgerError::InvalidBlock(format!(
                "cluster {} tag inflation: output mass {} exceeds expected {} (input mass {}, tolerance {})",
                cluster, actual, expected, input_mass, tolerance
            )));
        }
    }

    Ok(())
}

/// The bridge **wrap-eligibility** predicate (issues #831 / #822 / #824).
///
/// The wrap on-ramp (the bridge chain watcher, #824) admits a deposit output
/// for wrapping to wBTH ONLY if it was produced by an accepted
/// demurrage-settlement transaction AND carries a background/factor-1 tag. Both
/// conditions are necessary:
///
/// - **`producing_tx.is_settlement()`** — the transaction that created the
///   output was an explicit settlement, which block acceptance only admits
///   after the capitalized settlement charge was paid (C7 fee floor) and the
///   tag- rewrite rule held (C8, [`Ledger::verify_settlement`]). Binding to the
///   FLAG, not merely to background tags, is what closes the cheap-escape: a
///   normal spend-to-background is trivially background-tagged but is **not**
///   wrap-eligible, so a wealthy holder cannot bypass the capitalized charge by
///   an ordinary spend.
/// - **`output.cluster_tags.is_empty()`** — the output itself reads as
///   factor-1/background, so a settled-then-wrapped coin is peg-neutral (#825):
///   it carries no residual cluster provenance into the bridge reserve.
///
/// Pure comparison — no ledger read, no node-local state — so every node (and
/// the bridge) evaluates it identically.
pub fn wrap_eligible(output: &TxOutput, producing_tx: &BothoTransaction) -> bool {
    producing_tx.is_settlement() && output.cluster_tags.is_empty()
}

/// Congestion-free deterministic fee base for the consensus fee floor
/// (issue #578, design #574 item H1-B4).
///
/// The mempool floor multiplies the size fee by a *dynamic* base
/// (`DynamicFeeBase::compute_base`) that reflects local congestion. That base
/// carries an f64 EMA that is node-local and reset on restart
/// (`dynamic_fee.rs:79-80`), so it MUST NOT enter consensus (a node with a hot
/// vs. cold congestion EMA would otherwise accept a different set of blocks —
/// audit cycle 6 M1). This constant is the neutral, no-congestion value:
/// `DynamicFeeBase::default().base_min` (the fee-per-byte floor the dynamic
/// controller returns whenever the network is uncongested — i.e. not at
/// minimum block time, or below the target fullness; see
/// `DynamicFeeBase::compute_base`). Using it makes the consensus floor a
/// congestion-independent lower bound; the mempool's dynamic base can only ever
/// raise a node's local admission threshold ABOVE this floor, never below it
/// (Bitcoin's min-relay-fee vs. consensus-validity split, design #574 Q1/Q2).
const CONSENSUS_FEE_BASE: u64 = 1;

/// Quantile (in basis points) of the ring members' ages used by the consensus
/// demurrage clock. `10_000` == the maximum age (issue #578, design #574 item
/// H2/B1; empirical sweep #577/#595 selected `@max`). The max is the one order
/// statistic guaranteed to surface a lone old real input regardless of how many
/// fresh decoys pad the ring — p75/p90 miss it because a single old input in an
/// 11-ring needs `q > (n-1)/n = 90.9%`. See
/// `bth_cluster_tax::ring_elapsed_quantile`.
const CONSENSUS_RING_AGE_QUANTILE_BPS: u32 = 10_000;

/// Emission-controller state to persist atomically alongside a block.
///
/// The difficulty/reward/epoch counters are a pure function of the applied
/// block, so they can be written inside the *same* LMDB write transaction as
/// the block itself. Folding them into one txn (via
/// [`Ledger::add_block_with_emission`]) closes the crash-atomicity gap (audit
/// cycle-6 **H3**, issue #558): a crash between two separate commits would
/// otherwise advance the chain height while leaving the difficulty/epoch state
/// stale, causing the node to compute a hard-validated difficulty that diverges
/// permanently from its peers.
///
/// Field semantics mirror [`Ledger::update_emission_state`] exactly — this is
/// only a *when* change (one commit instead of two), never a *what* change to
/// the persisted values.
#[derive(Debug, Clone, Copy)]
pub struct EmissionStateUpdate {
    /// New PoW difficulty (chain state; hard-validated on the next block).
    pub difficulty: u64,
    /// Cumulative transaction count.
    pub total_tx: u64,
    /// Transactions accumulated in the current adjustment epoch.
    pub epoch_tx: u64,
    /// Emission accumulated in the current adjustment epoch.
    pub epoch_emission: u64,
    /// Burns accumulated in the current adjustment epoch.
    pub epoch_burns: u64,
    /// Current block reward (informational).
    pub current_reward: u64,
}

/// LMDB-backed ledger storage using heed
pub struct Ledger {
    env: Env,
    /// The network this ledger belongs to
    network: Network,
    /// blocks: height (u64) -> Block (bytes)
    blocks_db: Database<U64<heed::byteorder::LE>, Bytes>,
    /// metadata: key (bytes) -> value (bytes)
    meta_db: Database<Bytes, Bytes>,
    /// utxos: UtxoId (36 bytes) -> Utxo (bytes)
    utxo_db: Database<Bytes, Bytes>,
    /// address_index: target_key (32 bytes) -> [UtxoId (36 bytes), ...]
    /// Maps target keys to their UTXOs for efficient lookups
    address_index_db: Database<Bytes, Bytes>,
    /// key_images: key_image (32 bytes) -> height (8 bytes)
    /// Tracks spent key images to prevent double-spending with ring signatures.
    key_images_db: Database<Bytes, Bytes>,
    /// tx_index: tx_hash (32 bytes) -> TxLocation (12 bytes: height u64 +
    /// tx_index u32) Maps transaction hashes to their location for fast
    /// lookups (exchange integration).
    tx_index_db: Database<Bytes, Bytes>,
    /// cluster_wealth: cluster_id (8 bytes) -> wealth (16 bytes, u128 LE)
    /// Tracks total value per cluster tag across all UTXOs for progressive fee
    /// calculation. Note: This is an approximation - with ring signatures,
    /// we cannot know which UTXO was actually spent, so spent UTXOs still
    /// contribute to cluster wealth until eventually removed by UTXO
    /// pruning (if implemented).
    ///
    /// PROTOCOL 4.0.0 (#626): the value widened `u64` -> `u128` (16-byte LE) so
    /// cumulative tagged wealth can exceed the former `u64::MAX` pico ceiling
    /// (18.44M BTH), which the log-domain factor curve now maps across the full
    /// supply range. See [`decode_cluster_wealth`] for the reject-legacy read
    /// contract.
    cluster_wealth_db: Database<Bytes, Bytes>,
    /// bridge_import_clusters: cluster_id (8 bytes) -> [] (presence-only set)
    ///
    /// PROTOCOL 5.0.0 (ADR 0007, #938): the set of cluster ids that are
    /// bridge-import clusters `c_import(m) = H("bridge-import" ‖ m)`. An id is
    /// recorded when a block whose height falls in epoch `m` creates an output
    /// tagged to `import_cluster_id(m)` — i.e. an unwrap's minted output (the
    /// bridge tags it; see `bridge/service/src/bth_scan.rs`). Membership is
    /// what makes the ≥F import floor enforceable at spend time: a coin
    /// whose tag references a recorded import cluster is priced at
    /// `max(curve, F)` on that tagged fraction, whether it is a fresh
    /// unwrap output or a forwarded import tag on a later spend. Recording
    /// is a pure function of block height
    /// + output tag (the id must equal this-epoch's `import_cluster_id`), so
    /// every node builds the identical set — no fork, and the rebuild path
    /// reconstructs it byte-identically from the block store.
    bridge_import_clusters_db: Database<Bytes, Bytes>,
}

/// Serialized width of a `cluster_wealth_db` value: 16-byte little-endian
/// `u128` (protocol 4.0.0, #626).
const CLUSTER_WEALTH_LEN: usize = 16;

/// Decode a `cluster_wealth_db` value, enforcing the 16-byte LE `u128`
/// serialization contract (#626 determinism section).
///
/// Fresh genesis is ASSUMED: a legacy 8-byte (`u64`) value written by a
/// pre-4.0.0 node is REJECTED with a hard error, never silently reinterpreted
/// as a truncated/zero-padded `u128`. This fails CLOSED (consistent with the
/// M7 discipline): a wrong-width value surfaces as a `LedgerError` rather than
/// a bogus wealth that would mis-price the progressive fee / lottery tilt.
/// There is no in-place migration because the widening rides the 4.0.0 testnet
/// reset.
#[inline]
fn decode_cluster_wealth(bytes: &[u8]) -> Result<u128, LedgerError> {
    if bytes.len() != CLUSTER_WEALTH_LEN {
        return Err(LedgerError::Database(format!(
            "cluster_wealth value has {} bytes, expected {} (16-byte LE u128); \
             legacy 8-byte values are rejected — fresh genesis (protocol 4.0.0) assumed",
            bytes.len(),
            CLUSTER_WEALTH_LEN
        )));
    }
    let mut arr = [0u8; CLUSTER_WEALTH_LEN];
    arr.copy_from_slice(bytes);
    Ok(u128::from_le_bytes(arr))
}

// Metadata keys
const META_HEIGHT: &[u8] = b"height";
const META_TIP_HASH: &[u8] = b"tip_hash";

/// Location of a transaction in the blockchain.
/// Used for fast transaction lookups (exchange integration).
#[derive(Debug, Clone, Copy)]
pub struct TxLocation {
    /// Block height containing the transaction
    pub block_height: u64,
    /// Index of the transaction within the block
    pub tx_index: u32,
}
const META_TOTAL_MINED: &[u8] = b"total_mined";
const META_FEES_BURNED: &[u8] = b"fees_burned";
const META_DIFFICULTY: &[u8] = b"difficulty";

// EmissionController state
const META_TOTAL_TX: &[u8] = b"total_tx";
const META_EPOCH_TX: &[u8] = b"epoch_tx";
const META_EPOCH_EMISSION: &[u8] = b"epoch_emission";
const META_EPOCH_BURNS: &[u8] = b"epoch_burns";
const META_CURRENT_REWARD: &[u8] = b"current_reward";
// Redistribution lottery carryover pool (consensus state)
const META_LOTTERY_POOL: &[u8] = b"lottery_pool";

/// Sum a block's per-transaction fees, rejecting on `u64` overflow.
///
/// Per-tx fees are attacker-influenced, so a naive `.sum::<u64>()` would wrap
/// silently under `overflow-checks = false` (release) or panic (debug) for a
/// crafted block whose fees total past `u64::MAX`. We accumulate with
/// `checked_add` and return `LedgerError::FeeOverflow` instead, matching the
/// per-tx balance overflow guard from issue #340. See issue #599.
fn checked_block_fees(block: &Block) -> Result<u64, LedgerError> {
    block
        .transactions
        .iter()
        .try_fold(0u64, |acc, tx| acc.checked_add(tx.fee))
        .ok_or(LedgerError::FeeOverflow)
}

impl Ledger {
    /// Open or create a ledger at the given path (defaults to Testnet for
    /// backward compatibility)
    pub fn open(path: &Path) -> Result<Self, LedgerError> {
        Self::open_for_network(path, Network::Testnet)
    }

    /// Test-only: open a ledger whose LMDB reader table holds exactly one slot.
    /// Holding a single live `RoTxn` then exhausts the table, so the next
    /// `read_txn()` (e.g. inside `is_key_image_spent`) fails with
    /// `MdbError::ReadersFull`. Used by the M7 fail-closed tests to inject a DB
    /// error on the consensus double-spend check.
    #[cfg(test)]
    pub fn open_single_reader(path: &Path) -> Result<Self, LedgerError> {
        Self::open_internal(path, Network::Testnet, Some(1))
    }

    /// Test-only: open a read transaction against the private LMDB environment.
    /// Combined with `open_single_reader`, holding the returned `RoTxn`
    /// exhausts the reader table so subsequent lookups fail closed (used by
    /// M7 tests in sibling modules such as the mempool).
    #[cfg(test)]
    pub fn read_txn_for_test(&self) -> Result<heed::RoTxn<'_>, LedgerError> {
        self.env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))
    }

    /// Test-only: set the total wealth attributed to a cluster directly,
    /// bypassing the per-output accumulation path. Used to seed cluster wealth
    /// for the ring-centroid factor-floor tests.
    #[cfg(test)]
    pub fn set_cluster_wealth_for_test(
        &self,
        cluster_id: u64,
        wealth: u128,
    ) -> Result<(), LedgerError> {
        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;
        let cluster_key = cluster_id.to_le_bytes();
        self.cluster_wealth_db
            .put(&mut wtxn, cluster_key.as_slice(), &wealth.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to set cluster wealth: {}", e)))?;
        wtxn.commit().map_err(|e| {
            LedgerError::Database(format!("Failed to commit cluster wealth: {}", e))
        })?;
        Ok(())
    }

    /// Test-only: record a cluster id in the bridge-import set directly (ADR
    /// 0007, #938), bypassing the block-apply recording path. Used by the
    /// consensus import-floor tests to seed an import cluster without minting a
    /// full block at the matching epoch height.
    #[cfg(test)]
    pub fn record_bridge_import_cluster_for_test(
        &self,
        cluster_id: u64,
    ) -> Result<(), LedgerError> {
        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;
        self.bridge_import_clusters_db
            .put(&mut wtxn, cluster_id.to_le_bytes().as_slice(), &[])
            .map_err(|e| {
                LedgerError::Database(format!("Failed to record bridge-import cluster: {}", e))
            })?;
        wtxn.commit().map_err(|e| {
            LedgerError::Database(format!("Failed to commit bridge-import cluster: {}", e))
        })?;
        Ok(())
    }

    /// Open or create a ledger at the given path for a specific network.
    ///
    /// The ledger will be initialized with the appropriate genesis block
    /// for the specified network if it's empty.
    pub fn open_for_network(path: &Path, network: Network) -> Result<Self, LedgerError> {
        Self::open_internal(path, network, None)
    }

    /// Internal constructor shared by `open_for_network` and test helpers.
    ///
    /// `max_readers`, when `Some`, caps the LMDB reader-slot table. Production
    /// callers pass `None` (LMDB's default of 126). Tests pass a small value so
    /// they can deterministically exhaust the reader table and force read
    /// transactions to return `MdbError::ReadersFull` — the only practical way
    /// to inject a DB failure for the M7 fail-closed tests.
    fn open_internal(
        path: &Path,
        network: Network,
        max_readers: Option<u32>,
    ) -> Result<Self, LedgerError> {
        // Create directory if needed
        fs::create_dir_all(path)
            .map_err(|e| LedgerError::Database(format!("Failed to create directory: {}", e)))?;

        // SAFETY: LMDB environment opening is marked unsafe in heed because:
        // 1. The same LMDB environment must not be opened multiple times concurrently
        // 2. The path must exist and be accessible
        // 3. The environment must not outlive the filesystem path
        // We satisfy these by: only opening once per LedgerStore, creating the
        // directory first, and storing the Env in the struct which owns it for
        // its lifetime.
        let env = unsafe {
            let mut opts = EnvOpenOptions::new();
            opts.max_dbs(8) // +bridge_import_clusters_db (ADR 0007, #938)
                .map_size(1024 * 1024 * 1024); // 1GB
            if let Some(readers) = max_readers {
                opts.max_readers(readers);
            }
            opts.open(path)
        }
        .map_err(|e| LedgerError::Database(format!("Failed to open environment: {}", e)))?;

        // Create/open databases
        let mut wtxn = env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        let blocks_db = env
            .create_database(&mut wtxn, Some("blocks"))
            .map_err(|e| LedgerError::Database(format!("Failed to create blocks db: {}", e)))?;
        let meta_db = env
            .create_database(&mut wtxn, Some("meta"))
            .map_err(|e| LedgerError::Database(format!("Failed to create meta db: {}", e)))?;
        let utxo_db = env
            .create_database(&mut wtxn, Some("utxos"))
            .map_err(|e| LedgerError::Database(format!("Failed to create utxos db: {}", e)))?;
        let address_index_db = env
            .create_database(&mut wtxn, Some("address_index"))
            .map_err(|e| {
                LedgerError::Database(format!("Failed to create address_index db: {}", e))
            })?;
        let key_images_db = env
            .create_database(&mut wtxn, Some("key_images"))
            .map_err(|e| LedgerError::Database(format!("Failed to create key_images db: {}", e)))?;
        let tx_index_db = env
            .create_database(&mut wtxn, Some("tx_index"))
            .map_err(|e| LedgerError::Database(format!("Failed to create tx_index db: {}", e)))?;
        let cluster_wealth_db = env
            .create_database(&mut wtxn, Some("cluster_wealth"))
            .map_err(|e| {
                LedgerError::Database(format!("Failed to create cluster_wealth db: {}", e))
            })?;
        let bridge_import_clusters_db = env
            .create_database(&mut wtxn, Some("bridge_import_clusters"))
            .map_err(|e| {
                LedgerError::Database(format!("Failed to create bridge_import_clusters db: {}", e))
            })?;

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;

        let ledger = Self {
            env,
            network,
            blocks_db,
            meta_db,
            utxo_db,
            address_index_db,
            key_images_db,
            tx_index_db,
            cluster_wealth_db,
            bridge_import_clusters_db,
        };

        // Initialize with genesis if empty
        if ledger.get_chain_state()?.height == 0 {
            let state = ledger.get_chain_state()?;
            if state.tip_hash == [0u8; 32] {
                info!(network = %network, "Initializing ledger with genesis block");
                ledger.init_genesis()?;
            }
        }

        Ok(ledger)
    }

    /// Get the network this ledger belongs to.
    pub fn network(&self) -> Network {
        self.network
    }

    /// Initialize the ledger with the genesis block for this network.
    fn init_genesis(&self) -> Result<(), LedgerError> {
        let genesis = Block::genesis_for_network(self.network);
        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        // Store genesis block
        let block_bytes =
            bincode::serialize(&genesis).map_err(|e| LedgerError::Serialization(e.to_string()))?;
        self.blocks_db
            .put(&mut wtxn, &0u64, &block_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to put block: {}", e)))?;

        // Initialize metadata
        let genesis_hash = genesis.hash();
        self.meta_db
            .put(&mut wtxn, META_HEIGHT, &0u64.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put height: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_TIP_HASH, &genesis_hash)
            .map_err(|e| LedgerError::Database(format!("Failed to put tip_hash: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_TOTAL_MINED, &0u64.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put total_mined: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_FEES_BURNED, &0u64.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put fees_burned: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_DIFFICULTY,
                &crate::node::minter::INITIAL_DIFFICULTY.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put difficulty: {}", e)))?;

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;
        Ok(())
    }

    /// Get the current chain state
    pub fn get_chain_state(&self) -> Result<ChainState, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let height = self
            .meta_db
            .get(&rtxn, META_HEIGHT)
            .map_err(|e| LedgerError::Database(format!("Failed to get height: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let tip_hash = self
            .meta_db
            .get(&rtxn, META_TIP_HASH)
            .map_err(|e| LedgerError::Database(format!("Failed to get tip_hash: {}", e)))?
            .map(|b| b.try_into().unwrap_or([0u8; 32]))
            .unwrap_or([0u8; 32]);

        let total_mined = self
            .meta_db
            .get(&rtxn, META_TOTAL_MINED)
            .map_err(|e| LedgerError::Database(format!("Failed to get total_mined: {}", e)))?
            .map(|b| u128::from_le_bytes(b.try_into().unwrap_or([0; 16])))
            .unwrap_or(0);

        let total_fees_burned = self
            .meta_db
            .get(&rtxn, META_FEES_BURNED)
            .map_err(|e| LedgerError::Database(format!("Failed to get fees_burned: {}", e)))?
            .map(|b| u128::from_le_bytes(b.try_into().unwrap_or([0; 16])))
            .unwrap_or(0);

        let difficulty = self
            .meta_db
            .get(&rtxn, META_DIFFICULTY)
            .map_err(|e| LedgerError::Database(format!("Failed to get difficulty: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(crate::node::minter::INITIAL_DIFFICULTY);

        // Get tip timestamp from the tip block (if exists)
        let tip_timestamp = if height > 0 {
            self.blocks_db
                .get(&rtxn, &height)
                .ok()
                .flatten()
                .and_then(|bytes| bincode::deserialize::<Block>(bytes).ok())
                .map(|block| block.header.timestamp)
                .unwrap_or(0)
        } else {
            0
        };

        // EmissionController state
        let total_tx = self
            .meta_db
            .get(&rtxn, META_TOTAL_TX)
            .map_err(|e| LedgerError::Database(format!("Failed to get total_tx: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let epoch_tx = self
            .meta_db
            .get(&rtxn, META_EPOCH_TX)
            .map_err(|e| LedgerError::Database(format!("Failed to get epoch_tx: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let epoch_emission = self
            .meta_db
            .get(&rtxn, META_EPOCH_EMISSION)
            .map_err(|e| LedgerError::Database(format!("Failed to get epoch_emission: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let epoch_burns = self
            .meta_db
            .get(&rtxn, META_EPOCH_BURNS)
            .map_err(|e| LedgerError::Database(format!("Failed to get epoch_burns: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);

        let current_reward = self
            .meta_db
            .get(&rtxn, META_CURRENT_REWARD)
            .map_err(|e| LedgerError::Database(format!("Failed to get current_reward: {}", e)))?
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
            .unwrap_or(crate::block::difficulty::INITIAL_REWARD);

        Ok(ChainState {
            height,
            tip_hash,
            tip_timestamp,
            total_mined,
            total_fees_burned,
            difficulty,
            total_tx,
            epoch_tx,
            epoch_emission,
            epoch_burns,
            current_reward,
        })
    }

    /// Get a block by height
    pub fn get_block(&self, height: u64) -> Result<Block, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let bytes = self
            .blocks_db
            .get(&rtxn, &height)
            .map_err(|e| LedgerError::Database(format!("Failed to get block: {}", e)))?
            .ok_or(LedgerError::BlockNotFound(height))?;

        bincode::deserialize(bytes).map_err(|e| LedgerError::Serialization(e.to_string()))
    }

    /// Get the tip (latest) block
    pub fn get_tip(&self) -> Result<Block, LedgerError> {
        let state = self.get_chain_state()?;
        self.get_block(state.height)
    }

    /// Get a block by its hash.
    ///
    /// Searches recent blocks (up to `lookback` blocks from tip) for a matching
    /// hash. This is used for compact block reconstruction when responding
    /// to GetBlockTxn requests.
    ///
    /// Returns `Ok(None)` if the block is not found within the lookback window.
    pub fn get_block_by_hash(
        &self,
        hash: &[u8; 32],
        lookback: u64,
    ) -> Result<Option<Block>, LedgerError> {
        let state = self.get_chain_state()?;

        // Quick check: is it the tip?
        if &state.tip_hash == hash {
            return self.get_block(state.height).map(Some);
        }

        // Search recent blocks
        let start_height = state.height.saturating_sub(lookback);
        for height in (start_height..state.height).rev() {
            match self.get_block(height) {
                Ok(block) => {
                    if &block.hash() == hash {
                        return Ok(Some(block));
                    }
                }
                Err(LedgerError::BlockNotFound(_)) => continue,
                Err(e) => return Err(e),
            }
        }

        Ok(None)
    }

    /// Add a new block to the chain
    pub fn add_block(&self, block: &Block) -> Result<(), LedgerError> {
        self.add_block_inner(block, None)
    }

    /// Add a block AND persist the emission-controller state in a SINGLE LMDB
    /// write transaction (audit cycle-6 **H3**, issue #558).
    ///
    /// This is the crash-atomic block-acceptance path used by the network
    /// integration. The block writes (UTXO set, key images, height/tip/total
    /// meta, lottery pool) and the emission writes (difficulty/reward/epoch
    /// counters) commit together or not at all, so a crash can never advance
    /// the chain height while leaving difficulty/epoch state stale (which
    /// would make the node compute a hard-validated difficulty that
    /// diverges from peers).
    ///
    /// The emission values must be the pure function of `block` that the caller
    /// would otherwise have passed to [`Ledger::update_emission_state`]; this
    /// method changes only *when* they are committed, never *what* is
    /// committed.
    pub fn add_block_with_emission(
        &self,
        block: &Block,
        emission: EmissionStateUpdate,
    ) -> Result<(), LedgerError> {
        self.add_block_inner(block, Some(emission))
    }

    /// Shared implementation for [`Ledger::add_block`] and
    /// [`Ledger::add_block_with_emission`]. When `emission` is `Some`, the
    /// emission-controller meta keys are written into the same `wtxn` as the
    /// block, before the single commit (issue #558).
    fn add_block_inner(
        &self,
        block: &Block,
        emission: Option<EmissionStateUpdate>,
    ) -> Result<(), LedgerError> {
        let state = self.get_chain_state()?;

        // Validate block height
        let expected_height = state.height + 1;
        if block.height() != expected_height {
            return Err(LedgerError::InvalidBlock(format!(
                "Expected height {}, got {}",
                expected_height,
                block.height()
            )));
        }

        // Validate prev_block_hash
        if block.header.prev_block_hash != state.tip_hash {
            return Err(LedgerError::InvalidBlock(
                "Previous block hash mismatch".to_string(),
            ));
        }

        // Header / minting-tx consistency: the minting tx must agree with the
        // header on the fields that feed PoW and emission. Otherwise a
        // producer can declare one difficulty/height/prev-hash in the header
        // (used by is_valid_pow and our checks here) and a different one in
        // the minting tx (which the SCP proposer path would have rejected).
        if block.minting_tx.block_height != block.height() {
            return Err(LedgerError::InvalidBlock(format!(
                "Minting tx height {} does not match header height {}",
                block.minting_tx.block_height,
                block.height()
            )));
        }
        if block.minting_tx.prev_block_hash != block.header.prev_block_hash {
            return Err(LedgerError::InvalidBlock(
                "Minting tx prev_block_hash does not match header".to_string(),
            ));
        }
        if block.minting_tx.difficulty != block.header.difficulty {
            return Err(LedgerError::InvalidBlock(
                "Minting tx difficulty does not match header".to_string(),
            ));
        }
        if block.minting_tx.minter_view_key != block.header.minter_view_key
            || block.minting_tx.minter_spend_key != block.header.minter_spend_key
        {
            return Err(LedgerError::InvalidBlock(
                "Minting tx minter keys do not match header".to_string(),
            ));
        }

        // C1: Enforce chain-expected difficulty.
        //
        // is_valid_pow() only proves the PoW hash is below the header's *own*
        // difficulty field — without this check a producer can declare a
        // trivial difficulty and have us accept the block at near-zero PoW.
        if block.header.difficulty != state.difficulty {
            return Err(LedgerError::InvalidBlock(format!(
                "Block difficulty {:#x} does not match expected {:#x}",
                block.header.difficulty, state.difficulty
            )));
        }

        // Validate PoW (against the now-verified expected difficulty).
        if !block.header.is_valid_pow() {
            return Err(LedgerError::InvalidBlock(
                "Invalid proof of work".to_string(),
            ));
        }

        // C2a: Recompute the block reward from the emission schedule and
        // chain state. Without this a producer can claim any reward, inflating
        // supply arbitrarily.
        let expected_reward = calculate_block_reward(block.height(), state.total_mined);
        if block.minting_tx.reward != expected_reward {
            return Err(LedgerError::InvalidBlock(format!(
                "Block reward {} does not match expected {} at height {}",
                block.minting_tx.reward,
                expected_reward,
                block.height()
            )));
        }

        // C2b: Timestamp sanity.
        //
        // Monotonicity vs parent prevents difficulty/emission games via
        // backdating; the future bound prevents a producer from biasing
        // timestamp-derived state forward. The header timestamp and the
        // minting-tx timestamp must agree (the SCP proposer path validates
        // the minting tx's timestamp; we hold the gossip path to the same
        // rule).
        if block.minting_tx.timestamp != block.header.timestamp {
            return Err(LedgerError::InvalidBlock(
                "Minting tx timestamp does not match header".to_string(),
            ));
        }
        if block.header.timestamp < state.tip_timestamp {
            return Err(LedgerError::InvalidBlock(format!(
                "Block timestamp {} is before parent {}",
                block.header.timestamp, state.tip_timestamp
            )));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|_| LedgerError::InvalidBlock("System time before UNIX epoch".to_string()))?;
        if block.header.timestamp > now.saturating_add(MAX_FUTURE_TIMESTAMP_SECS) {
            return Err(LedgerError::InvalidBlock(format!(
                "Block timestamp {} is too far in the future (now={})",
                block.header.timestamp, now
            )));
        }

        // C4: Verify the transaction root commits to the actual tx list.
        //
        // header.tx_root feeds the header hash and therefore PoW, but is
        // never recomputed at acceptance — a relay can swap the tx list
        // under a valid PoW unless we re-derive and compare here.
        let expected_tx_root = Block::compute_tx_root(&block.transactions);
        if block.header.tx_root != expected_tx_root {
            return Err(LedgerError::InvalidBlock(
                "Block tx_root does not match transactions".to_string(),
            ));
        }

        // Fee-sum overflow guard (#599, #663). Per-tx fees are
        // attacker-influenced, so accumulate with `checked_add` and reject on
        // overflow with a typed error rather than wrapping silently or (under
        // `overflow-checks = true`, which the release profile now carries)
        // panicking the node. Runs here — right after the tx list is bound to
        // the header (C4) and before the expensive gates — so a crafted
        // overflow block is rejected early and deterministically: the check is
        // a pure function of the block contents, so proposer and validators
        // reach the same verdict (no fork). The validated sum is reused by the
        // fee-accounting section below.
        let block_fees: u64 = checked_block_fees(block)?;

        // C5 (issue #451): Deterministic height-based staleness backstop.
        //
        // Reject any block that contains a transfer tx that is too old relative
        // to the block's OWN height. This mirrors the staleness rule that the
        // SCP transfer-validity gate used to enforce against the local *current
        // tip* (`validate_transfer_tx`, removed in #451 because tip-dependence
        // is the #417-class fork condition). Evaluated against `block.height()`
        // — which is identical on every honest node applying block N — this
        // check is deterministic and cannot diverge across nodes.
        //
        // Honest block-builders already filter these out at build time
        // (`BlockBuilder::build_from_externalized`), so this is defense-in-depth
        // against a malformed/adversarial block, not the primary gate (it never
        // fires for blocks we build → no externalize-then-reject halt).
        if let Some(tx_idx) = first_stale_transfer_tx(block) {
            let tx = &block.transactions[tx_idx];
            return Err(LedgerError::InvalidBlock(format!(
                "Transaction {} is stale: created_at_height {} + MAX_TX_AGE {} < block height {}",
                tx_idx,
                tx.created_at_height,
                MAX_TX_AGE,
                block.height()
            )));
        }

        // C3: Resolve every ring member against the UTXO set.
        //
        // CLSAG verifies signatures over the *claimed* ring; without this
        // check a producer can fabricate ring members (target_key they
        // control + arbitrary commitment) and the signature verifies while
        // the balance check passes against the fabricated amount, minting
        // value out of thin air. Mempool already does this for tx
        // admission, but blocks bypass the mempool.
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            self.verify_ring_members(tx).map_err(|e| {
                LedgerError::InvalidBlock(format!(
                    "Transaction {} ring member validation failed: {}",
                    tx_idx, e
                ))
            })?;
        }

        // C6 (issue #576, H2-B3): cluster-tag inflation guard.
        //
        // Wire the existing transaction-core conservation-of-mass validator
        // (`validate_cluster_tag_inheritance`) into the consensus path so a
        // block carrying a tx with tag-INFLATED outputs is rejected at block
        // acceptance, not merely at mempool admission (blocks bypass the
        // mempool). The check is a pure function of the transaction plus the
        // committed UTXO state — integer/`BTreeMap` only, no node-local state —
        // so a proposer and a validator reach the same verdict (no fork). It
        // runs after C3, which has already bound every ring member to a real
        // UTXO. See `check_cluster_tag_inheritance`.
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            self.verify_cluster_tag_inheritance(tx).map_err(|e| {
                LedgerError::InvalidBlock(format!(
                    "Transaction {} cluster-tag inheritance validation failed: {}",
                    tx_idx, e
                ))
            })?;
        }

        // C7 (issue #578, H1-B4): deterministic consensus fee floor.
        //
        // Reject any block containing a transfer tx whose `fee` is below the
        // consensus floor `base_minimum_fee + demurrage_charge` recomputed here
        // from the transaction and committed chain state at `block.height()`.
        // This is the sole enforcement of the demurrage stock-level term of the
        // Gini mechanism (a miner could otherwise include own/under-fee txs and
        // evade demurrage entirely). The floor is CONGESTION-FREE: the mempool's
        // node-local dynamic fee base (f64 EMA, restart-reset) is deliberately
        // excluded, so a node with a hot vs. cold congestion EMA accepts the
        // SAME set of blocks (design #574 Q1/Q2, audit cycle 6 M1). Runs after
        // C3 (ring resolution) and C6 (tag inheritance), reusing the same
        // committed-UTXO ring resolution, and BEFORE this block's outputs are
        // applied — so the proposer and every validator compute identical floors
        // (no fork). See `verify_consensus_fee_floor`.
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            self.verify_consensus_fee_floor(tx, block.height())
                .map_err(|e| match e {
                    LedgerError::InvalidBlock(msg) => LedgerError::InvalidBlock(format!(
                        "Transaction {} fee-floor validation failed: {}",
                        tx_idx, msg
                    )),
                    // Propagate DB errors unchanged (fail-closed, M7).
                    other => other,
                })?;
        }

        // C8 (issue #831): demurrage-settlement structural validity.
        //
        // A settlement is the sanctioned wrap on-ramp (#822/#825): it
        // reclassifies wealthy-cluster value down to factor-1/background in
        // exchange for wrap eligibility. C7 above already priced the capitalized
        // settlement charge — because a settlement's outputs are background, the
        // shared `spend_demurrage_charge` fires its `capitalized_reset_charge`
        // term (== `demurrage_settlement_charge`) at the ring-floored input
        // class, so an under-paid settlement is already rejected. This check
        // enforces the STRUCTURAL rules that make the `settlement` flag mean
        // exactly what `wrap_eligible` trusts: every output is background (the
        // tag-rewrite rule; the one sanctioned full mass-drop to background) and
        // the certified `settled_value` matches the outputs. Pure tag/integer
        // comparison — no node-local state, proposer == validator. Non-settlement
        // txs are untouched.
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            self.verify_settlement(tx).map_err(|e| match e {
                LedgerError::InvalidBlock(msg) => LedgerError::InvalidBlock(format!(
                    "Transaction {} settlement validation failed: {}",
                    tx_idx, msg
                )),
                other => other,
            })?;
        }

        // Validate lottery results and compute the new carryover pool.
        // The pool is consensus state: fees' pool share and the
        // height-scheduled emission share flow in; capped payouts flow out.
        let stored_lottery_pool = self.get_lottery_pool()?;
        let emission_share = block.minting_tx.lottery_emission_share();
        let new_lottery_pool = if block.total_fees() > 0
            || !block.lottery_outputs.is_empty()
            || emission_share > 0
            || stored_lottery_pool > 0
        {
            let lottery_config = LotteryFeeConfig::default();
            // prev_block_hash is used for verifiable randomness — both for the
            // candidate-window start offset and the draw itself.
            let prev_block_hash = &block.header.prev_block_hash;
            let candidates = self.get_lottery_validation_candidates(
                block.height(),
                prev_block_hash,
                &lottery_config.draw_config,
            )?;

            match validate_block_lottery(
                block,
                &candidates,
                stored_lottery_pool,
                prev_block_hash,
                &lottery_config,
            ) {
                Ok(new_pool) => new_pool,
                Err(e) => {
                    warn!(
                        block_height = block.height(),
                        error = %e,
                        "Lottery validation failed"
                    );
                    return Err(LedgerError::InvalidBlock(format!(
                        "Lottery validation failed: {}",
                        e
                    )));
                }
            }
        } else {
            stored_lottery_pool
        };

        // Store block and update metadata
        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        let block_bytes =
            bincode::serialize(block).map_err(|e| LedgerError::Serialization(e.to_string()))?;

        self.blocks_db
            .put(&mut wtxn, &block.height(), &block_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to put block: {}", e)))?;

        let new_hash = block.hash();
        let new_height = block.height();
        let new_total_mined = state.total_mined + block.minting_tx.reward as u128;

        // Fee accounting. Only the burn share of fees is actually destroyed;
        // the remainder flows to the redistribution lottery pool (and is paid
        // back out as lottery UTXOs). `total_fees_burned` therefore tracks the
        // validated burn amount, NOT the gross fee total — counting the full
        // fee would overstate destroyed supply 5x and break conservation
        // (audit cycle 6, M4). The lottery summary's burn amount was verified
        // against the pool accounting by `validate_block_lottery` above.
        //
        // `block_fees` was already validated overflow-free by the fee-sum
        // overflow guard up front (#599, #663 — mirrors the #340 balance
        // guard).
        let actually_burned = block.lottery_summary.amount_burned;
        let new_total_fees_burned = state.total_fees_burned + actually_burned as u128;

        // Create UTXO from minting reward (coinbase)
        let coinbase_utxo_id = UtxoId::new(new_hash, 0);
        let coinbase_utxo = Utxo {
            id: coinbase_utxo_id,
            output: block.minting_tx.to_tx_output(),
            created_at: new_height,
        };
        let coinbase_bytes = bincode::serialize(&coinbase_utxo)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        self.utxo_db
            .put(&mut wtxn, &coinbase_utxo_id.to_bytes(), &coinbase_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to put coinbase utxo: {}", e)))?;
        // Add to address index
        self.add_to_address_index(&mut wtxn, &coinbase_utxo)?;
        // Update cluster wealth tracking
        self.update_cluster_wealth_for_output(&mut wtxn, &coinbase_utxo.output)?;
        debug!("Created coinbase UTXO at height {}", new_height);

        // Verify and process regular transactions
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            // Verify transaction signatures before processing
            self.verify_transaction(tx)?;

            let tx_hash = tx.hash();

            // Index transaction for fast lookups (exchange integration)
            self.add_tx_to_index(&mut wtxn, &tx_hash, new_height, tx_idx as u32)?;

            // Process spent inputs - record key images to prevent double-spend
            for input in tx.inputs.clsag() {
                self.record_key_image(&mut wtxn, &input.key_image, new_height)?;
            }

            // Add new UTXOs (outputs)
            for (idx, output) in tx.outputs.iter().enumerate() {
                let utxo_id = UtxoId::new(tx_hash, idx as u32);
                let utxo = Utxo {
                    id: utxo_id,
                    output: output.clone(),
                    created_at: new_height,
                };
                let utxo_bytes = bincode::serialize(&utxo)
                    .map_err(|e| LedgerError::Serialization(e.to_string()))?;
                self.utxo_db
                    .put(&mut wtxn, &utxo_id.to_bytes(), &utxo_bytes)
                    .map_err(|e| LedgerError::Database(format!("Failed to put utxo: {}", e)))?;
                // Add to address index
                self.add_to_address_index(&mut wtxn, &utxo)?;
                // Update cluster wealth tracking
                self.update_cluster_wealth_for_output(&mut wtxn, output)?;
                // ADR 0007 (#938): if this output carries this-epoch's
                // bridge-import tag (an unwrap's minted output), record the
                // import cluster so the ≥F floor is enforceable at spend time.
                self.record_bridge_import_clusters_for_output(&mut wtxn, output, new_height)?;
            }
        }

        // Mint lottery payout UTXOs.
        //
        // Each payout creates a new spendable UTXO for the winner. The keys
        // and cluster tags are taken from the WINNING UTXO (looked up by its
        // id), not from the proposer-supplied fields on the LotteryOutput —
        // `validate_block_lottery` confirms each winner_utxo_id is an eligible
        // candidate and that payout totals match the pool accounting, but it
        // does not bind the output's target_key/public_key, so trusting those
        // fields would let a proposer redirect payouts to themselves.
        //
        // Deterministic id scheme: (block_hash, 1 + lottery_index). The
        // coinbase occupies (block_hash, 0); transaction outputs use the tx
        // hash (never the block hash), so payout ids cannot collide.
        for (lottery_idx, lottery_output) in block.lottery_outputs.iter().enumerate() {
            let winner_id = lottery_output.winner_utxo_id();
            let winner_bytes = self
                .utxo_db
                .get(&wtxn, &winner_id)
                .map_err(|e| LedgerError::Database(format!("Failed to read winning utxo: {}", e)))?
                .ok_or_else(|| {
                    LedgerError::InvalidBlock(format!(
                        "Lottery winner UTXO {} not found in set",
                        hex::encode(&winner_id[..8])
                    ))
                })?;
            let winner_utxo: Utxo = bincode::deserialize(winner_bytes)
                .map_err(|e| LedgerError::Serialization(e.to_string()))?;

            let payout_output = TxOutput {
                amount: lottery_output.payout,
                target_key: winner_utxo.output.target_key,
                public_key: winner_utxo.output.public_key,
                e_memo: None,
                cluster_tags: winner_utxo.output.cluster_tags.clone(),
                kem_ciphertext: None,
            };

            let payout_utxo_id = UtxoId::new(new_hash, (lottery_idx as u32) + 1);
            let payout_utxo = Utxo {
                id: payout_utxo_id,
                output: payout_output,
                created_at: new_height,
            };
            let payout_bytes = bincode::serialize(&payout_utxo)
                .map_err(|e| LedgerError::Serialization(e.to_string()))?;
            self.utxo_db
                .put(&mut wtxn, &payout_utxo_id.to_bytes(), &payout_bytes)
                .map_err(|e| {
                    LedgerError::Database(format!("Failed to put lottery payout utxo: {}", e))
                })?;
            self.add_to_address_index(&mut wtxn, &payout_utxo)?;
            self.update_cluster_wealth_for_output(&mut wtxn, &payout_utxo.output)?;
        }

        self.meta_db
            .put(&mut wtxn, META_HEIGHT, &new_height.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put height: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_TIP_HASH, &new_hash)
            .map_err(|e| LedgerError::Database(format!("Failed to put tip_hash: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_TOTAL_MINED, &new_total_mined.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put total_mined: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_FEES_BURNED,
                &new_total_fees_burned.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put fees_burned: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_LOTTERY_POOL,
                &new_lottery_pool.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put lottery_pool: {}", e)))?;

        // H3 (#558): fold the emission-controller state into the SAME write txn
        // as the block. These are the difficulty/reward/epoch counters that are
        // a pure function of the applied block; writing them here (rather than
        // in a separate `update_emission_state` commit) makes block + emission
        // state crash-atomic. Mirrors `update_emission_state` exactly — same
        // keys, same encoding — so a no-crash node persists identical values.
        if let Some(e) = emission {
            self.meta_db
                .put(&mut wtxn, META_DIFFICULTY, &e.difficulty.to_le_bytes())
                .map_err(|err| {
                    LedgerError::Database(format!("Failed to put difficulty: {}", err))
                })?;
            self.meta_db
                .put(&mut wtxn, META_TOTAL_TX, &e.total_tx.to_le_bytes())
                .map_err(|err| LedgerError::Database(format!("Failed to put total_tx: {}", err)))?;
            self.meta_db
                .put(&mut wtxn, META_EPOCH_TX, &e.epoch_tx.to_le_bytes())
                .map_err(|err| LedgerError::Database(format!("Failed to put epoch_tx: {}", err)))?;
            self.meta_db
                .put(
                    &mut wtxn,
                    META_EPOCH_EMISSION,
                    &e.epoch_emission.to_le_bytes(),
                )
                .map_err(|err| {
                    LedgerError::Database(format!("Failed to put epoch_emission: {}", err))
                })?;
            self.meta_db
                .put(&mut wtxn, META_EPOCH_BURNS, &e.epoch_burns.to_le_bytes())
                .map_err(|err| {
                    LedgerError::Database(format!("Failed to put epoch_burns: {}", err))
                })?;
            self.meta_db
                .put(
                    &mut wtxn,
                    META_CURRENT_REWARD,
                    &e.current_reward.to_le_bytes(),
                )
                .map_err(|err| {
                    LedgerError::Database(format!("Failed to put current_reward: {}", err))
                })?;
        }

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;

        info!(
            "Added block {} with hash {} ({} txs, {} fees burned)",
            new_height,
            hex::encode(&new_hash[0..8]),
            block.transactions.len(),
            block_fees
        );

        Ok(())
    }

    /// Get the redistribution lottery carryover pool balance.
    ///
    /// Consensus state: the pool accumulates the fee pool share plus the
    /// height-scheduled emission share, and drains via capped per-block
    /// payouts. Missing key (fresh/pre-upgrade ledger) means zero.
    ///
    /// The balance is the cumulative carryover and can grow without bound under
    /// sustained high-fee inflow, so it is `u128` (persisted as 16-byte LE).
    pub fn get_lottery_pool(&self) -> Result<u128, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;
        match self.meta_db.get(&rtxn, META_LOTTERY_POOL) {
            // Cumulative carryover persists as 16-byte LE u128 (widened from
            // 8-byte u64 to prevent saturation; rides the testnet reset #323).
            Ok(Some(bytes)) if bytes.len() == 16 => {
                Ok(u128::from_le_bytes(bytes.try_into().unwrap_or([0u8; 16])))
            }
            Ok(_) => Ok(0),
            Err(e) => Err(LedgerError::Database(format!(
                "Failed to get lottery_pool: {}",
                e
            ))),
        }
    }

    /// Update the difficulty in chain state
    pub fn set_difficulty(&self, difficulty: u64) -> Result<(), LedgerError> {
        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_DIFFICULTY, &difficulty.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put difficulty: {}", e)))?;
        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;
        Ok(())
    }

    /// Update emission controller state in chain state in its OWN write txn.
    ///
    /// NOTE (#558): the network block-acceptance path no longer uses this — it
    /// now folds the emission writes into the block's write txn via
    /// [`Ledger::add_block_with_emission`] so block + emission state are
    /// crash-atomic. This standalone two-commit variant is retained (rather
    /// than deleted, per the project's code-preservation guideline) for any
    /// out-of-band state correction and as the reference for the field
    /// encoding, which `add_block_with_emission` mirrors exactly.
    pub fn update_emission_state(
        &self,
        difficulty: u64,
        total_tx: u64,
        epoch_tx: u64,
        epoch_emission: u64,
        epoch_burns: u64,
        current_reward: u64,
    ) -> Result<(), LedgerError> {
        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        self.meta_db
            .put(&mut wtxn, META_DIFFICULTY, &difficulty.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put difficulty: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_TOTAL_TX, &total_tx.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put total_tx: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_EPOCH_TX, &epoch_tx.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_tx: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_EPOCH_EMISSION,
                &epoch_emission.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_emission: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_EPOCH_BURNS, &epoch_burns.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_burns: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_CURRENT_REWARD,
                &current_reward.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put current_reward: {}", e)))?;

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;
        Ok(())
    }

    /// Get blocks in a range (for syncing)
    pub fn get_blocks(&self, start_height: u64, count: usize) -> Result<Vec<Block>, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;
        let mut blocks = Vec::with_capacity(count);

        for height in start_height..(start_height + count as u64) {
            match self.blocks_db.get(&rtxn, &height) {
                Ok(Some(bytes)) => {
                    let block: Block = bincode::deserialize(bytes)
                        .map_err(|e| LedgerError::Serialization(e.to_string()))?;
                    blocks.push(block);
                }
                Ok(None) => break,
                Err(e) => return Err(LedgerError::Database(format!("Failed to get block: {}", e))),
            }
        }

        Ok(blocks)
    }

    /// Get a specific UTXO by ID
    pub fn get_utxo(&self, id: &UtxoId) -> Result<Option<Utxo>, LedgerError> {
        self.get_utxo_by_id(&id.to_bytes())
    }

    /// Get a specific UTXO by raw 36-byte ID (tx_hash || output_index)
    pub fn get_utxo_by_id(&self, id: &[u8; 36]) -> Result<Option<Utxo>, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        match self.utxo_db.get(&rtxn, id) {
            Ok(Some(bytes)) => {
                let utxo: Utxo = bincode::deserialize(bytes)
                    .map_err(|e| LedgerError::Serialization(e.to_string()))?;
                Ok(Some(utxo))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(LedgerError::Database(format!("Failed to get utxo: {}", e))),
        }
    }

    /// Get all UTXOs belonging to an address (using address index)
    pub fn get_utxos_for_address(&self, address: &PublicAddress) -> Result<Vec<Utxo>, LedgerError> {
        let view_key = address.view_public_key().to_bytes();
        let spend_key = address.spend_public_key().to_bytes();
        let addr_key = Self::address_key(&view_key, &spend_key);

        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Look up UTXO IDs from the address index
        let id_bytes = match self.address_index_db.get(&rtxn, &addr_key) {
            Ok(Some(bytes)) => bytes.to_vec(),
            Ok(None) => return Ok(Vec::new()),
            Err(e) => {
                return Err(LedgerError::Database(format!(
                    "Failed to get address index: {}",
                    e
                )))
            }
        };

        // Parse each 36-byte UTXO ID and fetch the corresponding UTXO
        let mut utxos = Vec::new();
        for chunk in id_bytes.chunks(36) {
            if chunk.len() == 36 {
                if let Some(utxo_id) = UtxoId::from_bytes(chunk) {
                    // Fetch the UTXO by ID
                    if let Ok(Some(utxo_bytes)) = self.utxo_db.get(&rtxn, &utxo_id.to_bytes()) {
                        if let Ok(utxo) = bincode::deserialize::<Utxo>(utxo_bytes) {
                            utxos.push(utxo);
                        }
                    }
                }
            }
        }

        Ok(utxos)
    }

    /// Get balance for an address (sum of all UTXOs)
    pub fn get_balance(&self, address: &PublicAddress) -> Result<u64, LedgerError> {
        let utxos = self.get_utxos_for_address(address)?;
        Ok(utxos.iter().map(|u| u.output.amount).sum())
    }

    /// Scan all UTXOs and return those belonging to the given account key.
    ///
    /// This performs stealth address detection by checking each output's
    /// target_key against the account's view key. This is necessary because
    /// stealth outputs use one-time addresses that can only be identified
    /// by the recipient.
    pub fn scan_utxos_for_account(
        &self,
        account_key: &AccountKey,
    ) -> Result<Vec<Utxo>, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let mut owned_utxos = Vec::new();

        // Iterate over all UTXOs
        let iter = self
            .utxo_db
            .iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;

        for result in iter {
            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    // Check if this output belongs to us using stealth detection
                    if utxo.output.belongs_to(account_key).is_some() {
                        owned_utxos.push(utxo);
                    }
                }
            }
        }

        Ok(owned_utxos)
    }

    /// Check if a UTXO exists (for transaction validation)
    pub fn utxo_exists(&self, id: &UtxoId) -> Result<bool, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;
        match self.utxo_db.get(&rtxn, &id.to_bytes()) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(LedgerError::Database(format!("Failed to get utxo: {}", e))),
        }
    }

    /// Get a UTXO by its target_key (one-time stealth public key)
    pub fn get_utxo_by_target_key(
        &self,
        target_key: &[u8; 32],
    ) -> Result<Option<Utxo>, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Look up UTXO IDs from the target_key index
        let id_bytes = match self.address_index_db.get(&rtxn, target_key.as_slice()) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(None),
            Err(e) => {
                return Err(LedgerError::Database(format!(
                    "Failed to get address index: {}",
                    e
                )))
            }
        };

        // Get the first UTXO ID (there should typically be only one per target_key)
        if id_bytes.len() >= 36 {
            if let Some(utxo_id) = UtxoId::from_bytes(&id_bytes[0..36]) {
                if let Ok(Some(utxo_bytes)) = self.utxo_db.get(&rtxn, &utxo_id.to_bytes()) {
                    if let Ok(utxo) = bincode::deserialize::<Utxo>(utxo_bytes) {
                        return Ok(Some(utxo));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Compute address key from view and spend keys for index lookup
    fn address_key(view_key: &[u8; 32], spend_key: &[u8; 32]) -> [u8; 64] {
        let mut key = [0u8; 64];
        key[0..32].copy_from_slice(view_key);
        key[32..64].copy_from_slice(spend_key);
        key
    }

    /// Add a UTXO ID to the address index
    fn add_to_address_index(&self, wtxn: &mut RwTxn, utxo: &Utxo) -> Result<(), LedgerError> {
        // Index by target_key for UTXO retrieval after stealth detection
        let target_key = &utxo.output.target_key;

        // Get existing IDs or empty vec
        let existing = match self.address_index_db.get(wtxn, target_key.as_slice()) {
            Ok(Some(bytes)) => bytes.to_vec(),
            Ok(None) => Vec::new(),
            Err(e) => {
                return Err(LedgerError::Database(format!(
                    "Failed to get address index: {}",
                    e
                )))
            }
        };

        // Append the new UTXO ID
        let mut ids = existing;
        ids.extend_from_slice(&utxo.id.to_bytes());

        self.address_index_db
            .put(wtxn, target_key.as_slice(), &ids)
            .map_err(|e| LedgerError::Database(format!("Failed to put address index: {}", e)))?;

        Ok(())
    }

    /// Remove a UTXO ID from the address index
    fn remove_from_address_index(&self, wtxn: &mut RwTxn, utxo: &Utxo) -> Result<(), LedgerError> {
        let target_key = &utxo.output.target_key;

        // Get existing IDs
        let existing = match self.address_index_db.get(wtxn, target_key.as_slice()) {
            Ok(Some(bytes)) => bytes.to_vec(),
            Ok(None) => return Ok(()), // Nothing to remove
            Err(e) => {
                return Err(LedgerError::Database(format!(
                    "Failed to get address index: {}",
                    e
                )))
            }
        };

        // Filter out the removed UTXO ID
        let utxo_id_bytes = utxo.id.to_bytes();
        let filtered: Vec<u8> = existing
            .chunks(36)
            .filter(|chunk| chunk != &utxo_id_bytes)
            .flat_map(|chunk| chunk.iter().copied())
            .collect();

        if filtered.is_empty() {
            // No more UTXOs for this target key, remove the entry
            let _ = self.address_index_db.delete(wtxn, target_key.as_slice());
        } else {
            self.address_index_db
                .put(wtxn, target_key.as_slice(), &filtered)
                .map_err(|e| {
                    LedgerError::Database(format!("Failed to put address index: {}", e))
                })?;
        }

        Ok(())
    }

    /// Verify all signatures in a transaction
    pub fn verify_transaction(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        // Verify key images haven't been spent (double-spend check).
        //
        // A DB error here is a node-local operational failure, NOT a statement
        // that the transaction is invalid. We distinguish two outcomes and fail
        // CLOSED on the error path (audit cycle 6, M7):
        //   - lookup succeeds and finds the key image spent -> reject as invalid
        //     (`LedgerError::InvalidBlock`, the existing/correct verdict)
        //   - lookup returns Err (DB failure) -> propagate the DB error up via `?` so
        //     the caller aborts/retries. Previously this used `if let Ok(Some(..))`,
        //     which silently treated a DB error as "not spent" and let a double-spend
        //     pass (fail-open). Propagating with `?` preserves the happy-path verdict
        //     exactly while closing the fail-open hole — a DB error never gets branded
        //     as block-invalid.
        for (i, input) in tx.inputs.clsag().iter().enumerate() {
            if let Some(spent_height) = self.is_key_image_spent(&input.key_image)? {
                return Err(LedgerError::InvalidBlock(format!(
                    "Input {} uses key image already spent at height {}",
                    i, spent_height
                )));
            }
        }

        // Verify CLSAG ring signatures
        tx.verify_ring_signatures()
            .map_err(|e| LedgerError::InvalidBlock(format!("Invalid ring signature: {}", e)))?;

        Ok(())
    }

    /// Verify every CLSAG ring member of `tx` resolves to a UTXO and matches
    /// the stored output's (target_key, public_key, commitment).
    ///
    /// CLSAG signs over the *claimed* ring data, so without this check a
    /// producer can include ring members with a target_key they control and
    /// an arbitrary amount-commitment: the signature still verifies and the
    /// balance check passes against the fabricated amount, minting value.
    /// The mempool does the equivalent check at admission, but blocks bypass
    /// the mempool — so this check is the block-level analogue.
    fn verify_ring_members(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        for (input_idx, input) in tx.inputs.clsag().iter().enumerate() {
            // Track whether the claimed per-input pseudo-output amount matches
            // any resolved ring member's real UTXO amount. The real input is
            // hidden among decoys, so we cannot identify it directly — but the
            // CLSAG balance proof asserts the real member's committed amount
            // equals `pseudo_output_amount`, and every ring member is verified
            // below to match a real UTXO. Requiring the pseudo-output amount to
            // equal some ring member's UTXO amount therefore binds it to a real
            // UTXO and prevents a producer claiming an inflated input amount to
            // unbalance the transaction-level sum (audit finding I4; composes
            // with the C3 commitment check below).
            let mut pseudo_amount_bound = false;

            for (member_idx, member) in input.ring.iter().enumerate() {
                let utxo = self
                    .get_utxo_by_target_key(&member.target_key)?
                    .ok_or_else(|| {
                        LedgerError::InvalidBlock(format!(
                            "Input {} ring member {} target_key not in UTXO set",
                            input_idx, member_idx
                        ))
                    })?;
                let expected = RingMember::from_output(&utxo.output);
                if expected != *member {
                    return Err(LedgerError::InvalidBlock(format!(
                        "Input {} ring member {} does not match UTXO (target_key/public_key/commitment mismatch — possible counterfeit amount)",
                        input_idx, member_idx
                    )));
                }
                if utxo.output.amount == input.pseudo_output_amount {
                    pseudo_amount_bound = true;
                }
            }

            if !pseudo_amount_bound {
                return Err(LedgerError::InvalidBlock(format!(
                    "Input {} pseudo-output amount {} does not match any resolved ring member's UTXO amount (possible counterfeit input amount)",
                    input_idx, input.pseudo_output_amount
                )));
            }
        }
        Ok(())
    }

    /// C6 (issue #576, H2-B3): reject a transaction whose outputs inflate
    /// cluster-tag mass beyond what its inputs can supply.
    ///
    /// Resolves the transaction's ring members against the committed UTXO set
    /// and applies the conservation-of-mass logic in
    /// [`check_cluster_tag_inheritance`]. Runs after C3
    /// (`verify_ring_members`), which already guarantees every ring member
    /// resolves to a real UTXO; a member that fails to resolve here is simply
    /// skipped (C3 would have aborted the block first). The resolved ring
    /// members are the only deterministic, node-agnostic input set — the real
    /// input is hidden among the decoys — and the real inputs are a subset of
    /// the ring, so this never false-rejects a valid block. See the
    /// determinism note on [`check_cluster_tag_inheritance`].
    ///
    /// Members are grouped **per ring** (#581): the tag-inheritance bound uses
    /// the sum over rings of each ring's per-cluster maximum member mass, so
    /// the input structure must preserve which members belong to which input.
    fn verify_cluster_tag_inheritance(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        let mut input_rings: Vec<Vec<(ClusterTagVector, u64)>> = Vec::new();
        for input in tx.inputs.clsag() {
            let mut ring: Vec<(ClusterTagVector, u64)> = Vec::new();
            for member in &input.ring {
                if let Some(utxo) = self.get_utxo_by_target_key(&member.target_key)? {
                    ring.push((utxo.output.cluster_tags.clone(), utxo.output.amount));
                }
            }
            input_rings.push(ring);
        }
        check_cluster_tag_inheritance(&input_rings, &tx.outputs)
    }

    /// C8 (issue #831): structural validity of an explicit demurrage-settlement
    /// transaction — the sanctioned wrap on-ramp (#822/#825).
    ///
    /// The *price* of a settlement (the capitalized future demurrage over
    /// `SETTLEMENT_HORIZON_BLOCKS`) is enforced by the consensus fee floor
    /// (`consensus_fee_floor`/`verify_consensus_fee_floor`, C7): a settlement
    /// declares background outputs, so the shared `spend_demurrage_charge`
    /// fires its `capitalized_reset_charge` term — identical to
    /// [`bth_cluster_tax::demurrage_settlement_charge`] — at the ring-floored
    /// input class the spender cannot rewrite. This method enforces the
    /// STRUCTURAL rules so the `settlement` flag certifies exactly what
    /// [`wrap_eligible`] trusts:
    ///
    /// - **Tag-rewrite rule:** every output MUST carry a background
    ///   (`ClusterTagVector::empty`) tag. A settlement reclassifies *all* value
    ///   to factor-1; this is the ONE sanctioned place cluster mass drops to
    ///   background, and it is bounded to a FULL drop — never a partial/cheap
    ///   laundering of provenance, which would leave a wealthy tag on a
    ///   supposedly-settled output.
    /// - **Certified value:** `settled_value == Σ output amounts`. The balance
    ///   equation already bounds the output sum by the resolved input value, so
    ///   this ties the flag's certified figure to the actual reclassified
    ///   value.
    ///
    /// Non-settlement transactions return `Ok(())` unchanged. Pure tag/integer
    /// comparison — no node-local state — so proposer and validator agree.
    fn verify_settlement(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        let Some(settlement) = &tx.settlement else {
            return Ok(());
        };

        // Tag-rewrite rule: a settlement produces ONLY background value.
        for (i, out) in tx.outputs.iter().enumerate() {
            if !out.cluster_tags.is_empty() {
                return Err(LedgerError::InvalidBlock(format!(
                    "settlement output {} is not background (carries {} cluster tag(s)); \
                     a settlement must reclassify all value to factor-1",
                    i,
                    out.cluster_tags.len()
                )));
            }
        }

        // Certified value must equal the summed (all-background) outputs.
        let output_sum = tx
            .outputs
            .iter()
            .fold(0u64, |acc, o| acc.saturating_add(o.amount));
        if settlement.settled_value != output_sum {
            return Err(LedgerError::InvalidBlock(format!(
                "settlement settled_value {} does not equal output sum {}",
                settlement.settled_value, output_sum
            )));
        }

        Ok(())
    }

    /// H1-B4 (issue #578, design #574): reject a transfer tx whose `fee` is
    /// below the deterministic consensus fee floor.
    ///
    /// The floor mirrors the mempool's per-tx minimum-fee computation
    /// (`Mempool::validate_transaction`) EXACTLY, minus the one
    /// non-deterministic term:
    ///
    /// ```text
    /// consensus_floor(tx) = base_minimum_fee(tx_size, num_outputs, num_memos,
    ///                                        cluster_wealth,  // NO congestion
    ///                                        CONSENSUS_FEE_BASE)
    ///                     + spend_demurrage_charge(output_sum,
    ///                                        floored_factor,  // composed input floor
    ///                                        claimed_factor,  // declared output
    ///                                        ring_elapsed_quantile@max,
    ///                                        rate_bps(height), blocks_per_year)
    /// require: tx.fee >= consensus_floor(tx)
    /// ```
    ///
    /// where `spend_demurrage_charge = max(accrued_to_date, capitalized_reset)`
    /// (issue #925): the capitalized term prices a genuine class downgrade
    /// (`claimed_factor < floored_factor`) at capitalized future demurrage over
    /// the shared `SETTLEMENT_HORIZON_BLOCKS`, closing the #834
    /// background-reset leak. It is zero for in-class and
    /// background→background spends, so it only ever raises the floor on an
    /// actual downgrade.
    ///
    /// # Why this is a pure function of block + chain state (design #574 Q5)
    ///
    /// - `tx_size`/`num_outputs`/`num_memos`/`output_sum` come straight off the
    ///   transaction.
    /// - The fee base is the fixed [`CONSENSUS_FEE_BASE`] constant, NOT the
    ///   mempool's `DynamicFeeBase` (whose f64 restart-reset EMA must never
    ///   reach consensus — audit cycle 6 M1). Congestion pricing stays a
    ///   mempool-relay-only policy that can only make a node stricter.
    /// - `cluster_wealth` is resolved from the committed per-cluster wealth
    ///   state via [`Ledger::get_cluster_wealth`], fail-closed (a DB error
    ///   propagates as `LedgerError`, never defaults to a lower floor — M7).
    ///   This read happens BEFORE the block's own outputs are applied (they are
    ///   written later in the single `add_block_inner` write txn), so the
    ///   proposer and every validator see the identical pre-block state.
    /// - The demurrage clock uses `ring_elapsed_quantile@max` over the ring
    ///   members' public `(value, created_at)` — an unweighted order statistic,
    ///   value-independent, so fresh high-value decoys cannot dilute it to zero
    ///   (H2/B1). The factor is floored at the ring-centroid-implied factor
    ///   (B2, [`Ledger::ring_centroid_floored_factor`]) so background-tagged
    ///   outputs cannot escape a wealthy ring's demurrage.
    /// - `rate_bps(height)` and `blocks_per_year` come from
    ///   [`crate::monetary::mainnet_policy`], evaluated at `block_height` (the
    ///   height being applied), identical on every node.
    ///
    /// All arithmetic is integer-only; the only maps used are the `BTreeMap`
    /// inside the reused helpers. No `HashMap`/`HashSet` iteration order feeds
    /// a consensus value. Applies from genesis (height 0) — no soft
    /// activation-height ramp inside the consensus path.
    ///
    /// Returns `Ok(())` when `tx.fee >= floor`,
    /// `Err(LedgerError::InvalidBlock)` when under-fee, and propagates
    /// `LedgerError::Database` on a ledger read error (fail-closed).
    fn verify_consensus_fee_floor(
        &self,
        tx: &BothoTransaction,
        block_height: u64,
    ) -> Result<(), LedgerError> {
        let floor = self.consensus_fee_floor(tx, block_height)?;
        if tx.fee < floor {
            return Err(LedgerError::InvalidBlock(format!(
                "transfer tx fee {} is below the consensus fee floor {} (height {})",
                tx.fee, floor, block_height
            )));
        }
        Ok(())
    }

    /// Test-only entry point to the private block-validity gate
    /// [`Ledger::verify_consensus_fee_floor`], so tests in sibling modules
    /// (e.g. the mempool relay-only-tightening invariant, issue #579) can
    /// exercise the real consensus acceptance path rather than re-deriving it.
    #[cfg(test)]
    pub fn verify_consensus_fee_floor_for_test(
        &self,
        tx: &BothoTransaction,
        block_height: u64,
    ) -> Result<(), LedgerError> {
        self.verify_consensus_fee_floor(tx, block_height)
    }

    /// Compute the deterministic consensus fee floor for a transfer tx.
    ///
    /// Split out from [`Ledger::verify_consensus_fee_floor`] so tests (and any
    /// independent recomputation) can assert the floor is a pure function of
    /// the block plus chain state. See that method for the full determinism
    /// contract.
    pub fn consensus_fee_floor(
        &self,
        tx: &BothoTransaction,
        block_height: u64,
    ) -> Result<u64, LedgerError> {
        use bth_cluster_tax::{FeeConfig, TransactionType as FeeTransactionType};

        // Consensus fee config: the canonical, fixed FeeConfig. This is a pure
        // constant (no node-local state), so every node computes the identical
        // curve. It matches the mempool's `FeeConfig::default()`.
        let fee_config = FeeConfig::default();

        // Base minimum fee (size + cluster factor + output penalty + memos),
        // congestion-free: the dynamic base is pinned to CONSENSUS_FEE_BASE.
        let tx_size_bytes = tx.estimate_size();
        let num_outputs = tx.outputs.len();
        let num_memos = tx.outputs.iter().filter(|o| o.has_memo()).count();

        // effective cluster wealth from the tx's OUTPUT tags weighted against
        // committed global per-cluster wealth (fail-closed on DB error, M7).
        let cluster_wealth = self.effective_cluster_wealth_from_outputs(&tx.outputs)?;

        let base_minimum_fee = fee_config.minimum_fee_dynamic_with_outputs(
            FeeTransactionType::Hidden,
            tx_size_bytes,
            cluster_wealth,
            num_outputs,
            num_memos,
            CONSENSUS_FEE_BASE,
        );

        // Demurrage charge. Resolve the ring members ONCE from the committed
        // UTXO set: we need (value, created_at) for the age quantile and
        // (value, &cluster_tags) for the factor floor. Fail-closed on DB error.
        let output_sum: u64 = tx
            .outputs
            .iter()
            .fold(0u64, |acc, o| acc.saturating_add(o.amount));

        let mut ring_age_members: Vec<(u64, u64)> = Vec::new();
        // Owns the resolved cluster-tag vectors so we can hand out borrows.
        let mut ring_tag_owned: Vec<(u64, ClusterTagVector)> = Vec::new();
        for input in tx.inputs.clsag() {
            for member in &input.ring {
                if let Some(utxo) = self.get_utxo_by_target_key(&member.target_key)? {
                    ring_age_members.push((utxo.output.amount, utxo.created_at));
                    ring_tag_owned.push((utxo.output.amount, utxo.output.cluster_tags.clone()));
                }
            }
        }

        let policy = crate::monetary::mainnet_policy();
        let blocks_per_year = (365 * 24 * 60 * 60) / policy.target_block_time_secs.max(1);

        // H2/B1: max-quantile age (value-independent order statistic) instead of
        // the value-weighted mean centroid — fresh decoys can no longer drag
        // the demurrage clock to zero.
        let elapsed = bth_cluster_tax::ring_elapsed_quantile(
            &ring_age_members,
            block_height,
            CONSENSUS_RING_AGE_QUANTILE_BPS,
        );

        // B2: floor the spender-claimed factor at the ring-centroid-implied
        // factor so background-tagged outputs cannot escape a wealthy ring.
        let claimed_factor = fee_config.cluster_factor(cluster_wealth);
        let ring_tag_refs: Vec<(u64, &ClusterTagVector)> = ring_tag_owned
            .iter()
            .map(|(value, tags)| (*value, tags))
            .collect();
        let demurrage_factor = self.ring_centroid_floored_factor(
            claimed_factor,
            &ring_tag_refs,
            &fee_config.cluster_curve,
        )?;

        // ADR 0007 (#938): the bridge-import floor. Imported wealth (tagged to a
        // bridge-import cluster) is priced at ≥ F on its import-tagged fraction,
        // so an unwrapped coin cannot be spent at background factor-1 until it
        // circulates the tag off. This is a SEPARATE lower bound from the
        // ring-centroid floor above: both are `max` against the demurrage
        // factor, so composing them is a single dominating `max` — no
        // double-floor. It only ever RAISES the factor, and only for value that
        // actually traces to a recorded import cluster.
        let import_floor = self.import_floor_factor_from_outputs(&tx.outputs)?;
        let demurrage_factor = demurrage_factor.max(import_floor);

        // #925 (background-reset leak, #834): total demurrage is
        // `max(accrued_to_date, capitalized_reset_charge)`. The capitalized term
        // prices a genuine class DOWNGRADE — the spender-declared output factor
        // `claimed_factor` dropping below the composed input-class floor
        // `demurrage_factor` (ring-centroid B2 + ADR-0007 import floor) — at
        // capitalized future demurrage over the shared
        // `SETTLEMENT_HORIZON_BLOCKS`, mirroring #831's settlement charge. It
        // fires ONLY on a downgrade (`claimed_factor < demurrage_factor`); an
        // in-class or background→background spend has `claimed_factor ==
        // demurrage_factor` ⇒ zero capitalized, so honest holds pay only their
        // accrued-to-date demurrage exactly as before. The young-coin exploit
        // (elapsed ≈ 0 ⇒ accrued ≈ 0) is now caught by the capitalized term.
        // Pure integer math; only ever RAISES the floor (liveness-safe).
        let demurrage = bth_cluster_tax::spend_demurrage_charge(
            output_sum,
            demurrage_factor,
            claimed_factor,
            elapsed,
            policy.demurrage_rate_bps(block_height),
            blocks_per_year,
        );

        Ok(base_minimum_fee.saturating_add(demurrage))
    }

    /// Effective cluster wealth from a transaction's output tags, weighted
    /// against the committed global per-cluster wealth.
    ///
    /// Mirrors the mempool's `effective_cluster_wealth_from_outputs`
    /// (`mempool.rs:59`) but reads per-cluster wealth through the ledger's own
    /// [`Ledger::get_cluster_wealth`], **fail-closed**: a DB read error
    /// propagates as `LedgerError` rather than defaulting a cluster to zero
    /// wealth (which would lower the progressive fee/factor — the M7 fail-open
    /// bug). `effective_wealth = Σ_outputs Σ_tags (value·weight/SCALE·W_global)
    /// / Σ_outputs value`; background (untagged) value contributes zero. Pure
    /// integer u128 accumulation; no float, no HashMap iteration into the
    /// result.
    fn effective_cluster_wealth_from_outputs(
        &self,
        outputs: &[TxOutput],
    ) -> Result<u128, LedgerError> {
        let mut total_weighted_wealth: u128 = 0;
        let mut total_value: u128 = 0;

        for output in outputs {
            total_value = total_value.saturating_add(output.amount as u128);
            for entry in &output.cluster_tags.entries {
                let value_fraction =
                    (output.amount as u128 * entry.weight as u128) / (TAG_WEIGHT_SCALE as u128);
                // `get_cluster_wealth` is full-u128 (16-byte accumulator, #626
                // PR2). The consensus fee floor now consumes u128 wealth
                // end-to-end (#626 PR3): the prior `as u64` truncation of the
                // centroid is gone, so `minimum_fee_dynamic_with_outputs` and
                // `cluster_factor` see the exact value. Saturating math keeps
                // the (astronomically-distant) u128 overflow deterministic —
                // pinning to u128::MAX → factor 6000 (max floor), the
                // conservative consensus direction; every node computes the
                // identical result, so no fork.
                let global_wealth = self.get_cluster_wealth(entry.cluster_id.0)?;
                total_weighted_wealth = total_weighted_wealth
                    .saturating_add(value_fraction.saturating_mul(global_wealth));
            }
        }

        if total_value == 0 {
            return Ok(0);
        }

        Ok(total_weighted_wealth / total_value)
    }

    /// The bridge-import factor floor implied by a transaction's OUTPUT tags
    /// (ADR 0007, #938).
    ///
    /// Returns a value-weighted blend, in FACTOR_SCALE units, of the
    /// bridge-import floor `F` on the value tagged to recorded import clusters
    /// and background `1×` on the rest:
    ///
    /// ```text
    /// import_floor = Σ_outputs Σ_import-tags (value·weight/SCALE)·F
    ///              + (Σ_outputs value − import-tagged value)·1×
    ///              ────────────────────────────────────────────────
    ///                              Σ_outputs value
    /// ```
    ///
    /// This mirrors the calibration sim's value-weighted `effective_factor`
    /// blend (`simulation::bridge_import_sweep`): a coin that is 100% import-
    /// tagged floors at exactly `F`; a coin whose import weight has blended
    /// down through domestic spends floors proportionally lower, decaying
    /// to `1×` as the import weight reaches zero (decay-by-circulation, ADR
    /// 0007 §4). A tx with no import-tagged value returns `1×`
    /// (FACTOR_SCALE), a no-op against the `max` at the call site.
    /// Membership is the recorded-import-cluster set
    /// (`is_bridge_import_cluster`), fail-closed on DB error.
    ///
    /// Pure integer u128 accumulation; no float. `TAG_WEIGHT_SCALE` weights and
    /// `FACTOR_SCALE` factors compose exactly.
    fn import_floor_factor_from_outputs(&self, outputs: &[TxOutput]) -> Result<u64, LedgerError> {
        use bth_cluster_tax::{ClusterFactorCurve, BRIDGE_IMPORT_FACTOR_FLOOR};

        let background = ClusterFactorCurve::FACTOR_SCALE as u128; // 1× in FACTOR_SCALE
        let floor = BRIDGE_IMPORT_FACTOR_FLOOR as u128; // F in FACTOR_SCALE

        let mut total_value: u128 = 0;
        let mut import_value: u128 = 0;

        for output in outputs {
            let amount = output.amount as u128;
            total_value = total_value.saturating_add(amount);
            for entry in &output.cluster_tags.entries {
                if self.is_bridge_import_cluster(entry.cluster_id.0)? {
                    let tagged =
                        amount.saturating_mul(entry.weight as u128) / (TAG_WEIGHT_SCALE as u128);
                    import_value = import_value.saturating_add(tagged);
                }
            }
        }

        if total_value == 0 {
            return Ok(ClusterFactorCurve::FACTOR_SCALE);
        }
        // Clamp defensively: a malformed tag set could push import_value past
        // total_value; the blend must never exceed F.
        let import_value = import_value.min(total_value);
        let background_value = total_value - import_value;

        // Value-weighted blend of F (import fraction) and 1× (rest).
        let blended = (floor
            .saturating_mul(import_value)
            .saturating_add(background.saturating_mul(background_value)))
            / total_value;
        Ok(blended as u64)
    }

    // ========================================================================
    // Key Image Tracking (for Ring Signature Double-Spend Prevention)
    // ========================================================================

    /// Check if a key image has already been spent.
    pub fn is_key_image_spent(&self, key_image: &[u8; 32]) -> Result<Option<u64>, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        match self.key_images_db.get(&rtxn, key_image.as_slice()) {
            Ok(Some(bytes)) if bytes.len() == 8 => {
                let height = u64::from_le_bytes(bytes.try_into().unwrap());
                Ok(Some(height))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(LedgerError::Database(format!(
                "Failed to get key image: {}",
                e
            ))),
        }
    }

    /// Record a key image as spent at the given block height.
    pub fn record_key_image(
        &self,
        wtxn: &mut RwTxn,
        key_image: &[u8; 32],
        height: u64,
    ) -> Result<(), LedgerError> {
        // Check if already exists.
        //
        // As with `verify_transaction`, distinguish a DB failure (node-local,
        // propagate via `?`) from an actual collision (consensus-invalid). The
        // previous `if let Ok(Some(..))` swallowed a DB `Err` and fell through to
        // the `put` below, which would record the key image and let a
        // double-spend through on a transient read error (fail-open, M7).
        let existing = self
            .key_images_db
            .get(wtxn, key_image.as_slice())
            .map_err(|e| LedgerError::Database(format!("Failed to get key image: {}", e)))?;
        if let Some(existing_height_bytes) = existing {
            let existing_height =
                u64::from_le_bytes(existing_height_bytes.try_into().unwrap_or([0u8; 8]));
            warn!(
                "Key image collision: {} already spent at height {}, trying to spend at height {}",
                hex::encode(&key_image[0..8]),
                existing_height,
                height
            );
            return Err(LedgerError::InvalidBlock(
                "Key image already spent (double-spend)".to_string(),
            ));
        }

        self.key_images_db
            .put(wtxn, key_image.as_slice(), &height.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put key image: {}", e)))
    }

    /// Test-only helper: record a key image as spent at the given height in a
    /// self-contained write transaction. Lets tests in sibling modules (e.g.
    /// the RPC layer's `chain_areKeyImagesSpent` test) seed the double-spend
    /// set without reaching into the private LMDB environment.
    #[cfg(test)]
    pub fn record_key_image_for_test(
        &self,
        key_image: &[u8; 32],
        height: u64,
    ) -> Result<(), LedgerError> {
        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;
        self.record_key_image(&mut wtxn, key_image, height)?;
        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;
        Ok(())
    }

    /// Get a random sample of UTXOs for use as decoys in ring signatures.
    pub fn get_decoy_outputs(
        &self,
        count: usize,
        exclude: &[[u8; 32]], // target_keys to exclude
        min_confirmations: u64,
    ) -> Result<Vec<TxOutput>, LedgerError> {
        use rand::seq::SliceRandom;

        let state = self.get_chain_state()?;
        let max_height = state.height.saturating_sub(min_confirmations);

        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Collect all eligible UTXOs
        let mut candidates: Vec<TxOutput> = Vec::new();

        // Iterate over all UTXOs
        let iter = self
            .utxo_db
            .iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;

        for result in iter {
            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    // Check confirmations
                    if utxo.created_at <= max_height {
                        // Check exclusion list
                        if !exclude.contains(&utxo.output.target_key) {
                            candidates.push(utxo.output);
                        }
                    }
                }
            }
        }

        // Randomly sample from candidates
        let mut rng = rand::thread_rng();
        candidates.shuffle(&mut rng);
        candidates.truncate(count);

        Ok(candidates)
    }

    /// Get decoys using OSPEAD-style gamma-weighted selection.
    ///
    /// This method selects decoys to match expected spend age patterns, making
    /// it harder for observers to distinguish real spends from decoys based
    /// on output age. Uses a gamma distribution to model real-world
    /// spending behavior.
    ///
    /// # Arguments
    /// * `count` - Number of decoys to select
    /// * `exclude` - Target keys to exclude (the real inputs)
    /// * `min_confirmations` - Minimum block confirmations required
    /// * `selector` - Optional custom gamma selector (uses default if None)
    ///
    /// # Returns
    /// Selected decoys weighted by age distribution
    pub fn get_decoy_outputs_ospead<R: Rng>(
        &self,
        count: usize,
        exclude: &[[u8; 32]],
        min_confirmations: u64,
        selector: Option<&GammaDecoySelector>,
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, LedgerError> {
        let state = self.get_chain_state()?;
        let current_height = state.height;
        let max_height = current_height.saturating_sub(min_confirmations);

        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Collect all eligible UTXOs with age information
        let mut candidates: Vec<OutputCandidate> = Vec::new();

        let iter = self
            .utxo_db
            .iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;

        for result in iter {
            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    // Check confirmations
                    if utxo.created_at <= max_height {
                        // Check exclusion list
                        if !exclude.contains(&utxo.output.target_key) {
                            candidates.push(OutputCandidate::from_utxo(&utxo, current_height));
                        }
                    }
                }
            }
        }

        // Use provided selector or create default
        let default_selector = GammaDecoySelector::new();
        let selector = selector.unwrap_or(&default_selector);

        // Use OSPEAD selection
        selector
            .select_decoys(&candidates, count, exclude, current_height, rng)
            .map_err(|e| match e {
                DecoySelectionError::InsufficientCandidates {
                    required,
                    available,
                } => LedgerError::InsufficientDecoys {
                    required,
                    available,
                },
                DecoySelectionError::InvalidDistribution => {
                    LedgerError::InvalidBlock("Invalid gamma distribution parameters".to_string())
                }
            })
    }

    /// Get decoys using OSPEAD selection, targeting specific ages for better
    /// anonymity.
    ///
    /// This version samples decoy ages based on the gamma distribution, then
    /// finds outputs that best match those ages. This creates rings where
    /// the age distribution matches expected real spending patterns.
    ///
    /// # Arguments
    /// * `count` - Number of decoys to select
    /// * `exclude` - Target keys to exclude
    /// * `min_confirmations` - Minimum block confirmations
    /// * `real_input_age` - Age in blocks of the real input being spent
    /// * `selector` - Optional custom gamma selector
    ///
    /// # Returns
    /// Selected decoys with age distribution matching spend patterns
    pub fn get_decoy_outputs_for_input<R: Rng>(
        &self,
        count: usize,
        exclude: &[[u8; 32]],
        min_confirmations: u64,
        real_input_age: u64,
        selector: Option<&GammaDecoySelector>,
        rng: &mut R,
    ) -> Result<Vec<TxOutput>, LedgerError> {
        let state = self.get_chain_state()?;
        let current_height = state.height;
        let max_height = current_height.saturating_sub(min_confirmations);

        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let mut candidates: Vec<OutputCandidate> = Vec::new();

        let iter = self
            .utxo_db
            .iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;

        for result in iter {
            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    if utxo.created_at <= max_height {
                        if !exclude.contains(&utxo.output.target_key) {
                            candidates.push(OutputCandidate::from_utxo(&utxo, current_height));
                        }
                    }
                }
            }
        }

        let default_selector = GammaDecoySelector::new();
        let selector = selector.unwrap_or(&default_selector);

        selector
            .select_decoys_for_input(&candidates, count, exclude, real_input_age, rng)
            .map_err(|e| match e {
                DecoySelectionError::InsufficientCandidates {
                    required,
                    available,
                } => LedgerError::InsufficientDecoys {
                    required,
                    available,
                },
                DecoySelectionError::InvalidDistribution => {
                    LedgerError::InvalidBlock("Invalid gamma distribution parameters".to_string())
                }
            })
    }

    /// Calculate effective anonymity for a ring given member ages.
    ///
    /// Returns a value between 1 (no privacy) and ring_size (perfect privacy).
    /// A value of 10+ with ring size 20 indicates good anonymity (1-in-10 or
    /// better).
    pub fn effective_anonymity(ring_ages: &[u64], selector: Option<&GammaDecoySelector>) -> f64 {
        let default_selector = GammaDecoySelector::new();
        let selector = selector.unwrap_or(&default_selector);
        selector.effective_anonymity(ring_ages)
    }

    // ========================================================================
    // Transaction Index (for Exchange Integration)
    // ========================================================================

    /// Add a transaction to the index.
    fn add_tx_to_index(
        &self,
        wtxn: &mut RwTxn,
        tx_hash: &[u8; 32],
        block_height: u64,
        tx_index: u32,
    ) -> Result<(), LedgerError> {
        // Encode location as 12 bytes: height (8) + tx_index (4)
        let mut location_bytes = [0u8; 12];
        location_bytes[0..8].copy_from_slice(&block_height.to_le_bytes());
        location_bytes[8..12].copy_from_slice(&tx_index.to_le_bytes());

        self.tx_index_db
            .put(wtxn, tx_hash.as_slice(), &location_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to index transaction: {}", e)))
    }

    /// Get the location of a transaction by its hash.
    ///
    /// Returns `Ok(Some(TxLocation))` if found, `Ok(None)` if not found.
    pub fn get_transaction_location(
        &self,
        tx_hash: &[u8; 32],
    ) -> Result<Option<TxLocation>, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        match self.tx_index_db.get(&rtxn, tx_hash.as_slice()) {
            Ok(Some(bytes)) if bytes.len() == 12 => {
                let block_height = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
                let tx_index = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
                Ok(Some(TxLocation {
                    block_height,
                    tx_index,
                }))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(LedgerError::Database(format!(
                "Failed to get tx location: {}",
                e
            ))),
        }
    }

    /// Get a transaction by its hash.
    ///
    /// Returns the transaction along with its block height and confirmation
    /// count.
    pub fn get_transaction(
        &self,
        tx_hash: &[u8; 32],
    ) -> Result<Option<(BothoTransaction, u64, u64)>, LedgerError> {
        // Look up location in index
        let location = match self.get_transaction_location(tx_hash)? {
            Some(loc) => loc,
            None => return Ok(None),
        };

        // Get the block
        let block = self.get_block(location.block_height)?;

        // Get the transaction from the block
        let tx = block
            .transactions
            .get(location.tx_index as usize)
            .ok_or_else(|| LedgerError::Database("Transaction index out of bounds".to_string()))?;

        // Calculate confirmations
        let chain_state = self.get_chain_state()?;
        let confirmations = chain_state.height.saturating_sub(location.block_height) + 1;

        Ok(Some((tx.clone(), location.block_height, confirmations)))
    }

    /// Get the confirmation count for a transaction.
    ///
    /// Returns `Ok(Some(confirmations))` if found, `Ok(None)` if not found.
    /// Confirmations = current_height - tx_block_height + 1
    pub fn get_transaction_confirmations(
        &self,
        tx_hash: &[u8; 32],
    ) -> Result<Option<u64>, LedgerError> {
        let location = match self.get_transaction_location(tx_hash)? {
            Some(loc) => loc,
            None => return Ok(None),
        };

        let chain_state = self.get_chain_state()?;
        let confirmations = chain_state.height.saturating_sub(location.block_height) + 1;
        Ok(Some(confirmations))
    }

    // ========================================================================
    // Cluster Wealth Tracking (for Progressive Fees)
    // ========================================================================
    //
    // # Privacy Implications
    //
    // Cluster wealth tracking enables progressive transaction fees but has privacy
    // considerations that users should understand:
    //
    // 1. **Cluster IDs are public**: Each transaction output has visible cluster
    //    tags that show what fraction of its value traces back to each cluster
    //    origin. This is inherent to the progressive fee design and visible
    //    on-chain.
    //
    // 2. **Wealth is observable**: Anyone can query cluster wealth from the public
    //    UTXO set. This reveals aggregate wealth concentrations but NOT individual
    //    wallet balances (UTXOs are stealth addresses).
    //
    // 3. **Ring signatures protect spending privacy**: While cluster wealth is
    //    visible, ring signatures hide which UTXO was actually spent in a
    //    transaction. The cluster tags on outputs inherit from the hidden real
    //    input's tags.
    //
    // 4. **Approximation due to ring signatures**: Since we cannot know which UTXO
    //    was spent (ring signature privacy), cluster wealth tracking is an
    //    approximation. Spent UTXOs continue contributing until explicitly pruned.
    //
    // 5. **Decay over time**: Cluster tags decay with each transaction (5% by
    //    default), so wealth attribution naturally fades as coins circulate.
    //
    // The progressive fee system intentionally uses visible cluster wealth to
    // ensure that large holders pay proportionally higher fees. This is a
    // design choice that trades some wealth privacy for fairer fee
    // distribution.

    /// Update cluster wealth when a new output is created.
    ///
    /// Adds the output's weighted cluster contributions to the global wealth
    /// tracker.
    fn update_cluster_wealth_for_output(
        &self,
        wtxn: &mut RwTxn,
        output: &TxOutput,
    ) -> Result<(), LedgerError> {
        for entry in &output.cluster_tags.entries {
            // Contribution = output_amount × tag_weight / TAG_WEIGHT_SCALE.
            // Kept in full u128 (no down-cast): the accumulator is now u128 so
            // cumulative wealth can exceed the former u64::MAX pico ceiling.
            let contribution =
                (output.amount as u128) * (entry.weight as u128) / (TAG_WEIGHT_SCALE as u128);

            if contribution > 0 {
                let cluster_key = entry.cluster_id.0.to_le_bytes();

                // Get current wealth (16-byte LE u128, reject-legacy).
                let current = match self
                    .cluster_wealth_db
                    .get(wtxn, cluster_key.as_slice())
                    .map_err(|e| {
                        LedgerError::Database(format!("Failed to get cluster wealth: {}", e))
                    })? {
                    Some(bytes) => decode_cluster_wealth(bytes)?,
                    None => 0u128,
                };

                // Add contribution. saturating_add on u128 keeps byte-identical
                // semantics with the rebuild path (`rebuild_cluster_wealth_index`);
                // saturation at u128::MAX is astronomically unreachable but the
                // discipline is preserved (M3 lesson, #604/#607).
                let new_wealth = current.saturating_add(contribution);
                self.cluster_wealth_db
                    .put(wtxn, cluster_key.as_slice(), &new_wealth.to_le_bytes())
                    .map_err(|e| {
                        LedgerError::Database(format!("Failed to update cluster wealth: {}", e))
                    })?;
            }
        }
        Ok(())
    }

    /// Record any bridge-import cluster tags carried by an output created at
    /// `height` (ADR 0007, #938).
    ///
    /// An output tag is a genuine bridge-import origin iff its cluster id
    /// equals `import_cluster_id(⌊height/K⌋)` — the deterministic epoch key
    /// for the block that creates it. This is the ONLY way a cluster enters
    /// the import set: the derivation is a pure function of the block
    /// height and the tag, so every node records the identical set (no
    /// fork). A domestic mint or ordinary transfer output can only be
    /// recorded here if its randomly- derived tag id collides with this
    /// exact epoch's hash-derived import id (cryptographically negligible),
    /// and even then the id *is* that epoch's import cluster, so treating
    /// it as one is consistent.
    fn record_bridge_import_clusters_for_output(
        &self,
        wtxn: &mut RwTxn,
        output: &TxOutput,
        height: u64,
    ) -> Result<(), LedgerError> {
        let epoch_import_id = bth_cluster_tax::import_cluster_id_for_height(height).0;
        for entry in &output.cluster_tags.entries {
            if entry.cluster_id.0 == epoch_import_id {
                let key = epoch_import_id.to_le_bytes();
                self.bridge_import_clusters_db
                    .put(wtxn, key.as_slice(), &[])
                    .map_err(|e| {
                        LedgerError::Database(format!(
                            "Failed to record bridge-import cluster: {}",
                            e
                        ))
                    })?;
            }
        }
        Ok(())
    }

    /// Whether `cluster_id` is a recorded bridge-import cluster (ADR 0007).
    ///
    /// Presence-only lookup; fail-closed on DB error (propagates rather than
    /// defaulting to `false`, which would silently drop the ≥F import floor —
    /// the M7 fail-closed discipline).
    pub fn is_bridge_import_cluster(&self, cluster_id: u64) -> Result<bool, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;
        let key = cluster_id.to_le_bytes();
        match self.bridge_import_clusters_db.get(&rtxn, key.as_slice()) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(LedgerError::Database(format!(
                "Failed to read bridge-import cluster: {}",
                e
            ))),
        }
    }

    /// Get the total wealth attributed to a specific cluster.
    ///
    /// Returns the sum of (amount × weight / TAG_WEIGHT_SCALE) for all UTXOs
    /// with tags referencing this cluster.
    pub fn get_cluster_wealth(&self, cluster_id: u64) -> Result<u128, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let cluster_key = cluster_id.to_le_bytes();
        match self.cluster_wealth_db.get(&rtxn, cluster_key.as_slice()) {
            Ok(Some(bytes)) => decode_cluster_wealth(bytes),
            Ok(None) => Ok(0),
            Err(e) => Err(LedgerError::Database(format!(
                "Failed to get cluster wealth: {}",
                e
            ))),
        }
    }

    /// Floor a spender-claimed cluster factor at the factor implied by the ring
    /// centroid (audit cycle 6 H2, design #574 item B2).
    ///
    /// The `claimed_factor` is derived from a transaction's spender-authored
    /// OUTPUT tags, so on its own it is gameable: a wealthy spender can tag
    /// outputs as background (factor 1x) and pay ~zero demurrage. This raises
    /// it to at least the factor implied by the RING MEMBERS' own (public,
    /// inherited) cluster tags, which the spender cannot rewrite — so fresh
    /// background decoys can no longer drive the demurrage factor below what
    /// the ring composition implies. The floor can only ever RAISE the
    /// factor; a genuinely-background ring leaves a 1x claim unchanged.
    ///
    /// Per-cluster wealth is resolved from the ledger **fail-closed**: a DB
    /// read error propagates as `LedgerError` rather than silently
    /// defaulting to zero wealth (which would lower the floor — matching
    /// the M7 fail-closed fix). The factor math is the consensus-safe,
    /// integer-only, node-local-state-free helper
    /// [`bth_cluster_tax::ring_centroid_implied_factor`], which the consensus
    /// fee-floor enforcement (item B4) can reuse unchanged.
    ///
    /// # Arguments
    /// * `claimed_factor` - The cluster factor implied by the spender's output
    ///   tags, in FACTOR_SCALE units.
    /// * `ring_members` - The resolved `(value, cluster_tags)` of every ring
    ///   member across all inputs (public chain data).
    /// * `curve` - The progressive cluster factor curve.
    ///
    /// # Returns
    /// `max(claimed_factor, ring_centroid_implied_factor)` in FACTOR_SCALE
    /// units.
    pub fn ring_centroid_floored_factor(
        &self,
        claimed_factor: u64,
        ring_members: &[(u64, &ClusterTagVector)],
        curve: &bth_cluster_tax::ClusterFactorCurve,
    ) -> Result<u64, LedgerError> {
        // Resolve each ring member's value-normalized effective cluster wealth
        // (Σ_tag weight × W_global / TAG_WEIGHT_SCALE), fail-closed.
        let mut members: Vec<(u64, u128)> = Vec::with_capacity(ring_members.len());
        for (value, tags) in ring_members {
            let mut member_wealth: u128 = 0;
            for entry in &tags.entries {
                // `get_cluster_wealth` is full-u128 (16-byte LE accumulator).
                let global_wealth = self.get_cluster_wealth(entry.cluster_id.0)?;
                member_wealth = member_wealth.saturating_add(
                    (entry.weight as u128).saturating_mul(global_wealth) / TAG_WEIGHT_SCALE as u128,
                );
            }
            // `ring_centroid_implied_factor` now takes full-u128 wealth (#626
            // PR3) — the prior `.min(u64::MAX)` clamp is gone, so the consensus
            // fee floor sees the exact per-member cumulative wealth.
            //
            // Overflow is saturated at each multiply on this consensus path:
            //   - here, `weight × global_wealth` saturates to u128::MAX before the divide
            //     (global_wealth is the unbounded, monotonic PR2 accumulator, so the
            //     product is not provably < u128::MAX);
            //   - inside `ring_centroid_implied_factor`, the `value × member_wealth` and
            //     `total_value` products likewise saturate.
            // A saturated wealth maps deterministically to factor 6000 (the max,
            // most-conservative fee floor) — identical on every node → no fork.
            members.push((*value, member_wealth));
        }

        let implied = bth_cluster_tax::ring_centroid_implied_factor(&members, curve);
        Ok(claimed_factor.max(implied))
    }

    /// Compute effective cluster wealth for a set of UTXOs identified by
    /// target keys.
    ///
    /// This is the primary method for wallets to estimate their cluster wealth
    /// for fee calculation. For each UTXO, the effective wealth is the UTXO's
    /// tag weights averaged against the GLOBAL per-cluster wealth tracked by
    /// the ledger (background weight contributes zero). The maximum across the
    /// provided UTXOs is returned, since any of them may fund a transaction.
    ///
    /// This matches mempool fee enforcement, which uses global cluster wealth
    /// rather than per-transaction value — splitting funds does not reduce the
    /// fee rate.
    ///
    /// # Arguments
    /// * `target_keys` - Target keys (stealth addresses) identifying the UTXOs
    ///
    /// # Returns
    /// A `ClusterWealthInfo` containing the maximum effective cluster wealth
    /// and a breakdown of global wealth per referenced cluster
    pub fn compute_cluster_wealth_for_utxos(
        &self,
        target_keys: &[[u8; 32]],
    ) -> Result<ClusterWealthInfo, LedgerError> {
        use std::collections::HashMap;

        let mut global_wealths: HashMap<u64, u128> = HashMap::new();
        let mut max_effective_wealth = 0u128;
        let mut total_value = 0u64;
        let mut utxo_count = 0usize;

        for target_key in target_keys {
            if let Some(utxo) = self.get_utxo_by_target_key(target_key)? {
                total_value = total_value.saturating_add(utxo.output.amount);
                utxo_count += 1;

                let mut weighted: u128 = 0;
                for entry in &utxo.output.cluster_tags.entries {
                    let global = match global_wealths.entry(entry.cluster_id.0) {
                        std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                        std::collections::hash_map::Entry::Vacant(e) => {
                            *e.insert(self.get_cluster_wealth(entry.cluster_id.0)?)
                        }
                    };
                    weighted =
                        weighted.saturating_add((entry.weight as u128).saturating_mul(global));
                }
                // Divide by full scale: background weight dilutes toward zero
                let effective = weighted / TAG_WEIGHT_SCALE as u128;
                max_effective_wealth = max_effective_wealth.max(effective);
            }
        }

        let dominant_cluster = global_wealths
            .iter()
            .max_by_key(|(_, &wealth)| wealth)
            .map(|(&id, _)| id);

        Ok(ClusterWealthInfo {
            max_cluster_wealth: max_effective_wealth,
            total_value,
            utxo_count,
            dominant_cluster_id: dominant_cluster,
            cluster_breakdown: global_wealths.into_iter().collect(),
        })
    }

    /// Get all cluster wealth entries for analytics.
    ///
    /// Returns all tracked cluster IDs and their total wealth.
    /// Useful for network-wide wealth distribution analysis.
    pub fn get_all_cluster_wealth(&self) -> Result<Vec<(u64, u128)>, LedgerError> {
        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let mut result = Vec::new();
        let iter = self.cluster_wealth_db.iter(&rtxn).map_err(|e| {
            LedgerError::Database(format!("Failed to iterate cluster wealth: {}", e))
        })?;

        for item in iter {
            if let Ok((key, value)) = item {
                if key.len() == 8 {
                    let cluster_id = u64::from_le_bytes(key.try_into().unwrap());
                    // 16-byte LE u128, reject-legacy (fail closed on wrong width).
                    let wealth = decode_cluster_wealth(value)?;
                    result.push((cluster_id, wealth));
                }
            }
        }

        Ok(result)
    }

    /// Rebuild cluster wealth index from UTXO set.
    ///
    /// Scans all UTXOs and rebuilds the cluster wealth index from scratch.
    /// Useful for database repair or migration.
    pub fn rebuild_cluster_wealth_index(&self) -> Result<usize, LedgerError> {
        use std::collections::HashMap;

        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // First pass: compute wealth from all UTXOs, and reconstruct the
        // bridge-import cluster set (ADR 0007, #938) from each UTXO's
        // `created_at` height — an output tag is an import origin iff its id
        // equals `import_cluster_id(⌊created_at/K⌋)`, exactly the incremental
        // `record_bridge_import_clusters_for_output` rule, so the rebuilt set is
        // byte-identical.
        let mut cluster_wealths: HashMap<u64, u128> = HashMap::new();
        let mut import_clusters: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let iter = self
            .utxo_db
            .iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to iterate UTXOs: {}", e)))?;

        for item in iter {
            if let Ok((_, value)) = item {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    let epoch_import_id =
                        bth_cluster_tax::import_cluster_id_for_height(utxo.created_at).0;
                    for entry in &utxo.output.cluster_tags.entries {
                        // Full u128 contribution (no down-cast), matching the
                        // incremental path exactly at the new width.
                        let contribution = (utxo.output.amount as u128) * (entry.weight as u128)
                            / (TAG_WEIGHT_SCALE as u128);
                        // Saturating add for determinism parity with the
                        // incremental path (`update_cluster_wealth_for_output`),
                        // which uses `saturating_add`. A rebuilt node must
                        // produce byte-identical cluster wealth to an
                        // incrementally-accumulated one, since progressive-fee /
                        // demurrage inputs read `cluster_wealth_db`. See #604.
                        let e = cluster_wealths.entry(entry.cluster_id.0).or_insert(0u128);
                        *e = e.saturating_add(contribution);

                        if entry.cluster_id.0 == epoch_import_id {
                            import_clusters.insert(epoch_import_id);
                        }
                    }
                }
            }
        }
        drop(rtxn);

        // Second pass: write to database
        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        // Clear existing
        self.cluster_wealth_db
            .clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear cluster wealth: {}", e)))?;
        self.bridge_import_clusters_db
            .clear(&mut wtxn)
            .map_err(|e| {
                LedgerError::Database(format!("Failed to clear bridge-import clusters: {}", e))
            })?;

        // Write new values
        for (cluster_id, wealth) in &cluster_wealths {
            self.cluster_wealth_db
                .put(&mut wtxn, &cluster_id.to_le_bytes(), &wealth.to_le_bytes())
                .map_err(|e| {
                    LedgerError::Database(format!("Failed to write cluster wealth: {}", e))
                })?;
        }
        for cluster_id in &import_clusters {
            self.bridge_import_clusters_db
                .put(&mut wtxn, &cluster_id.to_le_bytes(), &[])
                .map_err(|e| {
                    LedgerError::Database(format!("Failed to write bridge-import cluster: {}", e))
                })?;
        }

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;

        Ok(cluster_wealths.len())
    }

    // ========================================================================
    // Snapshot Support
    // ========================================================================

    /// Create a UTXO snapshot at the current chain height.
    ///
    /// This captures the complete UTXO set, key images, and cluster wealth
    /// for fast initial sync of new nodes.
    ///
    /// # Returns
    ///
    /// A `UtxoSnapshot` containing all state needed to bootstrap a node.
    pub fn create_snapshot(&self) -> Result<super::UtxoSnapshot, LedgerError> {
        use super::snapshot::UtxoSnapshot;

        let chain_state = self.get_chain_state()?;
        let tip = self.get_tip()?;
        let block_hash = tip.hash();

        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        // Collect all UTXOs
        let mut utxos = Vec::new();
        let utxo_iter = self
            .utxo_db
            .iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to iterate UTXOs: {}", e)))?;

        for result in utxo_iter {
            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    utxos.push(utxo);
                }
            }
        }

        // Collect all key images
        let mut key_images = Vec::new();
        let ki_iter = self
            .key_images_db
            .iter(&rtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to iterate key images: {}", e)))?;

        for result in ki_iter {
            if let Ok((key, value)) = result {
                if key.len() == 32 && value.len() == 8 {
                    let mut ki = [0u8; 32];
                    ki.copy_from_slice(key);
                    let height = u64::from_le_bytes(value.try_into().unwrap());
                    key_images.push((ki, height));
                }
            }
        }

        // Collect cluster wealth data
        let mut cluster_wealth = Vec::new();
        let cw_iter = self.cluster_wealth_db.iter(&rtxn).map_err(|e| {
            LedgerError::Database(format!("Failed to iterate cluster wealth: {}", e))
        })?;

        for result in cw_iter {
            if let Ok((key, value)) = result {
                if key.len() == 8 {
                    let cluster_id = u64::from_le_bytes(key.try_into().unwrap());
                    // 16-byte LE u128, reject-legacy (#626).
                    let wealth = decode_cluster_wealth(value)?;
                    cluster_wealth.push((cluster_id, wealth));
                }
            }
        }

        drop(rtxn);

        info!(
            height = chain_state.height,
            utxo_count = utxos.len(),
            key_image_count = key_images.len(),
            cluster_count = cluster_wealth.len(),
            "Creating UTXO snapshot"
        );

        UtxoSnapshot::new(
            chain_state.height,
            block_hash,
            chain_state,
            utxos,
            key_images,
            cluster_wealth,
        )
        .map_err(|e| LedgerError::Serialization(e.to_string()))
    }

    /// Load ledger state from a snapshot.
    ///
    /// This replaces the current ledger state with the snapshot data.
    /// The snapshot is verified before loading.
    ///
    /// # Arguments
    ///
    /// * `snapshot` - The snapshot to load
    /// * `expected_block_hash` - Optional block hash to verify against
    ///
    /// # Returns
    ///
    /// The number of UTXOs loaded.
    pub fn load_from_snapshot(
        &self,
        snapshot: &super::UtxoSnapshot,
        expected_block_hash: Option<&[u8; 32]>,
    ) -> Result<usize, LedgerError> {
        // Verify snapshot integrity
        snapshot.verify().map_err(|e| {
            LedgerError::InvalidBlock(format!("Snapshot verification failed: {}", e))
        })?;

        // Verify block hash if provided
        if let Some(expected) = expected_block_hash {
            if &snapshot.block_hash != expected {
                return Err(LedgerError::InvalidBlock("Block hash mismatch".to_string()));
            }
        }

        info!(
            height = snapshot.height,
            utxo_count = snapshot.utxo_count,
            key_image_count = snapshot.key_image_count,
            "Loading ledger from snapshot"
        );

        // Extract data from snapshot
        let utxos = snapshot
            .get_utxos()
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        let key_images = snapshot
            .get_key_images()
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        let cluster_wealth = snapshot
            .get_cluster_wealth()
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;

        let mut wtxn = self
            .env
            .write_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start write txn: {}", e)))?;

        // Clear existing data
        self.utxo_db
            .clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear UTXO db: {}", e)))?;
        self.key_images_db
            .clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear key images db: {}", e)))?;
        self.address_index_db
            .clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear address index: {}", e)))?;
        self.cluster_wealth_db
            .clear(&mut wtxn)
            .map_err(|e| LedgerError::Database(format!("Failed to clear cluster wealth: {}", e)))?;

        // Load UTXOs
        let utxo_count = utxos.len();
        for utxo in utxos {
            let utxo_bytes =
                bincode::serialize(&utxo).map_err(|e| LedgerError::Serialization(e.to_string()))?;
            self.utxo_db
                .put(&mut wtxn, &utxo.id.to_bytes(), &utxo_bytes)
                .map_err(|e| LedgerError::Database(format!("Failed to put UTXO: {}", e)))?;

            // Rebuild address index
            self.add_to_address_index(&mut wtxn, &utxo)?;
        }

        // Load key images
        for (ki, height) in key_images {
            self.key_images_db
                .put(&mut wtxn, &ki, &height.to_le_bytes())
                .map_err(|e| LedgerError::Database(format!("Failed to put key image: {}", e)))?;
        }

        // Load cluster wealth
        for (cluster_id, wealth) in cluster_wealth {
            self.cluster_wealth_db
                .put(&mut wtxn, &cluster_id.to_le_bytes(), &wealth.to_le_bytes())
                .map_err(|e| {
                    LedgerError::Database(format!("Failed to put cluster wealth: {}", e))
                })?;
        }

        // Update metadata
        self.meta_db
            .put(
                &mut wtxn,
                META_HEIGHT,
                &snapshot.chain_state.height.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put height: {}", e)))?;
        self.meta_db
            .put(&mut wtxn, META_TIP_HASH, &snapshot.block_hash)
            .map_err(|e| LedgerError::Database(format!("Failed to put tip_hash: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_TOTAL_MINED,
                &snapshot.chain_state.total_mined.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put total_mined: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_FEES_BURNED,
                &snapshot.chain_state.total_fees_burned.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put fees_burned: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_DIFFICULTY,
                &snapshot.chain_state.difficulty.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put difficulty: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_TOTAL_TX,
                &snapshot.chain_state.total_tx.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put total_tx: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_EPOCH_TX,
                &snapshot.chain_state.epoch_tx.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_tx: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_EPOCH_EMISSION,
                &snapshot.chain_state.epoch_emission.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_emission: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_EPOCH_BURNS,
                &snapshot.chain_state.epoch_burns.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put epoch_burns: {}", e)))?;
        self.meta_db
            .put(
                &mut wtxn,
                META_CURRENT_REWARD,
                &snapshot.chain_state.current_reward.to_le_bytes(),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to put current_reward: {}", e)))?;

        wtxn.commit()
            .map_err(|e| LedgerError::Database(format!("Failed to commit: {}", e)))?;

        info!(utxo_count = utxo_count, "Snapshot loaded successfully");

        Ok(utxo_count)
    }

    /// Write a snapshot to a file.
    pub fn write_snapshot_to_file(&self, path: &std::path::Path) -> Result<u64, LedgerError> {
        let snapshot = self.create_snapshot()?;

        let file = std::fs::File::create(path)
            .map_err(|e| LedgerError::Database(format!("Failed to create file: {}", e)))?;

        let mut writer = std::io::BufWriter::new(file);
        snapshot
            .write_to(&mut writer)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;

        let size = writer
            .into_inner()
            .map_err(|e| LedgerError::Database(format!("Failed to flush: {}", e)))?
            .metadata()
            .map_err(|e| LedgerError::Database(format!("Failed to get metadata: {}", e)))?
            .len();

        info!(
            path = %path.display(),
            size_bytes = size,
            "Snapshot written to file"
        );

        Ok(size)
    }

    /// Load a snapshot from a file.
    pub fn load_snapshot_from_file(
        &self,
        path: &std::path::Path,
        expected_block_hash: Option<&[u8; 32]>,
    ) -> Result<usize, LedgerError> {
        let file = std::fs::File::open(path)
            .map_err(|e| LedgerError::Database(format!("Failed to open file: {}", e)))?;

        let reader = std::io::BufReader::new(file);
        let snapshot = super::UtxoSnapshot::read_from(reader)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;

        self.load_from_snapshot(&snapshot, expected_block_hash)
    }

    // ========================================================================
    // Lottery Candidate Selection (Block Production AND Validation)
    // ========================================================================

    /// Get all UTXOs eligible for lottery participation.
    ///
    /// This method returns `LotteryCandidate` objects (with real cluster
    /// factors) for all UTXOs that meet the eligibility requirements at the
    /// specified block height. It is the SINGLE source of lottery candidates
    /// for both block production and block validation: verification re-runs
    /// the draw, so any divergence in the candidate set forks consensus.
    /// (Previously the proposer used a separate `get_lottery_candidates`
    /// with different age/value thresholds — a latent consensus bug.)
    ///
    /// # Candidate windowing (anti-grind)
    /// When more than `MAX_LOTTERY_CANDIDATES` UTXOs are eligible, the set is
    /// not the lexicographically lowest 10k UTXO IDs (a fixed prefix that a
    /// vanity-ground low `tx_hash` could permanently occupy). Instead, the
    /// window starts at a deterministic, per-block seed-derived offset into the
    /// UTXO-id keyspace and wraps around to the start. The offset is derived
    /// from the same verifiable-randomness source the draw consumes
    /// (`prev_block_hash` + height), so the proposer and every validator derive
    /// an identical window, while the window rotates each block — grinding a
    /// low hash position no longer guarantees membership.
    ///
    /// # Arguments
    /// * `block_height` - The block height being validated (UTXOs must be
    ///   older)
    /// * `prev_block_hash` - Previous block hash; the verifiable-randomness
    ///   seed used (with height) to derive the candidate-window start offset.
    ///   MUST be the same value the draw/verification uses, or proposer and
    ///   validator diverge.
    /// * `config` - Lottery draw configuration with age/value thresholds
    ///
    /// # Returns
    /// A vector of `LotteryCandidate` for all eligible UTXOs (capped at
    /// `MAX_LOTTERY_CANDIDATES`). When at most that many are eligible, the
    /// result is exactly the full eligible set (only the order differs from a
    /// plain key-order scan; the draw re-seeds, so order is irrelevant).
    pub fn get_lottery_validation_candidates(
        &self,
        block_height: u64,
        prev_block_hash: &[u8; 32],
        config: &LotteryDrawConfig,
    ) -> Result<Vec<LotteryCandidate>, LedgerError> {
        use bth_cluster_tax::ClusterFactorCurve;
        use std::{collections::HashMap, ops::Bound};

        /// Deterministic cap on the lottery candidate set. Must be the same
        /// for proposers and validators (consensus-critical).
        const MAX_LOTTERY_CANDIDATES: usize = 10_000;

        let rtxn = self
            .env
            .read_txn()
            .map_err(|e| LedgerError::Database(format!("Failed to start read txn: {}", e)))?;

        let mut candidates = Vec::new();
        // Per-cluster global wealth cache for this scan (u128, #626)
        let mut wealth_cache: HashMap<u64, u128> = HashMap::new();
        let factor_curve = ClusterFactorCurve::default_params();

        // Seed-derived 36-byte start offset into the UTXO-id keyspace.
        let offset_key = Self::lottery_candidate_offset_key(prev_block_hash, block_height);
        let offset_slice: &[u8] = &offset_key;

        // Wraparound rotation: walk `[offset, end)` then `[start, offset)`.
        // The two half-open ranges partition the UTXO keyspace exactly once
        // (disjoint, no overlap, no double-count), so chaining them visits
        // every UTXO once, starting at the seed-derived offset and wrapping to
        // the start — a deterministic, per-block-rotating window. The cap break
        // stops collection once 10k eligible UTXOs are gathered; if the eligible
        // set is <= 10k we never break and collect them all.
        let upper = self
            .utxo_db
            .range(
                &rtxn,
                &(Bound::Included(offset_slice), Bound::<&[u8]>::Unbounded),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;
        let lower = self
            .utxo_db
            .range(
                &rtxn,
                &(Bound::<&[u8]>::Unbounded, Bound::Excluded(offset_slice)),
            )
            .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;

        for result in upper.chain(lower) {
            // CONSENSUS-CRITICAL: the candidate set (including the cap, the
            // seed-derived start offset, and the wraparound order) must be
            // identical for the block proposer and every validator, because
            // lottery verification re-runs the draw. The offset is a pure
            // function of (prev_block_hash, height); LMDB range iteration order
            // is key order, a deterministic function of the UTXO set.
            if candidates.len() >= MAX_LOTTERY_CANDIDATES {
                break;
            }

            if let Ok((_, value)) = result {
                if let Ok(utxo) = bincode::deserialize::<Utxo>(value) {
                    // Check eligibility: age and value thresholds
                    let age = block_height.saturating_sub(utxo.created_at);
                    if age >= config.min_utxo_age && utxo.output.amount >= config.min_utxo_value {
                        // Convert ClusterTagVector to TagVector for entropy calculation
                        let tag_vector =
                            Self::cluster_tags_to_tag_vector(&utxo.output.cluster_tags);

                        // Create candidate with UTXO ID (36 bytes: tx_hash || output_index)
                        let utxo_id = utxo.id.to_bytes();

                        // Effective cluster wealth for this UTXO: tag weights
                        // against global per-cluster wealth (background
                        // contributes zero), then mapped through the
                        // fixed-point factor curve. Used by ClusterWeighted
                        // winner selection; deterministic across nodes.
                        let mut weighted: u128 = 0;
                        for entry in &utxo.output.cluster_tags.entries {
                            let global = match wealth_cache.entry(entry.cluster_id.0) {
                                std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                                std::collections::hash_map::Entry::Vacant(e) => {
                                    // 16-byte LE u128 accumulator (#626). A
                                    // wrong-width value fails closed (propagates)
                                    // rather than silently reading as 0 wealth,
                                    // which would mis-tilt the consensus lottery.
                                    let w = match self
                                        .cluster_wealth_db
                                        .get(&rtxn, entry.cluster_id.0.to_le_bytes().as_slice())
                                        .map_err(|e| {
                                            LedgerError::Database(format!(
                                                "Failed to get cluster wealth: {}",
                                                e
                                            ))
                                        })? {
                                        Some(bytes) => decode_cluster_wealth(bytes)?,
                                        None => 0u128,
                                    };
                                    *e.insert(w)
                                }
                            };
                            weighted = weighted
                                .saturating_add((entry.weight as u128).saturating_mul(global));
                        }
                        // Full u128 effective wealth: `factor()` accepts u128
                        // (log-domain curve, #626 PR 1), so the lottery tilt uses
                        // the un-clamped accumulator directly.
                        let effective_wealth = weighted / TAG_WEIGHT_SCALE as u128;
                        // factor() returns FACTOR_SCALE units (1000..6000),
                        // which LotteryCandidate uses directly (integer
                        // fixed-point, consensus-deterministic)
                        let cluster_factor = factor_curve.factor(effective_wealth);

                        let candidate = LotteryCandidate::new(
                            utxo_id,
                            utxo.output.amount,
                            cluster_factor,
                            &tag_vector,
                            utxo.created_at,
                        );

                        candidates.push(candidate);
                    }
                }
            }
        }

        debug!(
            block_height = block_height,
            candidate_count = candidates.len(),
            "Found lottery validation candidates"
        );

        Ok(candidates)
    }

    /// Derive the deterministic 36-byte start offset into the UTXO-id keyspace
    /// for lottery candidate selection.
    ///
    /// UTXO keys are `tx_hash || output_index` (32 + 4 = 36 bytes). The offset
    /// rotates every block via the same verifiable-randomness source the draw
    /// consumes (`prev_block_hash` + height), so the proposer and every
    /// validator derive an identical window for the same block, while the
    /// window moves block-to-block. Using SHA-256 (the codebase's
    /// lottery-seed primitive, see `generate_seed` in `cluster-tax`) with
    /// domain separation keeps this offset independent of the draw seed yet
    /// equally deterministic.
    fn lottery_candidate_offset_key(prev_block_hash: &[u8; 32], block_height: u64) -> [u8; 36] {
        use sha2::{Digest, Sha256};

        // First 32 bytes (tx_hash slot): domain-separated hash over the lottery
        // randomness inputs.
        let mut head_hasher = Sha256::new();
        head_hasher.update(b"LOTTERY_CANDIDATE_OFFSET_V1");
        head_hasher.update(prev_block_hash);
        head_hasher.update(block_height.to_le_bytes());
        let head: [u8; 32] = head_hasher.finalize().into();

        // Remaining 4 bytes (output_index slot): a second domain-separated hash
        // so the full 36-byte offset is uniformly distributed across the
        // keyspace rather than always landing on a zero index suffix.
        let mut tail_hasher = Sha256::new();
        tail_hasher.update(b"LOTTERY_CANDIDATE_OFFSET_V1_TAIL");
        tail_hasher.update(head);
        let tail: [u8; 32] = tail_hasher.finalize().into();

        let mut key = [0u8; 36];
        key[..32].copy_from_slice(&head);
        key[32..].copy_from_slice(&tail[..4]);
        key
    }

    /// Convert ClusterTagVector (on-chain format) to TagVector (cluster-tax
    /// format).
    ///
    /// This conversion is needed for entropy calculation in lottery selection.
    fn cluster_tags_to_tag_vector(
        cluster_tags: &bth_transaction_types::ClusterTagVector,
    ) -> TagVector {
        let mut tag_vector = TagVector::new();

        for entry in &cluster_tags.entries {
            // ClusterTagVector uses u32 weights, TagVector also uses u32 weights
            tag_vector.set(bth_cluster_tax::ClusterId(entry.cluster_id.0), entry.weight);
        }

        tag_vector
    }
}

/// Information about cluster wealth for a set of UTXOs.
///
/// Used by wallets to understand their cluster profile and estimate fees.
#[derive(Debug, Clone)]
pub struct ClusterWealthInfo {
    /// Maximum cluster wealth across all provided UTXOs (picocredits).
    /// This is the value used for fee calculation (progressive fees).
    /// Widened to u128 with the accumulator (#626); can exceed u64::MAX pico.
    pub max_cluster_wealth: u128,

    /// Total value of the provided UTXOs.
    pub total_value: u64,

    /// Number of UTXOs found.
    pub utxo_count: usize,

    /// The cluster ID with the highest wealth (if any).
    pub dominant_cluster_id: Option<u64>,

    /// Breakdown of wealth by cluster ID: (cluster_id, wealth in picocredits,
    /// u128)
    pub cluster_breakdown: Vec<(u64, u128)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bth_transaction_types::ClusterTagVector;
    use tempfile::tempdir;

    // ========================================================================
    // Ring-centroid cluster-factor floor (audit cycle 6 H2, design #574 B2)
    // ========================================================================

    fn cluster_tags(cluster_id: u64) -> ClusterTagVector {
        ClusterTagVector::from_pairs(&[(
            bth_transaction_types::ClusterId(cluster_id),
            TAG_WEIGHT_SCALE,
        )])
    }

    /// Cold-start (issue #583): a fresh-genesis chain has no age-eligible
    /// outputs, so decoy gather must fail with the *typed*
    /// `LedgerError::InsufficientDecoys` variant (carrying the structured
    /// required/available counts) rather than a stringly-typed `InvalidBlock`.
    /// This is what lets the faucet RPC match the cold-start condition
    /// precisely and return a graceful "warming up" response.
    #[test]
    fn test_cold_start_decoy_gather_returns_typed_insufficient_decoys() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let mut rng = rand::thread_rng();
        let decoys_needed = 19;

        // get_decoy_outputs_for_input (used by create_private_transaction).
        let result = ledger.get_decoy_outputs_for_input(decoys_needed, &[], 10, 0, None, &mut rng);
        match result {
            Err(LedgerError::InsufficientDecoys {
                required,
                available,
            }) => {
                assert_eq!(required, decoys_needed);
                assert_eq!(available, 0, "fresh chain has no eligible decoys");
            }
            other => panic!("expected typed InsufficientDecoys, got {:?}", other),
        }

        // The sibling OSPEAD gather must map to the same typed variant.
        let result = ledger.get_decoy_outputs_ospead(decoys_needed, &[], 10, None, &mut rng);
        assert!(
            matches!(result, Err(LedgerError::InsufficientDecoys { .. })),
            "ospead gather should also surface the typed variant, got {:?}",
            result
        );
    }

    /// Attack case: the spender claims a background (1x) factor from output
    /// tags, but the ring members carry a wealthy cluster's tags. The floor
    /// must raise the demurrage factor to the ring-implied value.
    #[test]
    fn test_ring_centroid_floor_raises_understated_factor() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Seed a wealthy cluster (picocredits: 10M BTH).
        const W: u128 = 10_000_000_000_000_000_000; // 10M BTH in pico
        ledger.set_cluster_wealth_for_test(1, W).unwrap();

        let curve = bth_cluster_tax::ClusterFactorCurve::default_params();
        let wealthy = cluster_tags(1);
        let ring_members: Vec<(u64, &ClusterTagVector)> =
            vec![(1_000_000, &wealthy), (1_000_000, &wealthy)];

        let claimed_factor = 1_000; // 1x background claim
        let floored = ledger
            .ring_centroid_floored_factor(claimed_factor, &ring_members, &curve)
            .unwrap();

        assert!(
            floored > claimed_factor,
            "floor must raise the understated factor: {floored} > {claimed_factor}"
        );
        // Matches the factor the wealthy centroid implies directly.
        assert_eq!(floored, curve.factor(W));
    }

    /// Legitimate background spend: the ring is also background (no seeded
    /// cluster wealth), so the floor leaves the 1x claim unchanged.
    #[test]
    fn test_ring_centroid_floor_leaves_background_unchanged() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let curve = bth_cluster_tax::ClusterFactorCurve::default_params();
        let bg = ClusterTagVector::empty();
        let ring_members: Vec<(u64, &ClusterTagVector)> = vec![(1_000_000, &bg), (1_000_000, &bg)];

        let claimed_factor = 1_000; // 1x background
        let floored = ledger
            .ring_centroid_floored_factor(claimed_factor, &ring_members, &curve)
            .unwrap();

        assert_eq!(
            floored, claimed_factor,
            "background spend must be unaffected"
        );
    }

    /// The floor can only raise: a claim already above the ring-implied factor
    /// is preserved.
    #[test]
    fn test_ring_centroid_floor_never_lowers_claim() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let curve = bth_cluster_tax::ClusterFactorCurve::default_params();
        let bg = ClusterTagVector::empty();
        let ring_members: Vec<(u64, &ClusterTagVector)> = vec![(1_000_000, &bg)];

        let claimed_factor = 6_000; // 6x
        let floored = ledger
            .ring_centroid_floored_factor(claimed_factor, &ring_members, &curve)
            .unwrap();

        assert_eq!(floored, 6_000);
    }

    /// Regression (#626 PR3, Judge blocker): the C7 consensus fee-floor path
    /// resolves per-member wealth as `weight × global_wealth /
    /// TAG_WEIGHT_SCALE`. `global_wealth` is the unbounded, monotonic PR2
    /// accumulator, so the product is not provably below `u128::MAX`. With
    /// an unchecked `*` this path panics in a debug build (and wraps to a
    /// WRONG, LOWER floor in release — the non-conservative direction) at
    /// extreme wealth. The `saturating_mul` fix must instead saturate to
    /// `u128::MAX`, which the curve maps deterministically to the max,
    /// most-conservative factor 6000.
    #[test]
    fn test_ring_centroid_floor_saturates_at_max_wealth_no_panic() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Extreme accumulator value: the unbounded PR2 wealth at its ceiling.
        ledger.set_cluster_wealth_for_test(1, u128::MAX).unwrap();

        let curve = bth_cluster_tax::ClusterFactorCurve::default_params();
        let wealthy = cluster_tags(1); // full-weight tag on the saturated cluster
        let ring_members: Vec<(u64, &ClusterTagVector)> =
            vec![(1_000_000, &wealthy), (1_000_000, &wealthy)];

        let claimed_factor = 1_000; // 1x background claim
                                    // Must NOT panic (debug) nor wrap (release): the `weight × global_wealth`
                                    // multiply saturates before the divide.
        let floored = ledger
            .ring_centroid_floored_factor(claimed_factor, &ring_members, &curve)
            .unwrap();

        // Saturated wealth → max floor, the conservative direction.
        assert_eq!(
            floored,
            curve.factor(u128::MAX),
            "saturated wealth must map to the max, most-conservative factor"
        );
        assert!(floored >= claimed_factor);
    }

    /// Issue #451 (Test B, apply side): the deterministic backstop must flag a
    /// block containing a transfer tx that is too old relative to the block's
    /// OWN height, and must accept one that is exactly on the boundary. The
    /// check is purely a function of (created_at_height, block.height()), so it
    /// is identical on every node applying block N — no tip-dependence, no
    /// fork.
    #[test]
    fn test_first_stale_transfer_tx_backstop() {
        use crate::{
            consensus::{BlockBuilder, MAX_TX_AGE},
            transaction::{Transaction, TxInputs},
        };

        let block_height = 500u64;

        let mk_tx = |created_at_height: u64| Transaction {
            inputs: TxInputs::new(vec![]),
            outputs: vec![],
            fee: 0,
            created_at_height,
            settlement: None,
        };

        // mock_minting_tx-equivalent: build_direct sets header.height from the
        // minting tx's block_height.
        let minting = crate::block::MintingTx {
            block_height,
            reward: 0,
            minter_view_key: [1u8; 32],
            minter_spend_key: [2u8; 32],
            target_key: [3u8; 32],
            public_key: [4u8; 32],
            kem_ciphertext: None,
            prev_block_hash: [0u8; 32],
            difficulty: 1000,
            nonce: 0,
            timestamp: 1000,
        };

        // All fresh: boundary tx (created_at_height + MAX_TX_AGE == height) is
        // NOT stale.
        let fresh_block = BlockBuilder::build_direct(
            minting.clone(),
            vec![mk_tx(block_height - MAX_TX_AGE), mk_tx(block_height)],
        );
        assert_eq!(fresh_block.height(), block_height);
        assert_eq!(
            first_stale_transfer_tx(&fresh_block),
            None,
            "boundary/fresh transfer txs must not be flagged stale"
        );

        // Contains a stale tx (created_at_height + MAX_TX_AGE < height) at idx 1.
        let stale_block = BlockBuilder::build_direct(
            minting,
            vec![
                mk_tx(block_height - MAX_TX_AGE),
                mk_tx(block_height - MAX_TX_AGE - 1),
            ],
        );
        assert_eq!(
            first_stale_transfer_tx(&stale_block),
            Some(1),
            "a transfer tx older than MAX_TX_AGE relative to block height must be flagged"
        );
    }

    #[test]
    fn test_ledger_open_and_genesis() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let state = ledger.get_chain_state().unwrap();
        assert_eq!(state.height, 0);

        let genesis = ledger.get_block(0).unwrap();
        assert_eq!(genesis.height(), 0);
    }

    #[test]
    fn test_ledger_tip() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let tip = ledger.get_tip().unwrap();
        assert_eq!(tip.height(), 0);
    }

    /// #333: `META_TOTAL_MINED` / `META_FEES_BURNED` persist as 16-byte LE
    /// (u128). Values above u64::MAX must survive a write + ledger reopen
    /// without truncation. With the old 8-byte u64 encoding this would have
    /// been impossible to even store.
    #[test]
    fn test_supply_metadata_u128_persist_reload() {
        let dir = tempdir().unwrap();

        // ~1.22e21 picocredits gross emission, above u64::MAX (~1.84e19).
        let big_mined: u128 = 1_220_000_000_000_000_000_000;
        let big_burned: u128 = u64::MAX as u128 + 7;
        assert!(big_mined > u64::MAX as u128);
        assert!(big_burned > u64::MAX as u128);

        {
            let ledger = Ledger::open(dir.path()).unwrap();
            let mut wtxn = ledger.env.write_txn().unwrap();
            ledger
                .meta_db
                .put(&mut wtxn, META_TOTAL_MINED, &big_mined.to_le_bytes())
                .unwrap();
            ledger
                .meta_db
                .put(&mut wtxn, META_FEES_BURNED, &big_burned.to_le_bytes())
                .unwrap();
            wtxn.commit().unwrap();

            // 16-byte LE on disk (consensus-state format change, 8 -> 16).
            let rtxn = ledger.env.read_txn().unwrap();
            assert_eq!(
                ledger
                    .meta_db
                    .get(&rtxn, META_TOTAL_MINED)
                    .unwrap()
                    .unwrap()
                    .len(),
                16
            );
        }

        // Reopen and confirm exact reload through get_chain_state().
        let ledger = Ledger::open(dir.path()).unwrap();
        let state = ledger.get_chain_state().unwrap();
        assert_eq!(state.total_mined, big_mined);
        assert_eq!(state.total_fees_burned, big_burned);
    }

    /// #333: amounts stayed u64, so block wire format is byte-for-byte
    /// unchanged. Assert the genesis block (a fixed fixture) serializes to a
    /// stable byte length and that the monetary fields are u64-sized (8 bytes
    /// each), guarding against an accidental amount-widening that would break
    /// gossip/block compatibility.
    #[test]
    fn test_block_wire_format_amounts_stay_u64() {
        let genesis = crate::block::Block::genesis();
        let bytes = bincode::serialize(&genesis).unwrap();

        // Deterministic fixture: re-serializing yields identical bytes.
        assert_eq!(bytes, bincode::serialize(&genesis).unwrap());

        // Monetary fields are u64 (8 bytes), not u128 (16 bytes).
        assert_eq!(genesis.minting_tx.reward.to_le_bytes().len(), 8);
        assert_eq!(
            genesis.minting_tx.to_tx_output().amount.to_le_bytes().len(),
            8
        );
        for tx in &genesis.transactions {
            assert_eq!(tx.fee.to_le_bytes().len(), 8);
            for output in &tx.outputs {
                assert_eq!(output.amount.to_le_bytes().len(), 8);
            }
        }
    }

    #[test]
    fn test_key_image_tracking() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let key_image: [u8; 32] = [0xAB; 32];

        // Key image should not be spent initially
        assert!(ledger.is_key_image_spent(&key_image).unwrap().is_none());

        // Record key image as spent at height 10
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            ledger.record_key_image(&mut wtxn, &key_image, 10).unwrap();
            wtxn.commit().unwrap();
        }

        // Now it should be spent
        let spent_height = ledger.is_key_image_spent(&key_image).unwrap();
        assert_eq!(spent_height, Some(10));
    }

    #[test]
    fn test_key_image_double_spend_rejected() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let key_image: [u8; 32] = [0xCD; 32];

        // Record first spend
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            ledger.record_key_image(&mut wtxn, &key_image, 5).unwrap();
            wtxn.commit().unwrap();
        }

        // Try to record same key image again - should fail
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            let result = ledger.record_key_image(&mut wtxn, &key_image, 10);
            assert!(result.is_err());
        }
    }

    /// Build a minimal one-input transaction carrying `key_image`. The CLSAG
    /// signature/ring are bogus, but `verify_transaction` checks the key-image
    /// double-spend set BEFORE verifying ring signatures, so these tests only
    /// exercise (and only need) the double-spend branch.
    fn tx_with_key_image(key_image: [u8; 32]) -> BothoTransaction {
        use crate::transaction::ClsagRingInput;
        let input = ClsagRingInput {
            ring: vec![RingMember {
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                commitment: [0u8; 32],
            }],
            key_image,
            commitment_key_image: [0u8; 32],
            clsag_signature: Vec::new(),
            pseudo_output_amount: 0,
        };
        BothoTransaction::new(vec![input], vec![], 0, 0)
    }

    /// M7 fail-closed: when the consensus double-spend lookup hits a DB error,
    /// `verify_transaction` must PROPAGATE the error (as a node-local
    /// `LedgerError::Database`), not silently allow the transaction. Previously
    /// the `if let Ok(Some(..))` pattern swallowed the error and skipped the
    /// check, letting a double-spend pass on a transient DB failure
    /// (fail-open).
    ///
    /// The DB error is injected by exhausting the LMDB reader table: the ledger
    /// is opened with a single reader slot, that slot is held by a live read
    /// txn, so the read txn inside `is_key_image_spent` fails with
    /// `MdbError::ReadersFull`.
    #[test]
    fn test_verify_transaction_propagates_db_error_fail_closed() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open_single_reader(dir.path()).unwrap();

        // Hold the only reader slot open so the next read_txn() fails.
        let _held = ledger.env.read_txn().unwrap();

        // Sanity: the lookup itself errors when the reader table is exhausted.
        let probe = ledger.is_key_image_spent(&[0x11; 32]);
        assert!(
            matches!(probe, Err(LedgerError::Database(_))),
            "expected reader-table exhaustion to surface a Database error, got {:?}",
            probe
        );

        let tx = tx_with_key_image([0x22; 32]);
        let result = ledger.verify_transaction(&tx);

        // Must FAIL CLOSED: propagate the DB error, NOT return Ok (allow) and
        // NOT brand the block invalid (which would risk a node-local fork).
        match result {
            Err(LedgerError::Database(_)) => {}
            other => panic!(
                "verify_transaction must propagate a DB error (fail closed), got {:?}",
                other
            ),
        }
    }

    /// Happy path (no DB error) must be unchanged: a transaction whose key
    /// image is genuinely recorded as spent is rejected with
    /// `InvalidBlock`.
    #[test]
    fn test_verify_transaction_rejects_truly_spent_key_image() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let key_image = [0x33; 32];
        ledger.record_key_image_for_test(&key_image, 7).unwrap();

        let tx = tx_with_key_image(key_image);
        let result = ledger.verify_transaction(&tx);
        match result {
            Err(LedgerError::InvalidBlock(msg)) => {
                assert!(msg.contains("already spent"), "unexpected message: {}", msg);
            }
            other => panic!(
                "expected InvalidBlock for a truly-spent key image, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_get_utxo_by_target_key() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Create a test UTXO
        let target_key: [u8; 32] = [0x42; 32];
        let utxo_id = UtxoId::new([0x11; 32], 0);
        let output = TxOutput {
            amount: 1_000_000,
            target_key,
            public_key: [0x33; 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
            kem_ciphertext: None,
        };
        let utxo = Utxo {
            id: utxo_id,
            output,
            created_at: 1,
        };

        // Store the UTXO
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            let utxo_bytes = bincode::serialize(&utxo).unwrap();
            ledger
                .utxo_db
                .put(&mut wtxn, &utxo_id.to_bytes(), &utxo_bytes)
                .unwrap();
            ledger.add_to_address_index(&mut wtxn, &utxo).unwrap();
            wtxn.commit().unwrap();
        }

        // Look up by target_key
        let found = ledger.get_utxo_by_target_key(&target_key).unwrap();
        assert!(found.is_some());
        let found_utxo = found.unwrap();
        assert_eq!(found_utxo.output.amount, 1_000_000);
        assert_eq!(found_utxo.output.target_key, target_key);
    }

    #[test]
    fn test_cluster_wealth_for_utxos_uses_global_wealth() {
        use bth_transaction_types::ClusterId;

        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // A whale UTXO and a small UTXO, both 100% tagged to cluster 1
        let whale_amount = 100_000_000u64;
        let small_amount = 1_000u64;
        let small_target_key: [u8; 32] = [0x77; 32];

        let whale_utxo = Utxo {
            id: UtxoId::new([0x11; 32], 0),
            output: TxOutput {
                amount: whale_amount,
                target_key: [0x42; 32],
                public_key: [0x33; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::single(ClusterId(1)),
                kem_ciphertext: None,
            },
            created_at: 1,
        };
        let small_utxo = Utxo {
            id: UtxoId::new([0x22; 32], 0),
            output: TxOutput {
                amount: small_amount,
                target_key: small_target_key,
                public_key: [0x44; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::single(ClusterId(1)),
                kem_ciphertext: None,
            },
            created_at: 1,
        };

        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            for utxo in [&whale_utxo, &small_utxo] {
                let bytes = bincode::serialize(utxo).unwrap();
                ledger
                    .utxo_db
                    .put(&mut wtxn, &utxo.id.to_bytes(), &bytes)
                    .unwrap();
                ledger.add_to_address_index(&mut wtxn, utxo).unwrap();
            }
            wtxn.commit().unwrap();
        }
        ledger.rebuild_cluster_wealth_index().unwrap();

        // Estimating with only the SMALL UTXO must report the cluster's
        // GLOBAL wealth, not the UTXO's own value — otherwise wallets would
        // under-estimate fees relative to mempool enforcement.
        let info = ledger
            .compute_cluster_wealth_for_utxos(&[small_target_key])
            .unwrap();
        assert_eq!(info.utxo_count, 1);
        assert_eq!(info.total_value, small_amount);
        assert_eq!(
            info.max_cluster_wealth,
            (whale_amount + small_amount) as u128
        );
        assert_eq!(info.dominant_cluster_id, Some(1));
    }

    /// Full-weight tag => contribution == output.amount.
    #[cfg(test)]
    fn full_tags(cluster: u64) -> ClusterTagVector {
        use bth_transaction_types::ClusterId;
        ClusterTagVector::from_pairs(&[(ClusterId(cluster), TAG_WEIGHT_SCALE)])
    }

    #[cfg(test)]
    fn tagged_output(amount: u64, tk: u8, cluster: u64) -> TxOutput {
        TxOutput {
            amount,
            target_key: [tk; 32],
            public_key: [tk ^ 0xA0; 32],
            e_memo: None,
            cluster_tags: full_tags(cluster),
            kem_ciphertext: None,
        }
    }

    /// Determinism parity (#604/#607, u128 edition, #626): a node that
    /// *rebuilds* the cluster wealth index from the UTXO set must produce
    /// byte-identical `cluster_wealth_db` contents (now 16-byte LE u128) to a
    /// node that *incrementally* accumulated it via
    /// `update_cluster_wealth_for_output`. This includes the case the u128
    /// widening was designed for: a single cluster whose contributions sum
    /// **past `u64::MAX`** — under the old u64 accumulator this saturated at
    /// `u64::MAX`; at u128 it must record the EXACT sum, identically on both
    /// paths. Any accumulation-order or width bug surfaces as a divergence
    /// (a consensus fork caught in CI, not on the testnet).
    #[test]
    fn test_rebuild_matches_incremental_cluster_wealth_past_u64_max() {
        // Cluster 1: three huge contributions whose sum exceeds u64::MAX but
        // fits comfortably in u128 (each ~0.75·u64::MAX; sum ~2.25·u64::MAX).
        // Cluster 2: small, exact sum, confirms no behavior change below range.
        let huge = (u64::MAX / 4) * 3; // ~0.75 * u64::MAX, fits a u64 output.amount
        let expected_c1 = huge as u128 * 3; // exact, > u64::MAX, no saturation
        assert!(
            expected_c1 > u64::MAX as u128,
            "test fixture must exceed the former u64 ceiling"
        );

        let outputs: Vec<TxOutput> = vec![
            tagged_output(huge, 0x01, 1),
            tagged_output(huge, 0x02, 1),
            tagged_output(huge, 0x03, 1),
            tagged_output(1_000, 0x04, 2),
            tagged_output(2_500, 0x05, 2),
        ];

        // ---- Incremental ledger: drive update_cluster_wealth_for_output ----
        let inc_dir = tempdir().unwrap();
        let inc_ledger = Ledger::open(inc_dir.path()).unwrap();
        {
            let mut wtxn = inc_ledger.env.write_txn().unwrap();
            for output in &outputs {
                inc_ledger
                    .update_cluster_wealth_for_output(&mut wtxn, output)
                    .unwrap();
            }
            wtxn.commit().unwrap();
        }

        // ---- Rebuild ledger: populate utxo_db, then rebuild from scratch ----
        let rb_dir = tempdir().unwrap();
        let rb_ledger = Ledger::open(rb_dir.path()).unwrap();
        {
            let mut wtxn = rb_ledger.env.write_txn().unwrap();
            for (i, output) in outputs.iter().enumerate() {
                let utxo = Utxo {
                    id: UtxoId::new([i as u8; 32], 0),
                    output: output.clone(),
                    created_at: 1,
                };
                let bytes = bincode::serialize(&utxo).unwrap();
                rb_ledger
                    .utxo_db
                    .put(&mut wtxn, &utxo.id.to_bytes(), &bytes)
                    .unwrap();
            }
            wtxn.commit().unwrap();
        }
        rb_ledger.rebuild_cluster_wealth_index().unwrap();

        // ---- Compare full cluster_wealth_db contents (sorted for stability) ----
        let mut inc = inc_ledger.get_all_cluster_wealth().unwrap();
        let mut rb = rb_ledger.get_all_cluster_wealth().unwrap();
        inc.sort_unstable();
        rb.sort_unstable();
        assert_eq!(
            inc, rb,
            "rebuild path must produce byte-identical cluster wealth to the incremental path"
        );

        // Cluster 1 must be the EXACT sum (> u64::MAX), not saturated.
        assert_eq!(inc_ledger.get_cluster_wealth(1).unwrap(), expected_c1);
        assert_eq!(rb_ledger.get_cluster_wealth(1).unwrap(), expected_c1);

        // Cluster 2 (small) must be the exact sum on both paths.
        assert_eq!(inc_ledger.get_cluster_wealth(2).unwrap(), 3_500);
        assert_eq!(rb_ledger.get_cluster_wealth(2).unwrap(), 3_500);
    }

    /// Serialization contract pin (#626 determinism section): a
    /// `cluster_wealth_db` value is exactly 16 bytes, little-endian `u128`.
    /// Reading back the raw DB bytes must equal `wealth.to_le_bytes()` and
    /// `get_cluster_wealth` must decode it losslessly, including values that
    /// exceed the former u64 ceiling.
    #[test]
    fn test_cluster_wealth_serialization_is_16_byte_le_u128() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // A value beyond u64::MAX to exercise the widened width.
        let wealth: u128 = (u64::MAX as u128) + 123_456_789;
        ledger.set_cluster_wealth_for_test(42, wealth).unwrap();

        // Raw bytes: exactly 16, little-endian.
        let rtxn = ledger.env.read_txn().unwrap();
        let raw = ledger
            .cluster_wealth_db
            .get(&rtxn, 42u64.to_le_bytes().as_slice())
            .unwrap()
            .expect("cluster 42 must be present");
        assert_eq!(raw.len(), CLUSTER_WEALTH_LEN, "value must be 16 bytes");
        assert_eq!(
            raw,
            wealth.to_le_bytes(),
            "value must be little-endian u128"
        );
        drop(rtxn);

        // Lossless decode through the public accessor.
        assert_eq!(ledger.get_cluster_wealth(42).unwrap(), wealth);
    }

    /// Reject-legacy contract (#626): a legacy 8-byte (`u64`) value MUST fail
    /// closed on read rather than being silently reinterpreted. Fresh genesis
    /// (protocol 4.0.0) is assumed, so a wrong-width value is a hard error.
    #[test]
    fn test_cluster_wealth_rejects_legacy_8_byte_value() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Inject an 8-byte legacy value directly into the DB.
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            let legacy: u64 = 1_000_000;
            ledger
                .cluster_wealth_db
                .put(
                    &mut wtxn,
                    7u64.to_le_bytes().as_slice(),
                    &legacy.to_le_bytes(),
                )
                .unwrap();
            wtxn.commit().unwrap();
        }

        // Every reader must reject it (fail closed), not read a truncated value.
        let single = ledger.get_cluster_wealth(7);
        assert!(
            matches!(single, Err(LedgerError::Database(_))),
            "8-byte legacy value must be rejected, got {single:?}"
        );
        let all = ledger.get_all_cluster_wealth();
        assert!(
            matches!(all, Err(LedgerError::Database(_))),
            "get_all_cluster_wealth must reject an 8-byte legacy value, got {all:?}"
        );
    }

    /// Saturation discipline at the u128 boundary (#604/#607 lesson kept at the
    /// new width): although astronomically unreachable via real outputs (each
    /// contribution is a u64 amount), the incremental path must still
    /// `saturating_add` at `u128::MAX` rather than wrap. Seed a cluster just
    /// below the ceiling and add a contribution that would overflow.
    #[test]
    fn test_incremental_saturates_at_u128_max() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Seed near the ceiling, leaving headroom smaller than the next
        // contribution.
        ledger
            .set_cluster_wealth_for_test(3, u128::MAX - 10)
            .unwrap();

        // A full-weight output of amount 1000 => contribution 1000 > 10 headroom.
        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            ledger
                .update_cluster_wealth_for_output(&mut wtxn, &tagged_output(1_000, 0x0B, 3))
                .unwrap();
            wtxn.commit().unwrap();
        }

        assert_eq!(
            ledger.get_cluster_wealth(3).unwrap(),
            u128::MAX,
            "must saturate at u128::MAX, not wrap"
        );
    }

    #[test]
    fn test_get_utxo_by_target_key_not_found() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let nonexistent_key: [u8; 32] = [0xFF; 32];
        let result = ledger.get_utxo_by_target_key(&nonexistent_key).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_utxo_exists() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let utxo_id = UtxoId::new([0xDE; 32], 5);

        // Should not exist initially
        assert!(!ledger.utxo_exists(&utxo_id).unwrap());

        // Create and store UTXO
        let utxo = Utxo {
            id: utxo_id,
            output: TxOutput {
                amount: 500,
                target_key: [0x11; 32],
                public_key: [0x22; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
                kem_ciphertext: None,
            },
            created_at: 0,
        };

        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            let utxo_bytes = bincode::serialize(&utxo).unwrap();
            ledger
                .utxo_db
                .put(&mut wtxn, &utxo_id.to_bytes(), &utxo_bytes)
                .unwrap();
            wtxn.commit().unwrap();
        }

        // Now should exist
        assert!(ledger.utxo_exists(&utxo_id).unwrap());
    }

    #[test]
    fn test_get_utxo() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let utxo_id = UtxoId::new([0xAA; 32], 0);
        let amount = 12345u64;

        // Store UTXO
        let utxo = Utxo {
            id: utxo_id,
            output: TxOutput {
                amount,
                target_key: [0xBB; 32],
                public_key: [0xCC; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
                kem_ciphertext: None,
            },
            created_at: 100,
        };

        {
            let mut wtxn = ledger.env.write_txn().unwrap();
            let utxo_bytes = bincode::serialize(&utxo).unwrap();
            ledger
                .utxo_db
                .put(&mut wtxn, &utxo_id.to_bytes(), &utxo_bytes)
                .unwrap();
            wtxn.commit().unwrap();
        }

        // Retrieve and verify
        let retrieved = ledger.get_utxo(&utxo_id).unwrap().unwrap();
        assert_eq!(retrieved.output.amount, amount);
        assert_eq!(retrieved.created_at, 100);
    }

    #[test]
    fn test_set_difficulty() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let new_difficulty = 0x00FF_FFFF_0000_0000u64;
        ledger.set_difficulty(new_difficulty).unwrap();

        let state = ledger.get_chain_state().unwrap();
        assert_eq!(state.difficulty, new_difficulty);
    }

    // ------------------------------------------------------------------
    // Fuzz-harness wiring sanity check (issue #337, fuzz_add_block).
    //
    // NOT a substitute for the libfuzzer run (CI-deferred: cargo-fuzz cannot
    // run on the macOS dev host). Confirms the harness wiring: a fresh
    // genesis ledger satisfies supply conservation, and feeding a malformed
    // block to `add_block` is rejected (typed LedgerError) without panicking,
    // leaving the conserved state unchanged — the exact pre/post check the
    // fuzz target performs.
    // ------------------------------------------------------------------
    // #599: the block-fee accumulation in `add_block_inner` must reject a
    // fee-sum overflow with a typed error rather than wrapping silently
    // (release) or panicking (debug). `checked_block_fees` is the exact
    // reduction `add_block_inner` invokes; this unit test pins the reduction
    // directly, and `test_add_block_rejects_fee_overflow_block` below drives
    // a crafted block through the real `add_block` path (#663). This test is
    // meaningful in BOTH build modes: the old `.sum()` panicked here in debug
    // and wrapped to 1 in release.
    #[test]
    fn test_checked_block_fees_rejects_overflow() {
        use crate::transaction::Transaction;

        let mut block = Block::genesis_for_network(Network::Testnet);
        // Two fees whose sum exceeds u64::MAX.
        block.transactions = vec![
            Transaction::new_stub_with_fee(u64::MAX),
            Transaction::new_stub_with_fee(3),
        ];

        let result = checked_block_fees(&block);
        assert!(
            matches!(result, Err(LedgerError::FeeOverflow)),
            "fee-sum overflow must be rejected with LedgerError::FeeOverflow, got {:?}",
            result
        );
    }

    // #599: a non-overflowing fee total is still summed correctly.
    #[test]
    fn test_checked_block_fees_normal_sum() {
        use crate::transaction::Transaction;

        let mut block = Block::genesis_for_network(Network::Testnet);
        block.transactions = vec![
            Transaction::new_stub_with_fee(100),
            Transaction::new_stub_with_fee(250),
            Transaction::new_stub_with_fee(650),
        ];

        assert_eq!(checked_block_fees(&block).unwrap(), 1000);
    }

    // #663 (overflow-checks in release): integration-level companion to
    // `test_checked_block_fees_rejects_overflow`. A crafted gossiped block
    // whose per-tx fees sum past `u64::MAX` must be rejected by the REAL
    // block-acceptance path (`add_block`) with a clean typed
    // `Err(LedgerError::FeeOverflow)` — never a node panic. With
    // `overflow-checks = true` now set on the release profile, an unguarded
    // `.sum()` on this path would abort the node instead of wrapping, so this
    // test (run under `cargo test --release` in CI-equivalent verification)
    // proves the guard fires before any unchecked arithmetic can.
    //
    // The block is honest on every gate that precedes the fee-sum guard
    // (height, prev hash, minting-tx consistency, expected difficulty, PoW,
    // expected reward, timestamps, tx_root) so the rejection observed is the
    // fee-overflow rejection specifically, not an earlier structural one.
    #[test]
    fn test_add_block_rejects_fee_overflow_block() {
        use crate::{block::calculate_block_reward, transaction::Transaction};
        use bth_account_keys::AccountKey;
        use rand::{rngs::StdRng, SeedableRng};

        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Make PoW trivially satisfiable so the crafted block reaches the
        // fee-sum guard instead of stopping at the PoW gate.
        ledger.set_difficulty(u64::MAX).unwrap();
        let state = ledger.get_chain_state().unwrap();
        let genesis = ledger.get_block(0).unwrap();

        let mut rng = StdRng::seed_from_u64(663);
        let minter = AccountKey::random(&mut rng);

        // Two fees whose sum exceeds u64::MAX.
        let txs = vec![
            Transaction::new_stub_with_fee(u64::MAX),
            Transaction::new_stub_with_fee(3),
        ];
        let block = Block::new_template_with_txs(
            &genesis,
            &minter.default_subaddress(),
            state.difficulty,
            calculate_block_reward(1, state.total_mined),
            txs,
        );

        let result = ledger.add_block(&block);
        assert!(
            matches!(result, Err(LedgerError::FeeOverflow)),
            "overflow-fee block must be rejected via Err(LedgerError::FeeOverflow), got {:?}",
            result
        );

        // The rejected block must not have advanced the chain.
        assert_eq!(ledger.get_chain_state().unwrap().height, 0);
    }

    #[test]
    fn fuzz_wiring_add_block_rejects_malformed_and_conserves_supply() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Supply conservation must hold at genesis:
        // total_mined == Σ(UTXO values) + total_fees_burned + lottery_pool.
        let conserved = |ledger: &Ledger| -> u64 {
            let state = ledger.get_chain_state().unwrap();
            let pool = ledger.get_lottery_pool().unwrap();
            let utxo_sum: u128 = ledger
                .create_snapshot()
                .unwrap()
                .get_utxos()
                .unwrap()
                .iter()
                .map(|u| u.output.amount as u128)
                .sum();
            assert_eq!(
                state.total_mined,
                utxo_sum + state.total_fees_burned + pool,
                "supply conservation violated"
            );
            state.height
        };
        let prev_height = conserved(&ledger);

        // A malformed block (wrong height) must be rejected with a typed error
        // and must NOT advance the chain or break conservation.
        let mut bad = Block::genesis_for_network(Network::Testnet);
        bad.header.height = 999; // not prev_height + 1
        let result = ledger.add_block(&bad);
        assert!(
            matches!(result, Err(LedgerError::InvalidBlock(_))),
            "malformed block must be rejected with a typed LedgerError, got {:?}",
            result
        );

        // Post-state unchanged and still conserved.
        let post_height = conserved(&ledger);
        assert_eq!(
            prev_height, post_height,
            "rejected block must not advance height"
        );
    }

    // H3 (#558): block + emission state must be crash-atomic — written in a
    // SINGLE write txn that commits together or not at all.
    //
    // This asserts the rollback half of that guarantee: when the block is
    // rejected, the emission/difficulty writes that were staged into the SAME
    // `wtxn` are discarded with it. If emission used a separate commit (the old
    // two-txn path), the difficulty/epoch state would have advanced even though
    // the block was rejected — exactly the divergence H3 fixes. We feed a
    // malformed block (wrong height, fails the first validation gate) together
    // with an emission update whose values differ from the persisted ones, and
    // confirm NONE of the emission fields changed.
    #[test]
    fn test_add_block_with_emission_is_atomic_on_reject() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Baseline emission/difficulty state at genesis.
        let before = ledger.get_chain_state().unwrap();

        // An emission update that is DISTINCT from the persisted state in every
        // field, so any leaked write would be observable.
        let emission = EmissionStateUpdate {
            difficulty: before.difficulty.wrapping_add(123_456),
            total_tx: before.total_tx + 7,
            epoch_tx: before.epoch_tx + 11,
            epoch_emission: before.epoch_emission + 13,
            epoch_burns: before.epoch_burns + 17,
            current_reward: before.current_reward.wrapping_add(999),
        };
        // Sanity: the update really would change state if it landed.
        assert_ne!(emission.difficulty, before.difficulty);

        // Malformed block: wrong height is rejected before the wtxn ever opens,
        // but the contract we care about is that emission state is untouched.
        let mut bad = Block::genesis_for_network(Network::Testnet);
        bad.header.height = 999; // not before.height + 1

        let result = ledger.add_block_with_emission(&bad, emission);
        assert!(
            matches!(result, Err(LedgerError::InvalidBlock(_))),
            "malformed block must be rejected with a typed LedgerError, got {:?}",
            result
        );

        // No block was added AND no emission field moved: the pair is atomic.
        let after = ledger.get_chain_state().unwrap();
        assert_eq!(
            after.height, before.height,
            "rejected block advanced height"
        );
        assert_eq!(
            after.difficulty, before.difficulty,
            "emission difficulty leaked despite block rejection (non-atomic write)"
        );
        assert_eq!(after.total_tx, before.total_tx, "total_tx leaked");
        assert_eq!(after.epoch_tx, before.epoch_tx, "epoch_tx leaked");
        assert_eq!(
            after.epoch_emission, before.epoch_emission,
            "epoch_emission leaked"
        );
        assert_eq!(after.epoch_burns, before.epoch_burns, "epoch_burns leaked");
        assert_eq!(
            after.current_reward, before.current_reward,
            "current_reward leaked"
        );
    }

    // H3 (#558): the folded emission writes in `add_block_with_emission` must
    // use the SAME key encoding as the standalone `update_emission_state`, so a
    // node that takes the atomic path persists byte-identical chain state to one
    // that took the legacy two-commit path. We exercise the encoding via
    // `update_emission_state` (which `add_block_inner` mirrors verbatim) and
    // confirm every field round-trips through `get_chain_state` and a reopen.
    #[test]
    fn test_emission_state_encoding_roundtrip() {
        let dir = tempdir().unwrap();
        let values = EmissionStateUpdate {
            difficulty: 0x0123_4567_89ab_cdef,
            total_tx: 42,
            epoch_tx: 500,
            epoch_emission: 1_000_000,
            epoch_burns: 250_000,
            current_reward: 49_000_000_000_000,
        };

        {
            let ledger = Ledger::open(dir.path()).unwrap();
            ledger
                .update_emission_state(
                    values.difficulty,
                    values.total_tx,
                    values.epoch_tx,
                    values.epoch_emission,
                    values.epoch_burns,
                    values.current_reward,
                )
                .unwrap();

            let state = ledger.get_chain_state().unwrap();
            assert_eq!(state.difficulty, values.difficulty);
            assert_eq!(state.total_tx, values.total_tx);
            assert_eq!(state.epoch_tx, values.epoch_tx);
            assert_eq!(state.epoch_emission, values.epoch_emission);
            assert_eq!(state.epoch_burns, values.epoch_burns);
            assert_eq!(state.current_reward, values.current_reward);
        }

        // Survives a reopen from disk (same LE encoding the folded path uses).
        let reopened = Ledger::open(dir.path()).unwrap();
        let state = reopened.get_chain_state().unwrap();
        assert_eq!(state.difficulty, values.difficulty);
        assert_eq!(state.current_reward, values.current_reward);
    }

    // The cumulative lottery carryover persists as 16-byte LE u128. A value
    // above u64::MAX must round-trip exactly through META_LOTTERY_POOL — this
    // is the on-disk half of the u64->u128 widening (the saturation fix).
    #[test]
    fn test_lottery_pool_u128_persist_reload_roundtrip() {
        let dir = tempdir().unwrap();

        // A balance that no longer fits in u64 (would have saturated before).
        let big_pool: u128 = (u64::MAX as u128) + 1_234_567;

        {
            let ledger = Ledger::open(dir.path()).unwrap();
            let mut wtxn = ledger.env.write_txn().unwrap();
            ledger
                .meta_db
                .put(&mut wtxn, META_LOTTERY_POOL, &big_pool.to_le_bytes())
                .unwrap();
            wtxn.commit().unwrap();

            // Same handle reads it back exactly (16-byte LE decode).
            assert_eq!(ledger.get_lottery_pool().unwrap(), big_pool);
        }

        // Reopen from disk: value survives a reload with no truncation.
        let reopened = Ledger::open(dir.path()).unwrap();
        assert_eq!(reopened.get_lottery_pool().unwrap(), big_pool);

        // A fresh ledger (missing key) reads as zero.
        let fresh_dir = tempdir().unwrap();
        let fresh = Ledger::open(fresh_dir.path()).unwrap();
        assert_eq!(fresh.get_lottery_pool().unwrap(), 0u128);
    }

    // ----------------------------------------------------------------------
    // Lottery candidate windowing (issue #572): seed-derived wraparound
    // ----------------------------------------------------------------------

    /// Mirror of the private `MAX_LOTTERY_CANDIDATES` cap inside
    /// `get_lottery_validation_candidates`. Kept in sync by hand; the tests
    /// below probe behavior at and beyond this boundary.
    const TEST_LOTTERY_CAP: usize = 10_000;

    /// Build a lottery-eligible UTXO with a uniformly-distributed key.
    ///
    /// Real `tx_hash` values are SHA-256 outputs spread across the whole
    /// keyspace, so the seed-derived offset (also uniform) lands *among* them.
    /// We mirror that here by hashing `index` into `tx_hash` — structured/low
    /// keys clustered in one corner would make a uniform offset almost always
    /// fall above every key, collapsing the window to a fixed start. Amount and
    /// age clear the default draw thresholds, so every such UTXO is eligible.
    fn eligible_test_utxo(index: u64) -> Utxo {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"BOTHO_TEST_UTXO_V1");
        hasher.update(index.to_le_bytes());
        let tx_hash: [u8; 32] = hasher.finalize().into();
        Utxo {
            id: UtxoId {
                tx_hash,
                output_index: 0,
            },
            output: TxOutput {
                amount: 10_000_000, // >= default min_utxo_value (1_000_000)
                target_key: [0u8; 32],
                public_key: [0u8; 32],
                e_memo: None,
                cluster_tags: ClusterTagVector::empty(),
                kem_ciphertext: None,
            },
            created_at: 0, // eligible at any height >= min_utxo_age
        }
    }

    fn insert_test_utxos(ledger: &Ledger, utxos: &[Utxo]) {
        let mut wtxn = ledger.env.write_txn().unwrap();
        for u in utxos {
            let bytes = bincode::serialize(u).unwrap();
            ledger
                .utxo_db
                .put(&mut wtxn, &u.id.to_bytes(), &bytes)
                .unwrap();
        }
        wtxn.commit().unwrap();
    }

    fn candidate_ids(c: &[LotteryCandidate]) -> Vec<[u8; 36]> {
        c.iter().map(|x| x.id).collect()
    }

    /// Determinism + full-set-under-cap. Two independent calls with identical
    /// `(height, prev_block_hash, UTXO set)` must yield an identical candidate
    /// sequence (this is exactly the proposer-vs-validator agreement property,
    /// since both paths call this one function). When the eligible set is at
    /// most the cap, the result is the entire eligible set.
    #[test]
    fn lottery_candidates_deterministic_and_full_under_cap() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let config = LotteryDrawConfig::default();
        let height = 1000u64;
        let prev = [7u8; 32];

        let utxos: Vec<Utxo> = (0..50).map(eligible_test_utxo).collect();
        insert_test_utxos(&ledger, &utxos);

        let a = ledger
            .get_lottery_validation_candidates(height, &prev, &config)
            .unwrap();
        let b = ledger
            .get_lottery_validation_candidates(height, &prev, &config)
            .unwrap();

        // Determinism: identical inputs → identical candidate sequence (order
        // included), so the proposer and every validator agree.
        assert_eq!(candidate_ids(&a), candidate_ids(&b));

        // Under the cap, the candidate set is exactly the full eligible set.
        let got: std::collections::BTreeSet<[u8; 36]> = a.iter().map(|c| c.id).collect();
        let want: std::collections::BTreeSet<[u8; 36]> =
            utxos.iter().map(|u| u.id.to_bytes()).collect();
        assert_eq!(got, want);
        assert_eq!(a.len(), 50);
    }

    /// Under the cap the *set* of candidates is independent of the seed (only
    /// the rotation/order can differ). This confirms the seed offset never
    /// silently drops eligible UTXOs when everyone fits.
    #[test]
    fn lottery_candidates_full_set_independent_of_seed_under_cap() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let config = LotteryDrawConfig::default();
        let height = 1000u64;

        let utxos: Vec<Utxo> = (0..50).map(eligible_test_utxo).collect();
        insert_test_utxos(&ledger, &utxos);

        let a = ledger
            .get_lottery_validation_candidates(height, &[1u8; 32], &config)
            .unwrap();
        let b = ledger
            .get_lottery_validation_candidates(height, &[2u8; 32], &config)
            .unwrap();

        let sa: std::collections::BTreeSet<[u8; 36]> = a.iter().map(|c| c.id).collect();
        let sb: std::collections::BTreeSet<[u8; 36]> = b.iter().map(|c| c.id).collect();
        assert_eq!(
            sa, sb,
            "candidate set must not depend on seed under the cap"
        );
        assert_eq!(sa.len(), 50);
    }

    /// Anti-grind + coverage with more than the cap eligible.
    ///
    /// With a surplus over the 10k cap, the seed-derived wraparound window
    /// rotates each block. A vanity-ground lowest-`tx_hash` UTXO (the
    /// guaranteed-seat exploit under the old fixed first-N prefix) must be
    /// excluded for at least one seed, and every eligible UTXO must appear for
    /// at least one seed (no permanent exclusion).
    #[test]
    fn lottery_candidate_window_degrinds_low_hash_and_covers_all() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let config = LotteryDrawConfig::default();
        let height = 1000u64;

        let surplus = 3_000usize;
        let n = TEST_LOTTERY_CAP + surplus; // 13_000 eligible
        let utxos: Vec<Utxo> = (0..n as u64).map(eligible_test_utxo).collect();
        insert_test_utxos(&ledger, &utxos);

        // The lexicographically lowest UTXO id in the set: the best-case
        // grind target — exactly the "permanent seat" a vanity-ground low
        // tx_hash bought under the old fixed first-N prefix.
        let vanity = utxos
            .iter()
            .map(|u| u.id.to_bytes())
            .min()
            .expect("non-empty utxo set");

        let seeds: u8 = 48;
        let mut vanity_included = 0usize;
        let mut vanity_excluded = 0usize;
        let mut covered: std::collections::HashSet<[u8; 36]> = std::collections::HashSet::new();

        for s in 0..seeds {
            let prev = [s; 32];
            let cands = ledger
                .get_lottery_validation_candidates(height, &prev, &config)
                .unwrap();

            // The 10k cap is enforced when more are eligible.
            assert_eq!(cands.len(), TEST_LOTTERY_CAP);

            let ids: std::collections::HashSet<[u8; 36]> = cands.iter().map(|c| c.id).collect();
            // No duplicates within a single window (the two wraparound segments
            // are disjoint).
            assert_eq!(ids.len(), cands.len());

            if ids.contains(&vanity) {
                vanity_included += 1;
            } else {
                vanity_excluded += 1;
            }
            covered.extend(ids);
        }

        // Anti-grind: the lowest-hash UTXO is NOT guaranteed a seat — excluded
        // for at least one seed (old behavior: always included), yet it still
        // participates for some seeds.
        assert!(
            vanity_excluded > 0,
            "low-hash UTXO was a candidate for every seed (still grindable)"
        );
        assert!(
            vanity_included > 0,
            "low-hash UTXO never participated across the seed range"
        );

        // Coverage: over the seed range every eligible UTXO appears in some
        // window — no permanent exclusion.
        assert_eq!(
            covered.len(),
            n,
            "some eligible UTXOs were never selected for any seed"
        );
    }

    /// The seed-derived offset key is a pure function of `(prev_block_hash,
    /// height)`: identical inputs → identical 36-byte key, and changing either
    /// input changes the key. This is the determinism root for proposer ==
    /// validator candidate windows.
    #[test]
    fn lottery_candidate_offset_key_is_pure_and_sensitive() {
        let prev = [3u8; 32];
        let k = Ledger::lottery_candidate_offset_key(&prev, 100);
        assert_eq!(k, Ledger::lottery_candidate_offset_key(&prev, 100));
        assert_ne!(k, Ledger::lottery_candidate_offset_key(&prev, 101));
        assert_ne!(k, Ledger::lottery_candidate_offset_key(&[4u8; 32], 100));
    }

    // ------------------------------------------------------------------
    // Cluster-tag inflation guard (issue #576, H2-B3)
    // ------------------------------------------------------------------

    /// Build a `TxOutput` with a given amount and cluster tags. Stealth keys
    /// are deterministic functions of `seed` so distinct outputs index under
    /// distinct target keys.
    #[cfg(test)]
    fn mk_tagged_output(amount: u64, seed: u8, tags: ClusterTagVector) -> TxOutput {
        TxOutput {
            amount,
            target_key: [seed; 32],
            public_key: [seed.wrapping_add(100); 32],
            e_memo: None,
            cluster_tags: tags,
            kem_ciphertext: None,
        }
    }

    /// Cluster tag vector attributing 100% of value to a single cluster.
    #[cfg(test)]
    fn full_tag(cluster: u64) -> ClusterTagVector {
        use bth_transaction_types::ClusterId;
        ClusterTagVector::from_pairs(&[(ClusterId(cluster), TAG_WEIGHT_SCALE)])
    }

    /// Unit test of the pure conservation-of-mass core
    /// ([`check_cluster_tag_inheritance`]): a tx whose outputs claim MORE
    /// cluster-tag mass than the inputs supply is rejected, while a legitimate
    /// decayed-inheritance tx (output mass <= input mass) is accepted. This is
    /// the integer/`BTreeMap` logic that the consensus path now enforces.
    #[test]
    fn check_cluster_tag_inheritance_rejects_inflation_accepts_decay() {
        use bth_transaction_types::ClusterId;

        // One ring, one member: 1_000_000 picocredits fully attributed to
        // cluster 1. input mass(cluster 1) = 1_000_000 * 1_000_000 /
        // TAG_WEIGHT_SCALE = 1_000_000 (per-ring max over a single member).
        let inputs = vec![vec![(full_tag(1), 1_000_000u64)]];

        // Legitimate: a decayed inheritance — fee burned, weight decayed to
        // 95%. Output mass = 990_000 * 950_000 / 1_000_000 = 940_500 <= input
        // mass, so it is accepted.
        let legit = vec![mk_tagged_output(
            990_000,
            1,
            ClusterTagVector::from_pairs(&[(ClusterId(1), 950_000)]),
        )];
        assert!(
            check_cluster_tag_inheritance(&inputs, &legit).is_ok(),
            "a legitimately decayed-inheritance tx must pass"
        );

        // Inflated: outputs claim 2x the input's cluster-1 mass out of thin
        // air. output mass = 2_000_000 > input mass 1_000_000 + tolerance →
        // rejected.
        let inflated = vec![mk_tagged_output(2_000_000, 2, full_tag(1))];
        let err = check_cluster_tag_inheritance(&inputs, &inflated)
            .expect_err("a tag-inflated-output tx must be rejected");
        assert!(
            matches!(err, LedgerError::InvalidBlock(_)),
            "inflation must surface as InvalidBlock, got {:?}",
            err
        );
    }

    /// End-to-end through the ledger resolution path
    /// ([`Ledger::verify_cluster_tag_inheritance`]): the guard resolves the
    /// transaction's ring members against the committed UTXO set (the only
    /// deterministic, node-agnostic input set, since the real input is hidden
    /// among the decoys) and rejects a tx whose outputs inflate cluster-tag
    /// mass beyond the resolved ring's supply, while accepting a conserving
    /// tx. This exercises the exact wiring `add_block_inner` invokes.
    #[test]
    fn verify_cluster_tag_inheritance_resolves_ring_and_rejects_inflation() {
        use crate::transaction::{ClsagRingInput, Transaction as Tx, TxInputs};

        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Two on-chain UTXOs, each 1_000_000 fully attributed to cluster 1.
        // The transaction's single ring references both. Under the #581
        // per-ring-maximum bound the resolved cluster-1 input ceiling is
        // max(1_000_000, 1_000_000) = 1_000_000 — NOT the old sum of
        // 2_000_000. This is exactly the decoy-inflation shape: one real
        // 1_000_000 input plus one cluster-1 decoy of equal mass.
        let utxo_outputs = [
            mk_tagged_output(1_000_000, 10, full_tag(1)),
            mk_tagged_output(1_000_000, 20, full_tag(1)),
        ];
        for (idx, output) in utxo_outputs.iter().enumerate() {
            let utxo = Utxo {
                id: UtxoId::new([idx as u8 + 50; 32], idx as u32),
                output: output.clone(),
                created_at: 0,
            };
            let mut wtxn = ledger.env.write_txn().unwrap();
            let bytes = bincode::serialize(&utxo).unwrap();
            ledger
                .utxo_db
                .put(&mut wtxn, &utxo.id.to_bytes(), &bytes)
                .unwrap();
            ledger.add_to_address_index(&mut wtxn, &utxo).unwrap();
            wtxn.commit().unwrap();
        }

        let ring: Vec<RingMember> = utxo_outputs.iter().map(RingMember::from_output).collect();
        let mk_input = || ClsagRingInput {
            ring: ring.clone(),
            key_image: [0u8; 32],
            commitment_key_image: [0u8; 32],
            clsag_signature: Vec::new(),
            pseudo_output_amount: 0,
        };

        // Conserving: output mass 1_000_000 <= per-ring-max ceiling 1_000_000.
        let legit_tx = Tx {
            inputs: TxInputs::new(vec![mk_input()]),
            outputs: vec![mk_tagged_output(1_000_000, 30, full_tag(1))],
            fee: 0,
            created_at_height: 0,
            settlement: None,
        };
        assert!(
            ledger.verify_cluster_tag_inheritance(&legit_tx).is_ok(),
            "a conserving tx must pass the ledger-resolved guard"
        );

        // Decoy-sourced inflation (#581 regression): output mass 2_000_000.
        // Under the OLD sum-over-all-ring-members bound the ceiling was
        // 2_000_000 and this PASSED — the attacker attributed 2_000_000 of
        // cluster-1 mass to their output while spending a single 1_000_000
        // real input, sourcing the extra ceiling from a cluster-1 decoy. Under
        // the per-ring-maximum bound the ceiling is 1_000_000, so it is now
        // REJECTED.
        let decoy_inflated_tx = Tx {
            inputs: TxInputs::new(vec![mk_input()]),
            outputs: vec![mk_tagged_output(2_000_000, 35, full_tag(1))],
            fee: 0,
            created_at_height: 0,
            settlement: None,
        };
        let err = ledger
            .verify_cluster_tag_inheritance(&decoy_inflated_tx)
            .expect_err("decoy-sourced tag inflation must now be rejected (#581)");
        assert!(
            matches!(err, LedgerError::InvalidBlock(_)),
            "decoy inflation must surface as InvalidBlock, got {:?}",
            err
        );

        // Inflated well past any ceiling: output mass 3_000_000 → rejected.
        let inflated_tx = Tx {
            inputs: TxInputs::new(vec![mk_input()]),
            outputs: vec![mk_tagged_output(3_000_000, 40, full_tag(1))],
            fee: 0,
            created_at_height: 0,
            settlement: None,
        };
        let err = ledger
            .verify_cluster_tag_inheritance(&inflated_tx)
            .expect_err("a tag-inflated-output tx must be rejected by the ledger guard");
        assert!(
            matches!(err, LedgerError::InvalidBlock(_)),
            "inflation must surface as InvalidBlock, got {:?}",
            err
        );
    }

    /// #581 pure-function coverage of the per-ring-maximum input bound:
    /// (a) the ring-size inflation multiplier is gone, (b) multi-input
    /// transactions that legitimately combine cluster mass from several real
    /// inputs are NOT false-rejected (liveness).
    #[test]
    fn check_cluster_tag_inheritance_per_ring_max_bound() {
        use bth_transaction_types::{ClusterId, ClusterTagEntry};

        // (a) One ring of THREE cluster-1 decoys, each mass 1_000_000. The old
        // bound summed to 3_000_000; the per-ring maximum is 1_000_000. An
        // output claiming 3_000_000 of cluster-1 from this single ring (one
        // real 1_000_000 input + two cluster-1 decoys) is rejected.
        let one_ring_three_decoys = vec![vec![
            (full_tag(1), 1_000_000u64),
            (full_tag(1), 1_000_000u64),
            (full_tag(1), 1_000_000u64),
        ]];
        let inflated = vec![mk_tagged_output(3_000_000, 1, full_tag(1))];
        assert!(
            check_cluster_tag_inheritance(&one_ring_three_decoys, &inflated).is_err(),
            "ring-size-multiplied decoy inflation must be rejected (bound is the \
             per-ring max 1_000_000, not the sum 3_000_000)"
        );
        // The same ring legitimately supports an output up to its 1_000_000
        // ceiling.
        let at_ceiling = vec![mk_tagged_output(1_000_000, 2, full_tag(1))];
        assert!(
            check_cluster_tag_inheritance(&one_ring_three_decoys, &at_ceiling).is_ok(),
            "an output at the per-ring-max ceiling must pass"
        );

        // (b) Liveness: THREE separate rings, each a real 1_000_000 cluster-1
        // input (single-member rings). The bound is the SUM of the per-ring
        // maxima = 3_000_000, so a tx that legitimately merges all three real
        // inputs into a 3_000_000 cluster-1 output MUST pass — the per-ring
        // maximum must not collapse independent real inputs.
        let three_real_inputs = vec![
            vec![(full_tag(1), 1_000_000u64)],
            vec![(full_tag(1), 1_000_000u64)],
            vec![(full_tag(1), 1_000_000u64)],
        ];
        let merged = vec![mk_tagged_output(3_000_000, 3, full_tag(1))];
        assert!(
            check_cluster_tag_inheritance(&three_real_inputs, &merged).is_ok(),
            "merging three real single-input rings into one output must not \
             false-reject (bound = sum of per-ring maxima = 3_000_000)"
        );

        // (c) A background (untagged) real input with a cluster-1 decoy: the
        // ceiling is the decoy's mass (residual documented in #581), but it is
        // no longer ring-size-multiplied. An output above the single decoy's
        // mass is still rejected.
        let bg_input_one_decoy = vec![vec![
            (ClusterTagVector::empty(), 1_000_000u64),
            (full_tag(1), 1_000_000u64),
        ]];
        let over = vec![mk_tagged_output(
            1_500_000,
            4,
            ClusterTagVector::from_pairs(&[(ClusterId(1), TAG_WEIGHT_SCALE)]),
        )];
        assert!(
            check_cluster_tag_inheritance(&bg_input_one_decoy, &over).is_err(),
            "output above the single decoy's cluster-1 mass must be rejected"
        );

        // (d) Liveness under a duplicate-cluster-id member: a single member
        // whose tag vector carries cluster 1 twice (weights 400_000 + 600_000)
        // has cluster-1 mass 1_000_000 * (400_000 + 600_000) / SCALE =
        // 1_000_000. The per-member SUM must be used for the max, so the
        // ceiling is 1_000_000 and an output at 1_000_000 must PASS. A naive
        // per-entry max would have used max(400_000, 600_000) = 600_000 and
        // FALSE-REJECTED this valid tx — a consensus halt. We do not rely on
        // the tag-vector uniqueness invariant here.
        let mut dup_tags = ClusterTagVector::empty();
        dup_tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(1),
            weight: 400_000,
        });
        dup_tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(1),
            weight: 600_000,
        });
        let dup_entry_member = vec![vec![(dup_tags, 1_000_000u64)]];
        let at_full = vec![mk_tagged_output(1_000_000, 5, full_tag(1))];
        assert!(
            check_cluster_tag_inheritance(&dup_entry_member, &at_full).is_ok(),
            "a member's duplicate cluster entries must SUM (per-member mass), \
             not max — else a valid tx is false-rejected (#581 liveness)"
        );
    }

    // ========================================================================
    // H1-B4 consensus fee floor (issue #578, design #574)
    // ========================================================================

    /// Store on-chain UTXOs and return their ring members. Each entry is
    /// `(amount, created_at, cluster_id_or_0_for_background, seed)`.
    #[cfg(test)]
    fn seed_ring_utxos(ledger: &Ledger, specs: &[(u64, u64, u64, u8)]) -> Vec<RingMember> {
        use bth_transaction_types::ClusterId;
        let mut ring = Vec::new();
        for &(amount, created_at, cluster, seed) in specs {
            let tags = if cluster == 0 {
                ClusterTagVector::empty()
            } else {
                ClusterTagVector::from_pairs(&[(ClusterId(cluster), TAG_WEIGHT_SCALE)])
            };
            let output = mk_tagged_output(amount, seed, tags);
            let utxo = Utxo {
                id: UtxoId::new([seed; 32], 0),
                output: output.clone(),
                created_at,
            };
            let mut wtxn = ledger.env.write_txn().unwrap();
            let bytes = bincode::serialize(&utxo).unwrap();
            ledger
                .utxo_db
                .put(&mut wtxn, &utxo.id.to_bytes(), &bytes)
                .unwrap();
            ledger.add_to_address_index(&mut wtxn, &utxo).unwrap();
            wtxn.commit().unwrap();
            ring.push(RingMember::from_output(&output));
        }
        ring
    }

    #[cfg(test)]
    fn mk_transfer_tx(ring: Vec<RingMember>, outputs: Vec<TxOutput>, fee: u64) -> BothoTransaction {
        use crate::transaction::{ClsagRingInput, TxInputs};
        let input = ClsagRingInput {
            ring,
            key_image: [0u8; 32],
            commitment_key_image: [0u8; 32],
            clsag_signature: Vec::new(),
            pseudo_output_amount: 0,
        };
        BothoTransaction {
            inputs: TxInputs::new(vec![input]),
            outputs,
            fee,
            created_at_height: 0,
            settlement: None,
        }
    }

    /// Like [`mk_transfer_tx`] but flags the tx as a #831 demurrage-settlement
    /// certifying `settled_value` (defaults to Σ output amounts).
    #[cfg(test)]
    fn mk_settlement_tx(
        ring: Vec<RingMember>,
        outputs: Vec<TxOutput>,
        fee: u64,
        settled_value: u64,
    ) -> BothoTransaction {
        let mut tx = mk_transfer_tx(ring, outputs, fee);
        tx.settlement = Some(crate::transaction::SettlementInfo { settled_value });
        tx
    }

    /// A transfer tx with a fee one picocredit below the recomputed floor is
    /// rejected, while the same tx at exactly the floor is accepted. This is
    /// the core H1-B4 gate.
    #[test]
    fn consensus_fee_floor_rejects_under_fee_accepts_at_floor() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Background ring/outputs at a modest height: the floor is the pure
        // size/output base (demurrage is zero at factor 1x).
        let ring = seed_ring_utxos(&ledger, &[(1_000_000, 0, 0, 60)]);
        let outputs = vec![mk_tagged_output(900_000, 70, ClusterTagVector::empty())];
        let block_height = 1_000u64;

        // Recompute the floor and probe both sides of it.
        let floor = {
            let tx = mk_transfer_tx(ring.clone(), outputs.clone(), 0);
            ledger.consensus_fee_floor(&tx, block_height).unwrap()
        };
        assert!(floor > 0, "background floor should be a positive size fee");

        let under = mk_transfer_tx(ring.clone(), outputs.clone(), floor - 1);
        let err = ledger
            .verify_consensus_fee_floor(&under, block_height)
            .expect_err("a fee below the floor must be rejected");
        assert!(matches!(err, LedgerError::InvalidBlock(_)), "got {:?}", err);

        let at_floor = mk_transfer_tx(ring, outputs, floor);
        assert!(
            ledger
                .verify_consensus_fee_floor(&at_floor, block_height)
                .is_ok(),
            "a fee at exactly the floor must be accepted"
        );
    }

    /// The floor is a pure function of block + chain state: an independent
    /// recomputation yields the identical value (determinism contract, #574
    /// Q5).
    #[test]
    fn consensus_fee_floor_is_deterministic_recomputation() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        ledger.set_cluster_wealth_for_test(7, 500_000_000).unwrap();

        let ring = seed_ring_utxos(&ledger, &[(50_000_000, 0, 7, 61), (1_000, 900, 0, 62)]);
        let outputs = vec![mk_tagged_output(40_000_000, 71, {
            use bth_transaction_types::ClusterId;
            ClusterTagVector::from_pairs(&[(ClusterId(7), TAG_WEIGHT_SCALE)])
        })];
        let tx = mk_transfer_tx(ring, outputs, 0);

        let a = ledger.consensus_fee_floor(&tx, 1_000).unwrap();
        let b = ledger.consensus_fee_floor(&tx, 1_000).unwrap();
        assert_eq!(a, b, "the floor must be reproducible bit-for-bit");
        assert!(a > 0);
    }

    /// H2/B1 demurrage@max: padding a wealthy old input's ring with fresh
    /// decoys no longer drives the demurrage clock (and therefore the
    /// floor) to zero. Contrast with the old value-weighted centroid, which
    /// the equal-value fresh decoys would have diluted ~91%.
    #[test]
    fn consensus_fee_floor_demurrage_at_max_resists_fresh_decoy_dilution() {
        use bth_transaction_types::ClusterId;
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        // A wealthy cluster so the factor floor lifts demurrage above zero.
        ledger
            .set_cluster_wealth_for_test(9, 10_000_000_000_000_000_000)
            .unwrap();

        // Height must be at/after the halving interval (~6.3M blocks) so
        // demurrage is active (zero during the bootstrap epoch by design —
        // MonetaryPolicy::demurrage_rate_bps).
        let block_height = 8_000_000u64;
        // Real input: old (created at height 0, ~one year+ of age), wealthy;
        // plus 10 fresh, equal-value decoys created at the current height.
        let mut specs = vec![(100_000_000u64, 0u64, 9u64, 80u8)];
        for i in 0..10u8 {
            specs.push((100_000_000, block_height, 9, 90 + i));
        }
        let ring = seed_ring_utxos(&ledger, &specs);
        let outputs = vec![mk_tagged_output(
            90_000_000,
            120,
            ClusterTagVector::from_pairs(&[(ClusterId(9), TAG_WEIGHT_SCALE)]),
        )];
        let tx = mk_transfer_tx(ring, outputs, 0);

        let floor = ledger.consensus_fee_floor(&tx, block_height).unwrap();

        // Independently compute the base (no demurrage) to isolate the
        // demurrage contribution.
        let base_only = {
            use bth_cluster_tax::{FeeConfig, TransactionType};
            let fc = FeeConfig::default();
            let cw = ledger
                .effective_cluster_wealth_from_outputs(&tx.outputs)
                .unwrap();
            fc.minimum_fee_dynamic_with_outputs(
                TransactionType::Hidden,
                tx.estimate_size(),
                cw,
                tx.outputs.len(),
                0,
                CONSENSUS_FEE_BASE,
            )
        };
        let demurrage = floor - base_only;
        assert!(
            demurrage > 0,
            "max-quantile must recover the old real input's age so demurrage is charged \
             despite the fresh decoys (demurrage={demurrage}, floor={floor}, base={base_only})"
        );

        // Contrast: the OLD value-weighted centroid kernel would have been
        // diluted ~91% by the 10 equal-value fresh decoys, so the demurrage it
        // implies is far smaller than the max-quantile's. This is the exact H2
        // vector the cutover closes.
        let age_members: Vec<(u64, u64)> = {
            let mut v = vec![(100_000_000u64, 0u64)];
            for _ in 0..10 {
                v.push((100_000_000, block_height));
            }
            v
        };
        let mean_age = bth_cluster_tax::ring_elapsed_centroid(&age_members, block_height);
        let max_age = bth_cluster_tax::ring_elapsed_quantile(
            &age_members,
            block_height,
            CONSENSUS_RING_AGE_QUANTILE_BPS,
        );
        assert!(
            max_age > mean_age * 8,
            "the max-quantile age {max_age} must dwarf the diluted mean {mean_age}"
        );
    }

    /// B2 factor floor: outputs tagged as background (factor 1x) over a WEALTHY
    /// ring cannot escape the wealthy-ring demurrage factor — the floor still
    /// charges demurrage.
    #[test]
    fn consensus_fee_floor_factor_floor_binds_background_outputs() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        ledger
            .set_cluster_wealth_for_test(5, 10_000_000_000_000_000_000)
            .unwrap();

        // Past the halving interval so demurrage is active (see the note in the
        // decoy-dilution test above).
        let block_height = 8_000_000u64;
        // Ring members carry a wealthy cluster's tags and are old.
        let ring = seed_ring_utxos(&ledger, &[(100_000_000, 0, 5, 40), (100_000_000, 0, 5, 41)]);
        // Outputs claim BACKGROUND (untagged) -> claimed factor is 1x -> zero
        // demurrage on the claim alone. The ring-centroid floor must override.
        let outputs = vec![mk_tagged_output(90_000_000, 130, ClusterTagVector::empty())];
        let tx = mk_transfer_tx(ring, outputs, 0);

        let floor = ledger.consensus_fee_floor(&tx, block_height).unwrap();
        let base_only = {
            use bth_cluster_tax::{FeeConfig, TransactionType};
            let fc = FeeConfig::default();
            // Background outputs -> effective wealth 0 -> factor 1x base.
            fc.minimum_fee_dynamic_with_outputs(
                TransactionType::Hidden,
                tx.estimate_size(),
                0,
                tx.outputs.len(),
                0,
                CONSENSUS_FEE_BASE,
            )
        };
        assert!(
            floor > base_only,
            "the wealthy ring's implied factor must floor the background claim and \
             charge demurrage (floor={floor}, base={base_only})"
        );
    }

    /// Recompute the pure base minimum fee (no demurrage) for a tx, so a test
    /// can isolate the demurrage contribution of the consensus floor.
    #[cfg(test)]
    fn base_only_fee(ledger: &Ledger, tx: &BothoTransaction) -> u64 {
        use bth_cluster_tax::{FeeConfig, TransactionType};
        let fc = FeeConfig::default();
        let cw = ledger
            .effective_cluster_wealth_from_outputs(&tx.outputs)
            .unwrap();
        fc.minimum_fee_dynamic_with_outputs(
            TransactionType::Hidden,
            tx.estimate_size(),
            cw,
            tx.outputs.len(),
            tx.outputs.iter().filter(|o| o.has_memo()).count(),
            CONSENSUS_FEE_BASE,
        )
    }

    /// #925: spending a YOUNG wealthy coin to a background output — the #834
    /// background-reset exploit — is now PRICED at the capitalized reset even
    /// though the accrued-to-date demurrage is ≈0 (elapsed ≈ 0). This is the
    /// consensus analog of `demurrage_background_reset_leak_is_real`.
    #[test]
    fn consensus_fee_floor_prices_young_wealthy_to_background_downgrade() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        // Wealthy cluster 7 (10M BTH in picocredits).
        ledger
            .set_cluster_wealth_for_test(7, 10_000_000_000_000_000_000)
            .unwrap();

        // Demurrage active (past the halving interval).
        let block_height = 8_000_000u64;
        // The real input is YOUNG: created at the current height (age 0), and
        // wealthy (tagged to cluster 7). Fresh wealthy decoys too — the whole
        // ring is young.
        let mut specs = vec![(100_000_000u64, block_height, 7u64, 70u8)];
        for i in 0..10u8 {
            specs.push((100_000_000, block_height, 7, 71 + i));
        }
        let ring = seed_ring_utxos(&ledger, &specs);
        // Output: fully background (the deflating downgrade).
        let output_sum = 90_000_000u64;
        let outputs = vec![mk_tagged_output(output_sum, 140, ClusterTagVector::empty())];
        let tx = mk_transfer_tx(ring, outputs, 0);

        let floor = ledger.consensus_fee_floor(&tx, block_height).unwrap();
        let base_only = base_only_fee(&ledger, &tx);
        let demurrage = floor - base_only;

        assert!(
            demurrage > 0,
            "the young-wealthy→background downgrade must be priced (leak closed): \
             demurrage={demurrage}"
        );

        // The charge equals the capitalized reset over SETTLEMENT_HORIZON_BLOCKS,
        // computed from the same public inputs.
        let policy = crate::monetary::mainnet_policy();
        let rate = policy.demurrage_rate_bps(block_height);
        let bpy = (365 * 24 * 60 * 60) / policy.target_block_time_secs.max(1);
        let curve = bth_cluster_tax::ClusterFactorCurve::default_params();
        let floor_factor =
            bth_cluster_tax::ring_centroid_implied_factor(&[(output_sum, 10u128.pow(19))], &curve);
        assert_eq!(floor_factor, 5745, "10M-BTH cluster floors at 5.745×");
        let expected = bth_cluster_tax::capitalized_reset_charge(
            output_sum,
            floor_factor,
            bth_cluster_tax::demurrage::FACTOR_SCALE, // background 1× declared
            bth_cluster_tax::SETTLEMENT_HORIZON_BLOCKS,
            rate,
            bpy,
        );
        assert_eq!(
            demurrage, expected,
            "downgrade charge must equal the capitalized reset ({expected})"
        );

        // Determinism: proposer and validators recompute the identical verdict.
        let again = ledger.consensus_fee_floor(&tx, block_height).unwrap();
        assert_eq!(floor, again, "consensus floor must be deterministic");
    }

    // ---- #831: demurrage-settlement operation (the wrap on-ramp) ----

    /// The consensus fee floor prices an explicit settlement of a young wealthy
    /// coin at exactly `base + demurrage_settlement_charge` — the SAME shared
    /// `capitalized_reset_charge` primitive #925 uses (not a reimplementation),
    /// computed from the ring-floored input class the spender cannot rewrite.
    /// Determinism: proposer == validator (recomputation is bit-identical).
    #[test]
    fn consensus_fee_floor_prices_settlement_at_capitalized_charge() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        ledger
            .set_cluster_wealth_for_test(7, 10_000_000_000_000_000_000)
            .unwrap();

        let block_height = 8_000_000u64;
        // Young wealthy ring (cluster 7); the settlement declares all-background
        // outputs (the reclassification).
        let mut specs = vec![(100_000_000u64, block_height, 7u64, 200u8)];
        for i in 0..10u8 {
            specs.push((100_000_000, block_height, 7, 201 + i));
        }
        let ring = seed_ring_utxos(&ledger, &specs);
        let output_sum = 90_000_000u64;
        let outputs = vec![mk_tagged_output(output_sum, 230, ClusterTagVector::empty())];
        let tx = mk_settlement_tx(ring, outputs, 0, output_sum);

        let floor = ledger.consensus_fee_floor(&tx, block_height).unwrap();
        let base_only = base_only_fee(&ledger, &tx);
        let charge = floor - base_only;

        // The settlement charge is exactly the shared primitive to background.
        let policy = crate::monetary::mainnet_policy();
        let rate = policy.demurrage_rate_bps(block_height);
        let bpy = (365 * 24 * 60 * 60) / policy.target_block_time_secs.max(1);
        let curve = bth_cluster_tax::ClusterFactorCurve::default_params();
        let floor_factor =
            bth_cluster_tax::ring_centroid_implied_factor(&[(output_sum, 10u128.pow(19))], &curve);
        let expected =
            bth_cluster_tax::demurrage_settlement_charge(output_sum, floor_factor, rate, bpy);
        assert!(expected > 0, "wealthy settlement must cost > 0");
        assert_eq!(
            charge, expected,
            "settlement priced at demurrage_settlement_charge (reuses #925's capitalized primitive)"
        );

        // Determinism.
        let again = ledger.consensus_fee_floor(&tx, block_height).unwrap();
        assert_eq!(floor, again, "settlement floor must be deterministic");
    }

    /// A settlement of a genuinely background coin costs ZERO extra (factor-1
    /// settles free), so honest commerce coins wrap without penalty.
    #[test]
    fn consensus_fee_floor_settlement_of_background_is_free() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let block_height = 8_000_000u64;
        let ring = seed_ring_utxos(&ledger, &[(1_000_000, 0, 0, 210)]);
        let output_sum = 900_000u64;
        let outputs = vec![mk_tagged_output(output_sum, 231, ClusterTagVector::empty())];
        let tx = mk_settlement_tx(ring, outputs, 0, output_sum);

        let floor = ledger.consensus_fee_floor(&tx, block_height).unwrap();
        let base_only = base_only_fee(&ledger, &tx);
        assert_eq!(
            floor, base_only,
            "settling a background coin adds no charge (factor-1 settles free)"
        );
    }

    /// UNDER-PAY rejected: a settlement whose `fee` is one picocredit below
    /// `base + settlement_charge` is rejected at block acceptance; at exactly
    /// the floor it passes the fee gate.
    #[test]
    fn consensus_fee_floor_rejects_underpaid_settlement() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        ledger
            .set_cluster_wealth_for_test(7, 10_000_000_000_000_000_000)
            .unwrap();
        let block_height = 8_000_000u64;
        let ring = seed_ring_utxos(&ledger, &[(100_000_000, block_height, 7, 212)]);
        let output_sum = 90_000_000u64;
        let outputs = vec![mk_tagged_output(output_sum, 233, ClusterTagVector::empty())];

        let floor = {
            let tx = mk_settlement_tx(ring.clone(), outputs.clone(), 0, output_sum);
            ledger.consensus_fee_floor(&tx, block_height).unwrap()
        };
        assert!(floor > 1);

        let under = mk_settlement_tx(ring.clone(), outputs.clone(), floor - 1, output_sum);
        assert!(
            ledger
                .verify_consensus_fee_floor(&under, block_height)
                .is_err(),
            "an under-paid settlement must be rejected"
        );
        let at_floor = mk_settlement_tx(ring, outputs, floor, output_sum);
        assert!(
            ledger
                .verify_consensus_fee_floor(&at_floor, block_height)
                .is_ok(),
            "a settlement paying exactly the floor is accepted by the fee gate"
        );
    }

    /// Tag-rewrite rule / anti-laundering: a settlement whose output still
    /// carries a (wealthy) cluster tag is rejected — a settlement may ONLY drop
    /// to full background, never leave partial provenance on a "settled" coin.
    #[test]
    fn verify_settlement_rejects_nonbackground_output() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let ring = seed_ring_utxos(&ledger, &[(1_000_000, 0, 7, 214)]);
        // Output still tagged to cluster 7 — NOT a clean reclassification.
        let outputs = vec![mk_tagged_output(900_000, 235, full_tag(7))];
        let tx = mk_settlement_tx(ring, outputs, 0, 900_000);
        let err = ledger
            .verify_settlement(&tx)
            .expect_err("settlement with a tagged output must be rejected");
        assert!(matches!(err, LedgerError::InvalidBlock(_)), "got {:?}", err);
    }

    /// A settlement whose certified `settled_value` disagrees with its outputs
    /// is rejected (no under-certifying to weaken the wrap accounting).
    #[test]
    fn verify_settlement_rejects_wrong_certified_value() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let ring = seed_ring_utxos(&ledger, &[(1_000_000, 0, 0, 216)]);
        let outputs = vec![mk_tagged_output(900_000, 236, ClusterTagVector::empty())];
        // certified value != Σ outputs (900_000)
        let tx = mk_settlement_tx(ring, outputs, 0, 800_000);
        assert!(
            ledger.verify_settlement(&tx).is_err(),
            "settled_value must equal the output sum"
        );
    }

    /// A well-formed settlement (all-background outputs, matching certified
    /// value) passes structural validation; a non-settlement tx is unaffected.
    #[test]
    fn verify_settlement_accepts_wellformed_and_ignores_transfers() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let ring = seed_ring_utxos(&ledger, &[(1_000_000, 0, 0, 218)]);
        let outputs = vec![mk_tagged_output(900_000, 237, ClusterTagVector::empty())];
        let settlement = mk_settlement_tx(ring.clone(), outputs.clone(), 0, 900_000);
        assert!(ledger.verify_settlement(&settlement).is_ok());

        // A plain transfer (even one with a tagged output) is not a settlement,
        // so verify_settlement is a no-op Ok for it.
        let transfer = mk_transfer_tx(ring, vec![mk_tagged_output(900_000, 238, full_tag(7))], 0);
        assert!(ledger.verify_settlement(&transfer).is_ok());
    }

    /// The `wrap_eligible` predicate: true ONLY for a background output
    /// produced by an accepted settlement tx. A wealthy output is
    /// ineligible; and crucially a background output from a NON-settlement
    /// spend is ALSO ineligible — binding to the flag (not merely the tag)
    /// is what closes the cheap-escape (a normal spend-to-background cannot
    /// wrap).
    #[test]
    fn wrap_eligible_requires_settlement_flag_and_background() {
        let bg = mk_tagged_output(900_000, 240, ClusterTagVector::empty());
        let wealthy = mk_tagged_output(900_000, 241, full_tag(7));

        let settlement = mk_settlement_tx(Vec::new(), vec![bg.clone()], 0, 900_000);
        let transfer = mk_transfer_tx(Vec::new(), vec![bg.clone()], 0);

        // The single sanctioned case: background output + settlement flag.
        assert!(
            wrap_eligible(&bg, &settlement),
            "settled background is wrap-eligible"
        );

        // Cheap-escape closed: a normal spend-to-background is NOT eligible.
        assert!(
            !wrap_eligible(&bg, &transfer),
            "a background output from a non-settlement spend must NOT be wrap-eligible"
        );

        // A wealthy output is never eligible, flag or not.
        assert!(!wrap_eligible(&wealthy, &settlement));
        assert!(!wrap_eligible(&wealthy, &transfer));
    }

    /// #925 no-over-charge: an honest background→background spend (declared 1×
    /// over a background ring) incurs ZERO downgrade charge — the floor is 1×
    /// on both sides, so there is nothing to price. Honest commerce is
    /// untouched.
    #[test]
    fn consensus_fee_floor_honest_background_spend_has_zero_downgrade_charge() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let block_height = 8_000_000u64;
        // Background ring (cluster 0 = untagged) + background output.
        let ring = seed_ring_utxos(
            &ledger,
            &[(100_000_000, 0, 0, 50), (100_000_000, 10, 0, 51)],
        );
        let outputs = vec![mk_tagged_output(90_000_000, 150, ClusterTagVector::empty())];
        let tx = mk_transfer_tx(ring, outputs, 0);

        let floor = ledger.consensus_fee_floor(&tx, block_height).unwrap();
        let base_only = base_only_fee(&ledger, &tx);
        assert_eq!(
            floor, base_only,
            "an honest background spend pays zero demurrage (floor==base): \
             floor={floor}, base={base_only}"
        );
    }

    /// #925 max-form: an honest OLD in-class wealthy spend pays its
    /// accrued-to-date demurrage, NOT the (larger) capitalized reset — the
    /// `max(accrued, capitalized)` form never over-charges an in-class hold
    /// because there is no downgrade (capitalized term is 0).
    #[test]
    fn consensus_fee_floor_honest_inclass_wealthy_pays_accrued_not_capitalized() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        ledger
            .set_cluster_wealth_for_test(8, 10_000_000_000_000_000_000)
            .unwrap();
        let block_height = 8_000_000u64;
        // OLD wealthy ring (created at height 0 → age 8M blocks), spent IN-CLASS
        // (output tagged to the same wealthy cluster 8).
        let ring = seed_ring_utxos(&ledger, &[(100_000_000, 0, 8, 60), (100_000_000, 0, 8, 61)]);
        let output_sum = 90_000_000u64;
        let outputs = vec![mk_tagged_output(
            output_sum,
            160,
            ClusterTagVector::from_pairs(&[(
                bth_transaction_types::ClusterId(8),
                TAG_WEIGHT_SCALE,
            )]),
        )];
        let tx = mk_transfer_tx(ring, outputs, 0);

        let floor = ledger.consensus_fee_floor(&tx, block_height).unwrap();
        let base_only = base_only_fee(&ledger, &tx);
        let demurrage = floor - base_only;

        let policy = crate::monetary::mainnet_policy();
        let rate = policy.demurrage_rate_bps(block_height);
        let bpy = (365 * 24 * 60 * 60) / policy.target_block_time_secs.max(1);
        let elapsed = bth_cluster_tax::ring_elapsed_quantile(
            &[(100_000_000, 0), (100_000_000, 0)],
            block_height,
            CONSENSUS_RING_AGE_QUANTILE_BPS,
        );
        // In-class: floor factor == declared factor == 5745, so the accrued term
        // is charged and the capitalized (downgrade) term is exactly 0.
        let accrued = bth_cluster_tax::demurrage_charge(output_sum, 5745, elapsed, rate, bpy);
        assert_eq!(
            demurrage, accrued,
            "an in-class hold pays only accrued-to-date demurrage: \
             demurrage={demurrage}, accrued={accrued}"
        );
        // And that accrued (8M blocks ≈ 1.27yr) is strictly below the 5yr
        // capitalized reset a downgrade would have cost — proving `max` picked
        // the smaller, honest obligation.
        let capitalized = bth_cluster_tax::capitalized_reset_charge(
            output_sum,
            5745,
            bth_cluster_tax::demurrage::FACTOR_SCALE,
            bth_cluster_tax::SETTLEMENT_HORIZON_BLOCKS,
            rate,
            bpy,
        );
        assert!(
            accrued < capitalized,
            "the honest accrued charge {accrued} must be below the capitalized reset \
             {capitalized} it is NOT charged"
        );
    }

    /// Congestion independence: the consensus floor never reads the dynamic fee
    /// base, so it is identical regardless of any node's congestion state. Here
    /// we assert the floor equals the base computed with the pinned
    /// [`CONSENSUS_FEE_BASE`] and is unaffected by (nonexistent) node-local
    /// congestion — a node with a hot vs. cold EMA computes the same floor.
    #[test]
    fn consensus_fee_floor_is_congestion_independent() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let ring = seed_ring_utxos(&ledger, &[(1_000_000, 0, 0, 63)]);
        let outputs = vec![mk_tagged_output(900_000, 73, ClusterTagVector::empty())];
        let tx = mk_transfer_tx(ring, outputs, 0);
        let block_height = 500u64;

        let floor = ledger.consensus_fee_floor(&tx, block_height).unwrap();

        // The floor must match a recomputation that ONLY ever uses the fixed
        // congestion-free base. If congestion had leaked in, this would depend
        // on a DynamicFeeBase EMA — it does not exist in this path.
        use bth_cluster_tax::{FeeConfig, TransactionType};
        let fc = FeeConfig::default();
        let expected_base = fc.minimum_fee_dynamic_with_outputs(
            TransactionType::Hidden,
            tx.estimate_size(),
            0, // background outputs -> zero effective wealth
            tx.outputs.len(),
            0,
            CONSENSUS_FEE_BASE,
        );
        assert_eq!(
            floor, expected_base,
            "background floor must equal the congestion-free base (no demurrage, no dynamic base)"
        );
    }

    /// Fail-closed (M7): a demurrage-relevant DB read error propagates as a
    /// LedgerError rather than defaulting to a lower floor. Exercised
    /// indirectly via the happy path (get_cluster_wealth returns 0 for
    /// absent clusters, not an error) plus the explicit contract in the
    /// code; here we assert that a wealthy cluster's wealth actually raises
    /// the floor (proving the read is consulted, not silently zeroed).
    #[test]
    fn consensus_fee_floor_consults_cluster_wealth() {
        use bth_transaction_types::ClusterId;
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        let tags = ClusterTagVector::from_pairs(&[(ClusterId(3), TAG_WEIGHT_SCALE)]);
        let ring = seed_ring_utxos(&ledger, &[(100_000_000, 0, 3, 64)]);
        let outputs = vec![mk_tagged_output(90_000_000, 74, tags.clone())];
        let tx = mk_transfer_tx(ring, outputs, 0);

        let poor = ledger.consensus_fee_floor(&tx, 1000).unwrap();
        ledger
            .set_cluster_wealth_for_test(3, 10_000_000_000_000_000_000)
            .unwrap();
        let rich = ledger.consensus_fee_floor(&tx, 1000).unwrap();

        assert!(
            rich > poor,
            "raising the cluster's global wealth must raise the floor (poor={poor}, rich={rich})"
        );
    }

    // ========================================================================
    // ADR 0007 bridge-import cluster tagging + >=F import floor (#938)
    //
    // Consensus-layer assertions mirroring the #940 calibration sim
    // (cluster-tax/src/simulation/bridge_import_sweep.rs) at the fee-floor
    // enforcement point. K = 17,280 blocks, F = 1.5x (1500 FACTOR_SCALE).
    // ========================================================================

    /// A small import (below the curve knee) lands at exactly the >=F floor,
    /// and a flood import lands at the epoch-summed factor (well above F).
    /// This is the core ADR 0007 pricing at the consensus layer.
    #[test]
    fn import_floor_small_lands_at_f_flood_lands_at_curve() {
        use bth_cluster_tax::{
            import_cluster_id_for_height, ClusterFactorCurve, BRIDGE_IMPORT_FACTOR_FLOOR,
            PICO_PER_BTH,
        };
        use bth_transaction_types::ClusterId;

        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // An unwrap output created at some height carries this-epoch's import id.
        let height = 3 * bth_cluster_tax::BRIDGE_IMPORT_EPOCH_BLOCKS + 42;
        let import_id = import_cluster_id_for_height(height).0;
        ledger
            .record_bridge_import_cluster_for_test(import_id)
            .unwrap();

        // SMALL import: a 1,000-BTH epoch. curve(small) < F, so the import floor
        // binds at exactly F = 1.5x. import_floor_factor_from_outputs blends F on
        // the (100%) import-tagged output => exactly F.
        let small_out = vec![mk_tagged_output(
            90_000_000,
            80,
            ClusterTagVector::from_pairs(&[(ClusterId(import_id), TAG_WEIGHT_SCALE)]),
        )];
        ledger
            .set_cluster_wealth_for_test(import_id, 1_000 * PICO_PER_BTH)
            .unwrap();
        let small_floor = ledger.import_floor_factor_from_outputs(&small_out).unwrap();
        assert_eq!(
            small_floor, BRIDGE_IMPORT_FACTOR_FLOOR,
            "a small import must land at exactly F=1.5x (got {small_floor})"
        );

        // FLOOD import: a 10,000,000-BTH epoch saturates the curve. The import
        // *cluster factor* (curve then floor) is well above F. The floor does
        // not lower a curve factor that already exceeds it.
        let curve = ClusterFactorCurve::default_params();
        let flood_wealth = 10_000_000u128 * PICO_PER_BTH;
        let flood_factor = bth_cluster_tax::import_cluster_factor(flood_wealth, &curve);
        assert!(
            flood_factor > 5_000,
            "a flood import must price near 6x, got {flood_factor}"
        );
        assert_eq!(
            flood_factor,
            curve.factor(flood_wealth),
            "the >=F floor must NOT alter a flood factor already above F (no double-floor)"
        );
    }

    /// Two unwraps in the SAME epoch share one accumulating cluster (the
    /// Sybil-resistance load-bearing fact). Distinct heights inside
    /// `[mK,(m+1)K)` resolve to the same import cluster id, so their wealth
    /// pools together.
    #[test]
    fn import_two_unwraps_same_epoch_share_cluster() {
        use bth_cluster_tax::{import_cluster_id_for_height, BRIDGE_IMPORT_EPOCH_BLOCKS};

        let m = 5u64;
        let h1 = m * BRIDGE_IMPORT_EPOCH_BLOCKS + 1;
        let h2 = m * BRIDGE_IMPORT_EPOCH_BLOCKS + (BRIDGE_IMPORT_EPOCH_BLOCKS - 1);
        assert_eq!(
            import_cluster_id_for_height(h1),
            import_cluster_id_for_height(h2),
            "two unwraps in the same epoch must share one import cluster"
        );

        // And that shared cluster, once recorded, floors an output tagged to it.
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let shared = import_cluster_id_for_height(h1).0;
        ledger
            .record_bridge_import_cluster_for_test(shared)
            .unwrap();
        assert!(ledger.is_bridge_import_cluster(shared).unwrap());
    }

    /// Unwraps in DIFFERENT epochs form distinct clusters (diluting requires
    /// spreading across epochs — the time-as-cost that defeats the drip-split).
    #[test]
    fn import_different_epochs_distinct_clusters() {
        use bth_cluster_tax::{import_cluster_id_for_height, BRIDGE_IMPORT_EPOCH_BLOCKS};
        let h1 = 5 * BRIDGE_IMPORT_EPOCH_BLOCKS + 1;
        let h2 = 6 * BRIDGE_IMPORT_EPOCH_BLOCKS + 1;
        assert_ne!(
            import_cluster_id_for_height(h1),
            import_cluster_id_for_height(h2),
            "unwraps in different epochs must form distinct clusters"
        );
    }

    /// Decay-on-spend: as an imported coin blends its import tag down toward
    /// background (the existing value-weighted mix), the import floor it faces
    /// falls proportionally, reaching 1x (background) as the import weight hits
    /// zero. Proven here via the consensus floor helper on partially-blended
    /// output tags — no new decay machinery.
    #[test]
    fn import_floor_decays_with_tag_blend_toward_background() {
        use bth_cluster_tax::import_cluster_id_for_height;
        use bth_transaction_types::{ClusterId, ClusterTagEntry};

        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let import_id =
            import_cluster_id_for_height(7 * bth_cluster_tax::BRIDGE_IMPORT_EPOCH_BLOCKS).0;
        ledger
            .record_bridge_import_cluster_for_test(import_id)
            .unwrap();

        // 100% import-tagged => floor is exactly F.
        let full = vec![mk_tagged_output(
            100_000_000,
            81,
            ClusterTagVector::from_pairs(&[(ClusterId(import_id), TAG_WEIGHT_SCALE)]),
        )];
        let full_floor = ledger.import_floor_factor_from_outputs(&full).unwrap();
        assert_eq!(full_floor, 1_500, "100% import weight floors at F=1.5x");

        // 50% import weight (blended with background) => floor blends halfway
        // between F (1500) and background (1000) = 1250.
        let mut half_tags = ClusterTagVector::empty();
        half_tags.entries.push(ClusterTagEntry {
            cluster_id: ClusterId(import_id),
            weight: TAG_WEIGHT_SCALE / 2,
        });
        let half = vec![mk_tagged_output(100_000_000, 82, half_tags)];
        let half_floor = ledger.import_floor_factor_from_outputs(&half).unwrap();
        assert_eq!(
            half_floor, 1_250,
            "50% import weight must blend the floor halfway to background (got {half_floor})"
        );

        // 0% import weight (fully blended to background) => floor is 1x: the
        // imported coin is now as cheap as domestically-circulated money.
        let bg = vec![mk_tagged_output(100_000_000, 83, ClusterTagVector::empty())];
        let bg_floor = ledger.import_floor_factor_from_outputs(&bg).unwrap();
        assert_eq!(
            bg_floor,
            bth_cluster_tax::ClusterFactorCurve::FACTOR_SCALE,
            "a fully-circulated coin faces no import floor (1x)"
        );
    }

    /// A pure-external hold — an import-tagged coin that never blends in
    /// domestic value — stays at >=F. Its tag never shifts, so the floor
    /// never falls.
    #[test]
    fn import_pure_external_hold_stays_at_f() {
        use bth_cluster_tax::import_cluster_id_for_height;
        use bth_transaction_types::ClusterId;

        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let import_id =
            import_cluster_id_for_height(9 * bth_cluster_tax::BRIDGE_IMPORT_EPOCH_BLOCKS).0;
        ledger
            .record_bridge_import_cluster_for_test(import_id)
            .unwrap();

        // A never-mixing coin keeps its 100% import tag through any number of
        // self-spends; the floor helper still returns exactly F for it.
        let held = vec![mk_tagged_output(
            50_000_000,
            84,
            ClusterTagVector::from_pairs(&[(ClusterId(import_id), TAG_WEIGHT_SCALE)]),
        )];
        for _ in 0..5 {
            let f = ledger.import_floor_factor_from_outputs(&held).unwrap();
            assert_eq!(f, 1_500, "a pure-external hold must stay at >=F=1.5x");
        }
    }

    /// The >=F import floor and the ring-centroid floor compose without a
    /// double-floor: the consensus fee floor uses `max(claimed, ring_centroid,
    /// import_floor)`, so a wealthy-ring demurrage factor already above F is
    /// UNCHANGED by the import floor, and a background claim with an import tag
    /// is raised to F (not stacked on top of the ring floor).
    #[test]
    fn import_floor_composes_with_ring_centroid_no_double_floor() {
        use bth_cluster_tax::{import_cluster_id_for_height, PICO_PER_BTH};
        use bth_transaction_types::ClusterId;

        let dir = tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();

        // Demurrage active (past halving) so factors actually price.
        let block_height = 8_000_000u64;
        let import_id = import_cluster_id_for_height(block_height).0;
        ledger
            .record_bridge_import_cluster_for_test(import_id)
            .unwrap();

        // CASE A: background ring + a 100% import-tagged output, small import
        // wealth. The ring implies ~1x (no wealthy cluster), the claimed factor
        // is ~1x, but the import floor raises the demurrage factor to F. The
        // floor must therefore EXCEED the identical tx with a non-import tag.
        let ring_bg = seed_ring_utxos(&ledger, &[(100_000_000, 0, 0, 90)]);
        let import_out = vec![mk_tagged_output(
            90_000_000,
            91,
            ClusterTagVector::from_pairs(&[(ClusterId(import_id), TAG_WEIGHT_SCALE)]),
        )];
        // Keep the import cluster's global wealth tiny so curve(wealth) < F and
        // the floor is what binds (not the curve).
        ledger
            .set_cluster_wealth_for_test(import_id, 1_000 * PICO_PER_BTH)
            .unwrap();
        let tx_import = mk_transfer_tx(ring_bg.clone(), import_out, 0);
        let floor_import = ledger
            .consensus_fee_floor(&tx_import, block_height)
            .unwrap();

        // Same-shape tx whose output is a non-import cluster of equal tiny wealth
        // (so its claimed factor is also ~1x) — no import floor applies.
        let plain_cluster = 424242u64;
        ledger
            .set_cluster_wealth_for_test(plain_cluster, 1_000 * PICO_PER_BTH)
            .unwrap();
        let plain_out = vec![mk_tagged_output(
            90_000_000,
            92,
            ClusterTagVector::from_pairs(&[(ClusterId(plain_cluster), TAG_WEIGHT_SCALE)]),
        )];
        let tx_plain = mk_transfer_tx(ring_bg, plain_out, 0);
        let floor_plain = ledger.consensus_fee_floor(&tx_plain, block_height).unwrap();
        assert!(
            floor_import > floor_plain,
            "the import floor must raise a background-priced import above an \
             equivalent non-import tx (import={floor_import}, plain={floor_plain})"
        );

        // CASE B: a WEALTHY ring already drives the demurrage factor above F.
        // Adding the import tag must NOT stack another floor on top — the
        // effective factor is the single dominating max, so the floor equals a
        // recomputation where import_floor <= ring_centroid and is absorbed.
        ledger
            .set_cluster_wealth_for_test(5, 10_000_000_000_000_000_000)
            .unwrap();
        let rich_ring =
            seed_ring_utxos(&ledger, &[(100_000_000, 0, 5, 93), (100_000_000, 0, 5, 94)]);
        // Output tagged to the (low-wealth) import cluster: its own claimed
        // factor is ~1x but the rich ring floors far above F. The import floor
        // (F=1.5x) is BELOW the ring-centroid floor here, so it must be absorbed
        // by the max — the presence of the import tag cannot raise the floor
        // beyond what the wealthy ring already implies.
        let out_rich = vec![mk_tagged_output(
            90_000_000,
            95,
            ClusterTagVector::from_pairs(&[(ClusterId(import_id), TAG_WEIGHT_SCALE)]),
        )];
        let tx_rich_import = mk_transfer_tx(rich_ring.clone(), out_rich, 0);
        let floor_rich_import = ledger
            .consensus_fee_floor(&tx_rich_import, block_height)
            .unwrap();

        // Same wealthy ring, same output but tagged to a DISTINCT non-import
        // cluster of equally-tiny wealth: the ring-centroid floor is identical,
        // and since the import floor (1.5x) is below that ring floor, the two
        // fee floors MUST be equal — proving no double-floor stacking.
        let plain2 = 555555u64;
        ledger
            .set_cluster_wealth_for_test(plain2, 1_000 * PICO_PER_BTH)
            .unwrap();
        let out_rich_plain = vec![mk_tagged_output(
            90_000_000,
            96,
            ClusterTagVector::from_pairs(&[(ClusterId(plain2), TAG_WEIGHT_SCALE)]),
        )];
        let tx_rich_plain = mk_transfer_tx(rich_ring, out_rich_plain, 0);
        let floor_rich_plain = ledger
            .consensus_fee_floor(&tx_rich_plain, block_height)
            .unwrap();
        assert_eq!(
            floor_rich_import, floor_rich_plain,
            "when the ring-centroid floor already exceeds F, the import floor must be \
             absorbed by the single max (no double-floor): import={floor_rich_import}, \
             plain={floor_rich_plain}"
        );
    }
}
