//! Authenticated detached native-wallet witness (#1499, #1286 P10).
//!
//! A detached native wallet (no `Ledger`, no LMDB) recovers and spends its
//! outputs, including V2 lottery payouts with their derivation contexts, from
//! UNTRUSTED witness bytes plus a [`TrustedCheckpoint`] it obtained SEPARATELY.
//! Inactive: no RPC route, no network message, no consensus change.
//!
//! # Trust boundary
//!
//! The caller supplies `(network, height, accepted block hash)` through its own
//! trusted chain/checkpoint mechanism. The witness never carries, selects or
//! implies its own anchor: matching a body against a header that arrived WITH
//! the response proves nothing, so the only header hash that counts is the
//! anchor's. [`TrustedCheckpoint`] therefore has no `Deserialize` and is never
//! encoded into a witness. [`Ledger::accepted_checkpoint`] is the offline
//! local integration (a node reading its own accepted tip, in a separate call
//! from the export); a remote client must authenticate the same three fields
//! itself.
//!
//! A successful [`VerifiedSnapshot::verify`] proves that every byte it uses is
//! committed by the anchored block hash (inclusion). It does NOT replay
//! consensus (PoW, draw eligibility, signatures, monetary rules): the anchor
//! being an accepted block is the caller's obligation. It also proves nothing
//! about CURRENT state past the anchor: spends after it, and the mempool, are
//! invisible. Spent/decoy facts are "as of the anchor"; signing requires the
//! caller's current trusted anchor to equal the snapshot's, and the final
//! ledger validation of the resulting transaction remains the authority.
//!
//! # What the witness commits (production 7.0.0 rules, no new commitments)
//!
//! Each block's header hash commits `tx_root`, which under protocol 7.0.0 is
//! the V2 body root ([`lottery_v2::root`]) over:
//! - the minting tx hash (every `MintingTx` field),
//! - `Block::compute_tx_root` over `Transaction::hash`, which since #1638
//!   commits every input key image and every field of every output (amount,
//!   keys, memo, cluster tags, ML-KEM ciphertext), fee, height and settlement,
//! - the V2 payout root over every payout `Record` (winner, amount, target,
//!   envelope and derivation context),
//! - the lottery summary.
//!
//! Rings and CLSAG signatures are NOT committed by `Transaction::hash`, so the
//! witness does not carry them: a witness transaction is the hash projection
//! (key images + complete outputs + fee/height/settlement) and every byte in
//! the witness is authenticated. The one alias left by the hash layouts is
//! the minting ciphertext (`None` and `Some(empty)` hash identically); the
//! witness admits only `None` or exactly [`v2::KEM_BYTES`].
//!
//! The earlier prototype (branch `backup/issue-1499-stale-20260930`) needed a
//! private full-payload `ordinary_root` because the pre-#1638 transaction hash
//! omitted output fields; the production rule makes that unnecessary, so this
//! module reuses the production body root unchanged.
//!
//! # Codec and bounds (local transport policy, not consensus)
//!
//! `MAGIC` (8 bytes: `BTHWIT`, lottery rules 2, witness format 1), u32 LE
//! block count (= anchor height + 1), then per block: u32 LE length +
//! canonical bincode (fixint, LE) of the block projection, u8 record count
//! (<= `CANDIDATE_MAX_AWARDS`), and per record u16 LE length + the existing
//! canonical `Record::encode` bytes. All bytes must be consumed, every entry
//! must re-encode byte-identically, and every length is checked against its
//! bound and the remaining input BEFORE allocation or decoding.
//!
//! The witness is the complete history from genesis to the anchor: lineage of
//! repeated/nested payouts, historical spent key images and the decoy set are
//! derived only from authenticated bodies, never asserted. That is
//! deliberately non-scalable; oversized chains fail closed. Scalable
//! authenticated sync, versioned snapshots and RPC/client integration remain
//! parent (#1286) work.

// Inactive (#1499): consumed by tests until a production client integration
// (parent #1286) calls it. Kept compiled so it cannot rot against the rules.
#![cfg_attr(not(test), allow(dead_code))]

use super::{wallet::recover_owned, *};
use crate::{
    block::{BlockHeader, BlockLotterySummary, LotteryOutput, MintingTx},
    decoy_selection::{DecoySelectionError, GammaDecoySelector, OutputCandidate},
    ledger::lottery_v2,
    transaction::{ClsagRingInput, SettlementInfo, Transaction, TxInputs, TxOutput},
    wallet::{Wallet, WalletRead},
};
use bincode::Options;
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_ring_signature::KeyImage;
use bth_transaction_clsag::lottery_v2::{
    self as v2, Award, Outpoint, Record, Source, CANDIDATE_MAX_AWARDS,
};
use bth_util_from_random::OsRng;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Witness version: `BTHWIT`, lottery rules 2, witness format 1.
pub(in crate::ledger) const MAGIC: &[u8; 8] = b"BTHWIT\x02\x01";
/// Total witness bytes.
pub(in crate::ledger) const MAX_WITNESS_BYTES: usize = 64 * 1024 * 1024;
/// Blocks including genesis.
pub(in crate::ledger) const MAX_BLOCKS: usize = 16_384;
/// One block projection; the ledger's own stored-block decode bound.
pub(in crate::ledger) const MAX_BLOCK_BYTES: usize = 16 * 1024 * 1024;
/// One payout record (the fixed hybrid record is 1,238 bytes).
pub(in crate::ledger) const MAX_RECORD_BYTES: usize = 2048;
/// Aggregate outputs (coinbase + transaction + payout) across the witness.
pub(in crate::ledger) const MAX_OUTPUTS: usize = 262_144;
/// Aggregate spent key images across the witness.
pub(in crate::ledger) const MAX_KEY_IMAGES: usize = 262_144;
/// Confirmations before an output may be a ring member (the native rule).
const DECOY_CONFIRMATIONS: u64 = 10;

/// Why a witness was refused. Every variant is fail-closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WitnessError {
    /// A byte/count/work bound was exceeded (checked before allocation).
    Bound(&'static str),
    /// Malformed, truncated, trailing, unsupported-version or noncanonical.
    Encoding(&'static str),
    /// The chain does not end at (or begin from) the trusted anchor.
    Anchor(&'static str),
    /// A body component does not match its committed header.
    Body(&'static str),
    /// Lineage/derivation: missing or non-earlier source, record binding,
    /// duplicate outpoint/transaction/key image.
    Lineage(&'static str),
}
impl std::fmt::Display for WitnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "detached witness rejected: {self:?}")
    }
}
impl std::error::Error for WitnessError {}

/// The caller's independently trusted accepted block. Deliberately not
/// `Deserialize` and never part of a witness: see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrustedCheckpoint {
    network: Network,
    height: u64,
    hash: [u8; 32],
}
impl TrustedCheckpoint {
    /// The caller asserts, from its OWN trust source, that `hash` is the
    /// accepted block at `height` on `network`.
    pub(crate) fn new(network: Network, height: u64, hash: [u8; 32]) -> Self {
        Self {
            network,
            height,
            hash,
        }
    }
    pub(crate) fn height(&self) -> u64 {
        self.height
    }
    pub(crate) fn hash(&self) -> [u8; 32] {
        self.hash
    }
    pub(crate) fn network(&self) -> Network {
        self.network
    }
}

/// The hash projection of a transaction: exactly what `Transaction::hash`
/// commits. Rings and signatures are excluded (unauthenticated, unused).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::ledger) struct WitnessTx {
    pub(in crate::ledger) key_images: Vec<[u8; 32]>,
    pub(in crate::ledger) outputs: Vec<TxOutput>,
    pub(in crate::ledger) fee: u64,
    pub(in crate::ledger) created_at_height: u64,
    pub(in crate::ledger) settlement: Option<SettlementInfo>,
}
impl WitnessTx {
    fn project(tx: &Transaction) -> Self {
        Self {
            key_images: tx.key_images(),
            outputs: tx.outputs.clone(),
            fee: tx.fee,
            created_at_height: tx.created_at_height,
            settlement: tx.settlement.clone(),
        }
    }
    /// A transaction with the same `hash()` (rings/signatures emptied). Only
    /// ever used for hashing and output/key-image replay, never verified or
    /// relayed as a transaction.
    fn hashable(&self) -> Transaction {
        Transaction {
            inputs: TxInputs::new(
                self.key_images
                    .iter()
                    .map(|key_image| ClsagRingInput {
                        ring: Vec::new(),
                        key_image: *key_image,
                        commitment_key_image: [0; 32],
                        clsag_signature: Vec::new(),
                        pseudo_output_amount: 0,
                    })
                    .collect(),
            ),
            outputs: self.outputs.clone(),
            fee: self.fee,
            created_at_height: self.created_at_height,
            settlement: self.settlement.clone(),
        }
    }
}

/// One block's committed projection (payouts travel as records).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::ledger) struct WitnessBody {
    pub(in crate::ledger) header: BlockHeader,
    pub(in crate::ledger) minting_tx: MintingTx,
    pub(in crate::ledger) transactions: Vec<WitnessTx>,
    pub(in crate::ledger) lottery_summary: BlockLotterySummary,
}

/// UNAUTHENTICATED decoded witness entry. Private to the ledger: callers only
/// ever see a [`VerifiedSnapshot`]. Exposed to tests for mutation controls.
#[derive(Clone, Debug)]
pub(in crate::ledger) struct UnverifiedEntry {
    pub(in crate::ledger) body: WitnessBody,
    pub(in crate::ledger) records: Vec<Record>,
}
impl UnverifiedEntry {
    fn from_block(block: &Block, records: Vec<Record>) -> Self {
        Self {
            body: WitnessBody {
                header: block.header.clone(),
                minting_tx: block.minting_tx.clone(),
                transactions: block.transactions.iter().map(WitnessTx::project).collect(),
                lottery_summary: block.lottery_summary.clone(),
            },
            records,
        }
    }
    /// The block this entry describes, with payouts rebuilt from records.
    fn block(&self) -> Block {
        Block {
            header: self.body.header.clone(),
            minting_tx: self.body.minting_tx.clone(),
            transactions: self
                .body
                .transactions
                .iter()
                .map(WitnessTx::hashable)
                .collect(),
            lottery_outputs: self
                .records
                .iter()
                .map(|r| LotteryOutput {
                    winner_tx_hash: r.winner.hash,
                    winner_output_index: r.winner.index,
                    payout: r.amount,
                    target_key: r.target,
                    public_key: r.public_key,
                    kem_ciphertext: r.ciphertext.clone(),
                })
                .collect(),
            lottery_summary: self.body.lottery_summary.clone(),
        }
    }
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(MAX_BLOCK_BYTES as u64)
        .reject_trailing_bytes()
}

/// Encode entries into witness bytes (export, and test mutation controls).
pub(in crate::ledger) fn encode(entries: &[UnverifiedEntry]) -> Result<Vec<u8>, WitnessError> {
    if entries.is_empty() || entries.len() > MAX_BLOCKS {
        return Err(WitnessError::Bound("block count"));
    }
    let mut out = MAGIC.to_vec();
    out.extend((entries.len() as u32).to_le_bytes());
    for e in entries {
        let body = codec()
            .serialize(&e.body)
            .map_err(|_| WitnessError::Bound("block bytes"))?;
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(body);
        if e.records.len() > CANDIDATE_MAX_AWARDS {
            return Err(WitnessError::Bound("record count"));
        }
        out.push(e.records.len() as u8);
        for r in &e.records {
            let bytes = r.encode().map_err(|_| WitnessError::Encoding("record"))?;
            out.extend((bytes.len() as u16).to_le_bytes());
            out.extend(bytes);
        }
        if out.len() > MAX_WITNESS_BYTES {
            return Err(WitnessError::Bound("witness bytes"));
        }
    }
    Ok(out)
}

struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WitnessError> {
        let (a, b) = self
            .0
            .split_at_checked(n)
            .ok_or(WitnessError::Encoding("truncated"))?;
        self.0 = b;
        Ok(a)
    }
    fn u32(&mut self) -> Result<u32, WitnessError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u16(&mut self) -> Result<u16, WitnessError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
}

/// Decode WITHOUT authenticating. Bounds every count/length against the
/// trusted height and the remaining input before allocating.
pub(in crate::ledger) fn decode_unverified(
    bytes: &[u8],
    trusted: &TrustedCheckpoint,
) -> Result<Vec<UnverifiedEntry>, WitnessError> {
    if bytes.len() > MAX_WITNESS_BYTES {
        return Err(WitnessError::Bound("witness bytes"));
    }
    if trusted.height >= MAX_BLOCKS as u64 {
        return Err(WitnessError::Bound("anchor height"));
    }
    let mut r = Cursor(bytes);
    if r.take(MAGIC.len())? != MAGIC {
        return Err(WitnessError::Encoding("unsupported witness version"));
    }
    let count = r.u32()? as u64;
    if count != trusted.height + 1 {
        return Err(WitnessError::Anchor("block count is not anchor height + 1"));
    }
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let size = r.u32()? as usize;
        if size > MAX_BLOCK_BYTES {
            return Err(WitnessError::Bound("block bytes"));
        }
        let raw = r.take(size)?;
        let body: WitnessBody = codec()
            .deserialize(raw)
            .map_err(|_| WitnessError::Encoding("block body"))?;
        if codec().serialize(&body).ok().as_deref() != Some(raw) {
            return Err(WitnessError::Encoding("noncanonical block body"));
        }
        let awards = r.take(1)?[0] as usize;
        if awards > CANDIDATE_MAX_AWARDS {
            return Err(WitnessError::Bound("record count"));
        }
        let mut records = Vec::with_capacity(awards);
        for _ in 0..awards {
            let size = r.u16()? as usize;
            if size > MAX_RECORD_BYTES {
                return Err(WitnessError::Bound("record bytes"));
            }
            // `Record::decode` is canonical (re-encodes byte-identically) and
            // rejects unknown context versions and zero/noncanonical tweaks.
            records
                .push(Record::decode(r.take(size)?).map_err(|_| WitnessError::Encoding("record"))?);
        }
        entries.push(UnverifiedEntry { body, records });
    }
    if !r.0.is_empty() {
        return Err(WitnessError::Encoding("trailing bytes"));
    }
    Ok(entries)
}

/// The only product of a witness: every output with its committed derivation
/// context, the spent key images and the canonical target index, all as of
/// the anchor. Holds no store or view handle.
pub(crate) struct VerifiedSnapshot {
    checkpoint: TrustedCheckpoint,
    outputs: BTreeMap<[u8; 36], (Utxo, StoredContext)>,
    /// Payout outpoint -> winning outpoint.
    winners: BTreeMap<[u8; 36], UtxoId>,
    /// First outpoint written for each target key (the ledger's address-index
    /// rule): only it is ownable/spendable or a decoy.
    canonical: BTreeMap<[u8; 32], [u8; 36]>,
    /// Spent key image -> spending height.
    spent: BTreeMap<[u8; 32], u64>,
}

/// One output this wallet owns, discovered from a verified snapshot.
#[derive(Clone, Debug)]
pub(crate) struct DetachedOutput {
    pub(crate) utxo: Utxo,
    pub(crate) subaddress: u64,
    pub(crate) key_image: [u8; 32],
    /// Spent at this height AS OF THE ANCHOR (later spends are invisible).
    pub(crate) spent_at: Option<u64>,
    /// The ORIGINAL derivation index (`Context::base_index`), distinct from
    /// `utxo.id.output_index` for a payout.
    pub(crate) base_index: u32,
    /// The winning outpoint, for a V2 lottery payout.
    pub(crate) winner: Option<UtxoId>,
    pub(crate) checkpoint: TrustedCheckpoint,
}

impl VerifiedSnapshot {
    /// Authenticate `bytes` against the caller's `trusted` anchor, then replay
    /// every output and context chronologically.
    pub(crate) fn verify(bytes: &[u8], trusted: &TrustedCheckpoint) -> Result<Self, WitnessError> {
        let entries = decode_unverified(bytes, trusted)?;
        let genesis = Block::genesis_for_network(trusted.network);
        let genesis_hash = genesis.hash();

        // Phase 1: authenticate every byte to the anchor. Aggregate work is
        // bounded before any lineage derivation runs.
        let (mut outputs, mut images) = (0usize, 0usize);
        let mut blocks = Vec::with_capacity(entries.len());
        let mut parent = [0u8; 32];
        for (height, entry) in entries.iter().enumerate() {
            let body = &entry.body;
            outputs = outputs
                .saturating_add(1 + entry.records.len())
                .saturating_add(body.transactions.iter().map(|t| t.outputs.len()).sum());
            images =
                images.saturating_add(body.transactions.iter().map(|t| t.key_images.len()).sum());
            if outputs > MAX_OUTPUTS || images > MAX_KEY_IMAGES {
                return Err(WitnessError::Bound("aggregate outputs/key images"));
            }
            if body
                .minting_tx
                .kem_ciphertext
                .as_ref()
                .is_some_and(|ct| ct.len() != v2::KEM_BYTES)
            {
                return Err(WitnessError::Encoding("noncanonical minting ciphertext"));
            }
            let block = entry.block();
            if block.height() != height as u64 {
                return Err(WitnessError::Body("height sequence"));
            }
            if height == 0 {
                if block.hash() != genesis_hash
                    || codec().serialize(&body).ok()
                        != codec()
                            .serialize(&UnverifiedEntry::from_block(&genesis, vec![]).body)
                            .ok()
                    || !entry.records.is_empty()
                {
                    return Err(WitnessError::Anchor("not the network's canonical genesis"));
                }
            } else {
                if block.header.prev_block_hash != parent {
                    return Err(WitnessError::Body("parent hash"));
                }
                lottery_v2::bind_outputs(&block, &entry.records)
                    .map_err(|_| WitnessError::Body("payout records"))?;
                let root = lottery_v2::root(&block, &entry.records)
                    .map_err(|_| WitnessError::Body("payout records"))?;
                if root != block.header.tx_root {
                    return Err(WitnessError::Body("V2 body root"));
                }
            }
            parent = block.hash();
            blocks.push(block);
        }
        if parent != trusted.hash {
            return Err(WitnessError::Anchor("chain does not end at the anchor"));
        }

        // Phase 2: replay outputs in the production writer's order (coinbase,
        // transaction outputs, payouts). Every payout must resolve an already
        // replayed, strictly older source, which rules out missing edges and
        // cycles without recursion.
        let mut snapshot = Self {
            checkpoint: *trusted,
            outputs: BTreeMap::new(),
            winners: BTreeMap::new(),
            canonical: BTreeMap::new(),
            spent: BTreeMap::new(),
        };
        let mut tx_ids = BTreeSet::new();
        for (block, entry) in blocks.iter().zip(&entries).skip(1) {
            let (hash, height) = (block.hash(), block.height());
            snapshot.insert(
                Utxo {
                    id: UtxoId::new(hash, 0),
                    output: block.minting_tx.to_tx_output(),
                    created_at: height,
                },
                StoredContext::Direct {
                    height,
                    base_index: 0,
                },
            )?;
            for tx in &block.transactions {
                let id = tx.hash();
                if !tx_ids.insert(id) {
                    return Err(WitnessError::Lineage("duplicate transaction"));
                }
                for image in tx.key_images() {
                    if snapshot.spent.insert(image, height).is_some() {
                        return Err(WitnessError::Lineage("duplicate key image"));
                    }
                }
                for (index, output) in tx.outputs.iter().enumerate() {
                    snapshot.insert(
                        Utxo {
                            id: UtxoId::new(id, index as u32),
                            output: output.clone(),
                            created_at: height,
                        },
                        StoredContext::Direct {
                            height,
                            base_index: index as u32,
                        },
                    )?;
                }
            }
            let awards: Vec<Award> = entry
                .records
                .iter()
                .map(|r| Award {
                    winner: r.winner.clone(),
                    amount: r.amount,
                })
                .collect();
            let manifest =
                v2::manifest(&awards).map_err(|_| WitnessError::Lineage("award manifest"))?;
            for (ordinal, record) in entry.records.iter().enumerate() {
                let winner = UtxoId::new(record.winner.hash, record.winner.index);
                let (source, context) = snapshot
                    .outputs
                    .get(&winner.to_bytes())
                    .ok_or(WitnessError::Lineage("missing payout source"))?;
                if source.created_at >= height {
                    return Err(WitnessError::Lineage("payout source not strictly older"));
                }
                let domain =
                    lottery_v2::domain_for(genesis_hash, block, manifest, ordinal, record.amount);
                record
                    .validate(
                        &Source {
                            outpoint: Outpoint {
                                hash: winner.tx_hash,
                                index: winner.output_index,
                            },
                            target: source.output.target_key,
                            context: context.derivation(),
                        },
                        &source.output,
                        &domain,
                    )
                    .map_err(|_| WitnessError::Lineage("payout record derivation"))?;
                let output =
                    block.lottery_outputs[ordinal].to_tx_output(source.output.cluster_tags.clone());
                let id = UtxoId::new(hash, ordinal as u32 + 1);
                snapshot.insert(
                    Utxo {
                        id,
                        output,
                        created_at: height,
                    },
                    StoredContext::Lottery {
                        height,
                        ordinal: ordinal as u32,
                        context: record.context.clone(),
                    },
                )?;
                snapshot.winners.insert(id.to_bytes(), winner);
            }
        }
        Ok(snapshot)
    }

    fn insert(&mut self, utxo: Utxo, context: StoredContext) -> Result<(), WitnessError> {
        let key = utxo.id.to_bytes();
        self.canonical.entry(utxo.output.target_key).or_insert(key);
        if self.outputs.insert(key, (utxo, context)).is_some() {
            return Err(WitnessError::Lineage("duplicate outpoint"));
        }
        Ok(())
    }

    fn is_canonical(&self, utxo: &Utxo) -> bool {
        self.canonical.get(&utxo.output.target_key) == Some(&utxo.id.to_bytes())
    }

    /// Every output `wallet` owns as of the anchor (spent or not), through the
    /// shared native recovery rule (`recover_owned`, ending in the `x·G = P`
    /// check) on the VERIFIED contexts. No store is consulted.
    pub(crate) fn discover(&self, wallet: &Wallet) -> Vec<DetachedOutput> {
        let mut owned = Vec::new();
        for (key, (utxo, context)) in &self.outputs {
            if !self.is_canonical(utxo) {
                continue;
            }
            let Ok((subaddress, secret)) = recover_owned(wallet, &utxo.output, context) else {
                continue;
            };
            let key_image = *KeyImage::from(&secret).as_bytes();
            owned.push(DetachedOutput {
                utxo: utxo.clone(),
                subaddress,
                key_image,
                spent_at: self.spent.get(&key_image).copied(),
                base_index: context.derivation().base_index,
                winner: self.winners.get(key).copied(),
                checkpoint: self.checkpoint,
            });
        }
        owned
    }

    /// Sign a spend of verified, discovered `inputs` with the shared native
    /// constructor (`Wallet::create_private_transaction_impl`: ring selection,
    /// tag inheritance, signing hash, CLSAG). `current` is the caller's
    /// CURRENT trusted anchor and must equal this snapshot's; it cannot reveal
    /// spends after the anchor, so the final ledger validation remains the
    /// authority on current unspentness.
    pub(crate) fn create_transaction(
        &self,
        wallet: &Wallet,
        current: &TrustedCheckpoint,
        inputs: &[DetachedOutput],
        outputs: Vec<TxOutput>,
        fee: u64,
    ) -> anyhow::Result<Transaction> {
        anyhow::ensure!(
            *current == self.checkpoint,
            "verified snapshot is stale or for another chain"
        );
        for input in inputs {
            anyhow::ensure!(
                input.checkpoint == self.checkpoint,
                "input discovered under another anchor"
            );
        }
        let utxos: Vec<Utxo> = inputs.iter().map(|i| i.utxo.clone()).collect();
        wallet.create_private_transaction_impl(
            &utxos,
            outputs,
            fee,
            self.checkpoint.height,
            self,
            None,
        )
    }
}

impl WalletRead for VerifiedSnapshot {
    fn validate_input(&self, utxo: &Utxo) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.is_canonical(utxo),
            "selected input is not the canonical verified output"
        );
        Ok(())
    }
    fn recover_input(&self, wallet: &Wallet, claimed: &Utxo) -> anyhow::Result<RistrettoPrivate> {
        let (utxo, context) = self
            .outputs
            .get(&claimed.id.to_bytes())
            .ok_or_else(|| anyhow::anyhow!("input is not in the verified snapshot"))?;
        anyhow::ensure!(
            bincode::serialize(claimed)? == bincode::serialize(utxo)?,
            "input differs from the verified output"
        );
        let (_, secret) = recover_owned(wallet, &utxo.output, context)
            .map_err(|e| anyhow::anyhow!("input does not belong to this wallet ({e:?})"))?;
        anyhow::ensure!(
            !self.spent.contains_key(KeyImage::from(&secret).as_bytes()),
            "input is already spent as of the anchor"
        );
        Ok(secret)
    }
    fn decoys(
        &self,
        count: usize,
        exclude: &[[u8; 32]],
        real_age: u64,
        selector: &GammaDecoySelector,
        rng: &mut OsRng,
    ) -> Result<Vec<TxOutput>, LedgerError> {
        let max_height = self.checkpoint.height.saturating_sub(DECOY_CONFIRMATIONS);
        let candidates: Vec<OutputCandidate> = self
            .outputs
            .values()
            .filter(|(u, _)| {
                u.created_at <= max_height
                    && !exclude.contains(&u.output.target_key)
                    && self.is_canonical(u)
            })
            .map(|(u, _)| OutputCandidate::from_utxo(u, self.checkpoint.height))
            .collect();
        selector
            .select_decoys_for_input(&candidates, count, exclude, real_age, rng)
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
}

impl Ledger {
    /// Offline local integration ONLY: this node's own accepted tip as an
    /// anchor, read in a call separate from [`Ledger::export_wallet_witness`].
    /// A remote consumer must obtain the same fields from its own trust source.
    pub(crate) fn accepted_checkpoint(&self) -> Result<TrustedCheckpoint, LedgerError> {
        let state = self.get_chain_state()?;
        Ok(TrustedCheckpoint::new(
            self.network,
            state.height,
            state.tip_hash,
        ))
    }

    /// Export UNTRUSTED witness bytes for blocks `0..=height` from accepted V2
    /// state, in one pinned read. Payout records are rebuilt from each block's
    /// payouts and the persisted (strictly read) derivation contexts, and must
    /// reproduce the committed body root. Never emits a trust anchor.
    pub(crate) fn export_wallet_witness(&self, height: u64) -> Result<Vec<u8>, LedgerError> {
        let bound =
            |what: String| LedgerError::StorageEncoding(format!("wallet witness bound: {what}"));
        if height >= MAX_BLOCKS as u64 {
            return Err(bound("height".into()));
        }
        let txn = self.env.read_txn().map_err(db_op("witness read txn"))?;
        let tip = u64::from_le_bytes(lottery_v2::fixed(
            self.meta_db
                .get(&txn, META_HEIGHT)
                .map_err(db_op("witness height"))?
                .ok_or_else(|| LedgerError::InconsistentRecord("missing height".into()))?,
        )?);
        if height > tip {
            return Err(LedgerError::InvalidBlock(format!(
                "witness height {height} is above the accepted tip {tip}"
            )));
        }
        let mut entries = Vec::with_capacity(height as usize + 1);
        for h in 0..=height {
            let block: Block = crate::ledger::experimental::decode(
                self.blocks_db
                    .get(&txn, &h)
                    .map_err(db_op("witness block"))?
                    .ok_or_else(|| LedgerError::InconsistentRecord("missing block".into()))?,
            )?;
            let hash = block.hash();
            let mut records = Vec::with_capacity(block.lottery_outputs.len());
            for (ordinal, o) in block.lottery_outputs.iter().enumerate() {
                let id = UtxoId::new(hash, ordinal as u32 + 1);
                let context = match read_context(self, &txn, &id)?.1 {
                    StoredContext::Lottery {
                        ordinal: stored,
                        context,
                        ..
                    } if stored as usize == ordinal => context,
                    _ => {
                        return Err(LedgerError::InconsistentRecord(
                            "payout without its lottery context".into(),
                        ))
                    }
                };
                records.push(Record {
                    ordinal: ordinal as u32,
                    winner: Outpoint {
                        hash: o.winner_tx_hash,
                        index: o.winner_output_index,
                    },
                    amount: o.payout,
                    target: o.target_key,
                    public_key: o.public_key,
                    ciphertext: o.kem_ciphertext.clone(),
                    context,
                });
            }
            if h > 0 && lottery_v2::root(&block, &records)? != block.header.tx_root {
                return Err(LedgerError::InconsistentRecord(
                    "stored payouts/contexts do not reproduce the body root".into(),
                ));
            }
            entries.push(UnverifiedEntry::from_block(&block, records));
        }
        encode(&entries).map_err(|e| bound(format!("{e:?}")))
    }
}

#[cfg(test)]
mod tests;
