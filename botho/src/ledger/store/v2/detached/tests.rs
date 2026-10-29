//! Detached native-wallet witness (#1499): authenticated export -> detached
//! verification -> recovery -> shared native signing -> accepted ledger spend,
//! plus unsigned local trust-anchor, codec, body and lineage controls.
//!
//! The fast fixture's ordinary blocks are sealed by the V2 producer and
//! accepted by `add_block`. Its three AWARD blocks are written through the
//! production writer with honest records (`v2::derive` over the persisted
//! source and context, the shared `lottery_v2::domain`, the real V2 body
//! root), but their awards are chosen, not drawn: no consensus draw can award
//! before the 720-block lottery maturity. They isolate lineage (repeated,
//! nested twice, nonzero original index, source spent before its award).
//! Every SPEND of those payouts is then signed detached and accepted by the
//! real `add_block`. The natural producer -> validator path at real maturity
//! is the ignored `detached_wallet_spends_natural_payout_at_real_maturity`.
use super::*;
use crate::{
    block::calculate_block_reward, ledger::lottery_v2::DerivationReads,
    transaction::PICOCREDITS_PER_CREDIT,
};
use bth_transaction_types::ClusterTagVector;
use std::collections::BTreeSet;
use tempfile::TempDir;

const OWNER_WORDS: &str = super::super::wallet::fixture::OWNER_WORDS;
const OTHER_WORDS: &str = super::super::wallet::fixture::OTHER_WORDS;
const FEE: u64 = PICOCREDITS_PER_CREDIT;

fn template(ledger: &Ledger, minter: &Wallet, txs: Vec<Transaction>) -> Block {
    let state = ledger.get_chain_state().unwrap();
    let tip = ledger.get_tip().unwrap();
    let mut block = Block::new_template_with_txs(
        &tip,
        &minter.default_address(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        txs,
    );
    block.header.timestamp = block.header.timestamp.max(state.tip_timestamp);
    block.minting_tx.timestamp = block.header.timestamp;
    block
}

fn sealed(ledger: &Ledger, minter: &Wallet, txs: Vec<Transaction>) -> Block {
    let mut block = ledger.seal_block(template(ledger, minter, txs)).unwrap();
    while !block.header.is_valid_pow() {
        block.header.nonce += 1;
    }
    block.minting_tx.nonce = block.header.nonce;
    block
}

/// V2 producer -> V2 validator.
fn mine(ledger: &Ledger, minter: &Wallet, txs: Vec<Transaction>) -> Block {
    let block = sealed(ledger, minter, txs);
    ledger
        .add_block(&block)
        .expect("validator accepts the block");
    block
}

/// A writer-applied award block with honest V2 records (see module docs).
fn award_block(ledger: &Ledger, minter: &Wallet, awards: &[(UtxoId, u64)]) -> Block {
    let mut block = template(ledger, minter, vec![]);
    let txn = ledger.env.read_txn().unwrap();
    let view = ledger.production_view(&txn);
    let list: Vec<Award> = awards
        .iter()
        .map(|(id, amount)| Award {
            winner: Outpoint {
                hash: id.tx_hash,
                index: id.output_index,
            },
            amount: *amount,
        })
        .collect();
    let manifest = v2::manifest(&list).unwrap();
    let mut records = Vec::new();
    for (ordinal, (id, amount)) in awards.iter().enumerate() {
        let (source_utxo, context) = view.output_context(id).unwrap();
        let source = Source {
            outpoint: list[ordinal].winner.clone(),
            target: source_utxo.output.target_key,
            context: context.derivation(),
        };
        let domain = lottery_v2::domain(&view, &block, manifest, ordinal, *amount).unwrap();
        let derived = v2::derive(&source, &domain).unwrap();
        let record = Record {
            ordinal: ordinal as u32,
            winner: source.outpoint.clone(),
            amount: *amount,
            target: derived.target,
            public_key: source_utxo.output.public_key,
            ciphertext: source_utxo.output.kem_ciphertext.clone(),
            context: derived.context,
        };
        record
            .validate(&source, &source_utxo.output, &domain)
            .unwrap();
        records.push(record);
    }
    drop(txn);
    let entry = UnverifiedEntry {
        body: UnverifiedEntry::from_block(&block, vec![]).body,
        records: records.clone(),
    };
    block.lottery_outputs = entry.block().lottery_outputs;
    block.lottery_summary = BlockLotterySummary {
        total_fees: 0,
        pool_distributed: awards.iter().map(|(_, a)| a).sum(),
        amount_burned: 0,
        lottery_seed: [0x5e; 32],
    };
    block.header.tx_root = lottery_v2::root(&block, &records).unwrap();
    let pool = ledger.get_lottery_pool().unwrap();
    ledger
        .apply_writer_effects_for_test(&block, &records, pool, None)
        .unwrap();
    block
}

struct Chain {
    ledger: Ledger,
    owner: Wallet,
    other: Wallet,
    /// Owner coinbase at height 1, SPENT at `spend_height` before its award.
    spent_source: UtxoId,
    spend_height: u64,
    /// Owner hybrid output at original index 1.
    indexed_source: UtxoId,
    /// (payout, winner, original base index, amount).
    payouts: Vec<(UtxoId, UtxoId, u32, u64)>,
    _dir: TempDir,
}

impl Chain {
    fn build() -> Self {
        let dir = TempDir::new().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        ledger.set_difficulty(u64::MAX).unwrap();
        let owner = Wallet::from_mnemonic(OWNER_WORDS).unwrap();
        let other = Wallet::from_mnemonic(OTHER_WORDS).unwrap();
        let mut blocks = Vec::new();
        // Enough mature canonical outputs for a 20-member ring.
        for _ in 0..34 {
            blocks.push(mine(&ledger, &owner, vec![]));
        }
        // A real accepted spend of the height-1 coinbase, paying the owner at
        // output index 1 (a nonzero original derivation index).
        let spent_source = UtxoId::new(blocks[0].hash(), 0);
        let input = ledger.get_utxo(&spent_source).unwrap().unwrap();
        let to_other = 5 * PICOCREDITS_PER_CREDIT;
        let outputs = vec![
            TxOutput::new(to_other, &other.default_address()),
            TxOutput::new_hybrid_to_address(
                input.output.amount - to_other - FEE,
                &owner.default_address(),
                1,
                None,
                ClusterTagVector::empty(),
            )
            .unwrap(),
        ];
        let height = ledger.get_chain_state().unwrap().height;
        let tx = owner
            .create_private_transaction(&[input], outputs, FEE, height, &ledger)
            .unwrap();
        let indexed_source = UtxoId::new(tx.hash(), 1);
        let spend_block = mine(&ledger, &other, vec![tx]);

        // Awards: to the already spent coinbase, twice to the index-1 output
        // (repeated), then a payout winning again, twice (nested).
        let credit = PICOCREDITS_PER_CREDIT;
        let a1 = award_block(
            &ledger,
            &other,
            &[
                (spent_source, 3 * credit),
                (indexed_source, 4 * credit),
                (indexed_source, 5 * credit),
            ],
        );
        let p = |b: &Block, o: u32| UtxoId::new(b.hash(), o + 1);
        let a2 = award_block(&ledger, &other, &[(p(&a1, 1), 6 * credit)]);
        let a3 = award_block(&ledger, &other, &[(p(&a2, 0), 7 * credit)]);
        let payouts = vec![
            (p(&a1, 0), spent_source, 0, 3 * credit),
            (p(&a1, 1), indexed_source, 1, 4 * credit),
            (p(&a1, 2), indexed_source, 1, 5 * credit),
            (p(&a2, 0), p(&a1, 1), 1, 6 * credit),
            (p(&a3, 0), p(&a2, 0), 1, 7 * credit),
        ];
        // Ten confirmations for the newest payout.
        for _ in 0..10 {
            mine(&ledger, &other, vec![]);
        }
        Self {
            ledger,
            owner,
            other,
            spent_source,
            spend_height: spend_block.height(),
            indexed_source,
            payouts,
            _dir: dir,
        }
    }

    fn witness(&self) -> (TrustedCheckpoint, Vec<u8>) {
        let anchor = self.ledger.accepted_checkpoint().unwrap();
        let bytes = self.ledger.export_wallet_witness(anchor.height()).unwrap();
        (anchor, bytes)
    }

    fn spend(
        &self,
        snapshot: &VerifiedSnapshot,
        anchor: &TrustedCheckpoint,
        input: &DetachedOutput,
    ) -> anyhow::Result<Transaction> {
        let output = TxOutput::new(
            input.utxo.output.amount - FEE,
            &self.other.default_address(),
        );
        snapshot.create_transaction(
            &self.owner,
            anchor,
            std::slice::from_ref(input),
            vec![output],
            FEE,
        )
    }
}

fn by_id(owned: Vec<DetachedOutput>) -> BTreeMap<[u8; 36], DetachedOutput> {
    owned
        .into_iter()
        .map(|o| (o.utxo.id.to_bytes(), o))
        .collect()
}

/// Detached discovery must equal the authoritative native discovery exactly.
fn assert_matches_ledger(chain: &Chain, snapshot: &VerifiedSnapshot) {
    let detached = by_id(snapshot.discover(&chain.owner));
    let native = chain.ledger.discover_wallet_outputs(&chain.owner).unwrap();
    assert_eq!(detached.len(), native.len());
    for n in native {
        let d = &detached[&n.utxo.id.to_bytes()];
        assert_eq!(
            bincode::serialize(&d.utxo).unwrap(),
            bincode::serialize(&n.utxo).unwrap()
        );
        assert_eq!(
            (d.key_image, d.spent_at, d.subaddress),
            (n.key_image, n.spent_at, n.subaddress)
        );
        assert_eq!(d.winner.is_some(), n.lottery_payout);
    }
}

#[test]
fn detached_wallet_recovers_and_spends_repeated_nested_payouts() {
    let chain = Chain::build();
    let (anchor, bytes) = chain.witness();
    let snapshot = VerifiedSnapshot::verify(&bytes, &anchor).unwrap();
    assert_matches_ledger(&chain, &snapshot);
    let owned = by_id(snapshot.discover(&chain.owner));

    // The source was spent before its award; the award is independent.
    let source = &owned[&chain.spent_source.to_bytes()];
    assert_eq!(source.spent_at, Some(chain.spend_height));
    // Winning outpoint, payout outpoint and original index stay separate.
    for (payout, winner, base_index, amount) in &chain.payouts {
        let o = &owned[&payout.to_bytes()];
        assert_eq!(o.winner, Some(*winner));
        assert_eq!(o.base_index, *base_index);
        assert_eq!(o.utxo.output.amount, *amount);
        assert_eq!(o.spent_at, None);
        assert_ne!(o.key_image, owned[&winner.to_bytes()].key_image);
    }
    assert_eq!(owned[&chain.indexed_source.to_bytes()].base_index, 1);
    let images: BTreeSet<_> = owned.values().map(|o| o.key_image).collect();
    assert_eq!(images.len(), owned.len());
    // Another wallet owns none of them.
    let foreign = by_id(snapshot.discover(&chain.other));
    for (payout, ..) in &chain.payouts {
        assert!(!foreign.contains_key(&payout.to_bytes()));
    }
    // The spent source cannot be signed.
    let err = chain.spend(&snapshot, &anchor, source).unwrap_err();
    assert!(err.to_string().contains("already spent"), "{err}");

    // Detached-sign every payout and the index-1 source; one accepted block.
    let mut spends = Vec::new();
    let mut inputs: Vec<UtxoId> = chain.payouts.iter().map(|p| p.0).collect();
    inputs.push(chain.indexed_source);
    for id in &inputs {
        let input = &owned[&id.to_bytes()];
        let tx = chain.spend(&snapshot, &anchor, input).unwrap();
        assert_eq!(tx.inputs.clsag()[0].key_image, input.key_image);
        tx.verify_ring_signatures().unwrap();
        spends.push(tx);
    }
    let block = mine(&chain.ledger, &chain.other, spends.clone());

    // Replay of an accepted detached spend is rejected by the ledger.
    let replay = sealed(&chain.ledger, &chain.other, vec![spends[3].clone()]);
    assert!(chain.ledger.add_block(&replay).is_err());

    // The old snapshot cannot sign against the new current anchor, and a fresh
    // witness reports every input spent and refuses to re-sign it.
    let (anchor2, bytes2) = chain.witness();
    let err = chain
        .spend(&snapshot, &anchor2, &owned[&inputs[0].to_bytes()])
        .unwrap_err();
    assert!(err.to_string().contains("stale"), "{err}");
    let snapshot2 = VerifiedSnapshot::verify(&bytes2, &anchor2).unwrap();
    assert_matches_ledger(&chain, &snapshot2);
    let owned2 = by_id(snapshot2.discover(&chain.owner));
    for id in &inputs {
        assert_eq!(owned2[&id.to_bytes()].spent_at, Some(block.height()));
    }
    let err = chain
        .spend(&snapshot2, &anchor2, &owned2[&inputs[4].to_bytes()])
        .unwrap_err();
    assert!(err.to_string().contains("already spent"), "{err}");
    // An input discovered under the old anchor is refused too.
    assert!(chain
        .spend(&snapshot2, &anchor2, &owned[&inputs[0].to_bytes()])
        .is_err());
}

#[test]
fn trust_anchor_is_the_callers_not_the_responses() {
    let chain = Chain::build();
    let (anchor, bytes) = chain.witness();
    let verify = |a: &TrustedCheckpoint| VerifiedSnapshot::verify(&bytes, a).err();
    let mut hash = anchor.hash();
    hash[0] ^= 1;
    let wrong_hash = TrustedCheckpoint::new(anchor.network(), anchor.height(), hash);
    assert_eq!(
        verify(&wrong_hash),
        Some(WitnessError::Anchor("chain does not end at the anchor"))
    );
    for height in [anchor.height() - 1, anchor.height() + 1] {
        let a = TrustedCheckpoint::new(anchor.network(), height, anchor.hash());
        assert!(matches!(verify(&a), Some(WitnessError::Anchor(_))));
    }
    let mainnet = TrustedCheckpoint::new(Network::Mainnet, anchor.height(), anchor.hash());
    assert_eq!(
        verify(&mainnet),
        Some(WitnessError::Anchor("not the network's canonical genesis"))
    );
    let huge = TrustedCheckpoint::new(anchor.network(), MAX_BLOCKS as u64, anchor.hash());
    assert!(matches!(verify(&huge), Some(WitnessError::Bound(_))));

    // A prefix witness verifies against an earlier accepted anchor and sees
    // the state as of that anchor only.
    let earlier = chain.spend_height - 1;
    let prefix = chain.ledger.export_wallet_witness(earlier).unwrap();
    let earlier_anchor = TrustedCheckpoint::new(
        anchor.network(),
        earlier,
        chain.ledger.get_block(earlier).unwrap().hash(),
    );
    let snapshot = VerifiedSnapshot::verify(&prefix, &earlier_anchor).unwrap();
    let owned = by_id(snapshot.discover(&chain.owner));
    assert_eq!(owned[&chain.spent_source.to_bytes()].spent_at, None);
    // ...and that same witness does not verify against the current anchor.
    assert!(VerifiedSnapshot::verify(&prefix, &anchor).is_err());
    assert!(chain
        .ledger
        .export_wallet_witness(anchor.height() + 1)
        .is_err());
}

#[test]
fn codec_controls_fail_closed() {
    let chain = Chain::build();
    let (anchor, bytes) = chain.witness();
    let verify = |b: &[u8]| VerifiedSnapshot::verify(b, &anchor).err();
    assert!(verify(&bytes).is_none());

    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(
        verify(&trailing),
        Some(WitnessError::Encoding("trailing bytes"))
    );
    assert_eq!(
        verify(&bytes[..bytes.len() - 1]),
        Some(WitnessError::Encoding("truncated"))
    );
    let mut version = bytes.clone();
    version[7] = 2;
    assert_eq!(
        verify(&version),
        Some(WitnessError::Encoding("unsupported witness version"))
    );
    let mut rules = bytes.clone();
    rules[6] = 1; // a V1 lottery-rules witness is never reinterpreted
    assert!(matches!(verify(&rules), Some(WitnessError::Encoding(_))));
    // First block length prefix (after magic + count) beyond its bound.
    let mut long = bytes.clone();
    long[12..16].copy_from_slice(&(MAX_BLOCK_BYTES as u32 + 1).to_le_bytes());
    assert_eq!(verify(&long), Some(WitnessError::Bound("block bytes")));
    let mut huge = bytes.clone();
    huge.resize(MAX_WITNESS_BYTES + 1, 0);
    assert_eq!(verify(&huge), Some(WitnessError::Bound("witness bytes")));

    // Record-count and record-length bounds on the first award block.
    let entries = decode_unverified(&bytes, &anchor).unwrap();
    let k = entries.iter().position(|e| e.records.len() == 3).unwrap();
    // Byte offset of entry `upto` (its u32 length prefix).
    let offset = |upto: usize| encode(&entries[..upto]).unwrap().len();
    let body_len = codec().serialize(&entries[k].body).unwrap().len();
    let count_at = offset(k) + 4 + body_len;
    let mut many = bytes.clone();
    many[count_at] = CANDIDATE_MAX_AWARDS as u8 + 1;
    assert_eq!(verify(&many), Some(WitnessError::Bound("record count")));
    let mut record_len = bytes.clone();
    record_len[count_at + 1..count_at + 3]
        .copy_from_slice(&(MAX_RECORD_BYTES as u16 + 1).to_le_bytes());
    assert_eq!(
        verify(&record_len),
        Some(WitnessError::Bound("record bytes"))
    );
    // A noncanonical record (unknown context version byte).
    let record_start = count_at + 3;
    let record_len = u16::from_le_bytes(bytes[count_at + 1..count_at + 3].try_into().unwrap());
    let mut context_version = bytes.clone();
    context_version[record_start + record_len as usize - 37] ^= 0xff;
    assert_eq!(
        verify(&context_version),
        Some(WitnessError::Encoding("record"))
    );
    // One flipped bit in block 1's header (its version field) unlinks block 2.
    let mut body_byte = bytes.clone();
    body_byte[offset(1) + 4] ^= 1;
    assert_eq!(verify(&body_byte), Some(WitnessError::Body("parent hash")));
}

/// Mutate one decoded component, re-encode, and expect a typed refusal.
fn mutated(
    bytes: &[u8],
    anchor: &TrustedCheckpoint,
    k: usize,
    f: impl FnOnce(&mut UnverifiedEntry),
) -> Option<WitnessError> {
    let mut entries = decode_unverified(bytes, anchor).unwrap();
    f(&mut entries[k]);
    VerifiedSnapshot::verify(&encode(&entries).unwrap(), anchor).err()
}

#[test]
fn every_committed_body_component_is_authenticated() {
    let chain = Chain::build();
    let (anchor, bytes) = chain.witness();
    let entries = decode_unverified(&bytes, &anchor).unwrap();
    let tx_block = entries
        .iter()
        .position(|e| !e.body.transactions.is_empty())
        .unwrap();
    let award = entries.iter().position(|e| e.records.len() == 3).unwrap();
    let root = Some(WitnessError::Body("V2 body root"));
    let m = |k, f: Box<dyn FnOnce(&mut UnverifiedEntry)>| mutated(&bytes, &anchor, k, f);

    // Ordinary transaction projection: every output field and key image.
    assert_eq!(
        m(
            tx_block,
            Box::new(|e| e.body.transactions[0].outputs[1].amount += 1)
        ),
        root
    );
    assert_eq!(
        m(
            tx_block,
            Box::new(|e| e.body.transactions[0].outputs[1].target_key[0] ^= 1)
        ),
        root
    );
    assert_eq!(
        m(
            tx_block,
            Box::new(|e| e.body.transactions[0].outputs[1].public_key[0] ^= 1)
        ),
        root
    );
    assert_eq!(
        m(
            tx_block,
            Box::new(|e| e.body.transactions[0].outputs[1]
                .kem_ciphertext
                .as_mut()
                .unwrap()[0] ^= 1)
        ),
        root
    );
    assert_eq!(
        m(
            tx_block,
            Box::new(|e| e.body.transactions[0].outputs[1]
                .cluster_tags
                .entries
                .clear())
        ),
        root
    );
    assert_eq!(
        m(
            tx_block,
            Box::new(|e| e.body.transactions[0].outputs[0].e_memo =
                Some(crate::transaction::EncryptedMemo::from_bytes(&[7; 66]).unwrap()))
        ),
        root
    );
    assert_eq!(
        m(
            tx_block,
            Box::new(|e| e.body.transactions[0].key_images[0][0] ^= 1)
        ),
        root
    );
    assert_eq!(
        m(tx_block, Box::new(|e| e.body.transactions[0].fee += 1)),
        root
    );
    assert_eq!(
        m(
            tx_block,
            Box::new(
                |e| e.body.transactions[0].settlement = Some(SettlementInfo { settled_value: 1 })
            )
        ),
        root
    );
    assert_eq!(
        m(
            tx_block,
            Box::new(|e| e.body.transactions.push(e.body.transactions[0].clone()))
        ),
        root
    );

    // Mint identity, summary, and every payout record component.
    assert_eq!(
        m(award, Box::new(|e| e.body.minting_tx.target_key[0] ^= 1)),
        root
    );
    assert_eq!(m(award, Box::new(|e| e.body.minting_tx.reward += 1)), root);
    assert_eq!(
        m(
            award,
            Box::new(|e| e.body.minting_tx.kem_ciphertext = Some(vec![]))
        ),
        Some(WitnessError::Encoding("noncanonical minting ciphertext"))
    );
    assert_eq!(
        m(
            award,
            Box::new(|e| e.body.lottery_summary.lottery_seed[0] ^= 1)
        ),
        root
    );
    assert_eq!(
        m(
            award,
            Box::new(|e| e.body.lottery_summary.pool_distributed += 1)
        ),
        root
    );
    assert_eq!(m(award, Box::new(|e| e.records[1].amount += 1)), root);
    assert_eq!(m(award, Box::new(|e| e.records[1].winner.index += 1)), root);
    assert_eq!(
        m(award, Box::new(|e| e.records[1].context.base_index += 1)),
        root
    );
    assert_eq!(
        m(award, Box::new(|e| e.records[1].context.tweak[0] ^= 1)),
        root
    );
    assert_eq!(m(award, Box::new(|e| e.records[1].ciphertext = None)), root);
    assert_eq!(
        m(
            award,
            Box::new(|e| e.records[1].public_key = e.records[0].public_key)
        ),
        root
    );
    // Order, duplication and omission.
    assert_eq!(
        m(award, Box::new(|e| e.records.swap(1, 2))),
        Some(WitnessError::Body("payout records"))
    );
    assert_eq!(
        m(award, Box::new(|e| e.records.push(e.records[2].clone()))),
        Some(WitnessError::Body("payout records"))
    );
    assert_eq!(
        m(
            award,
            Box::new(|e| {
                e.records.pop();
            })
        ),
        root
    );
    // Header fields: the chain no longer links to the anchor.
    assert_eq!(
        m(award, Box::new(|e| e.body.header.timestamp += 1)),
        Some(WitnessError::Body("parent hash"))
    );
    assert_eq!(
        m(award, Box::new(|e| e.body.header.height += 1)),
        Some(WitnessError::Body("height sequence"))
    );
    assert_eq!(
        m(entries.len() - 1, Box::new(|e| e.body.header.nonce += 1)),
        Some(WitnessError::Anchor("chain does not end at the anchor"))
    );
    assert_eq!(
        m(0, Box::new(|e| e.body.header.timestamp += 1)),
        Some(WitnessError::Anchor("not the network's canonical genesis"))
    );
}

/// Re-root and re-link from `k` onward and re-anchor at the forged tip, as a
/// responder that ALSO controls the anchor would. Lineage checks still hold.
fn forged(
    bytes: &[u8],
    anchor: &TrustedCheckpoint,
    k: usize,
    f: impl FnOnce(&mut UnverifiedEntry),
) -> Option<WitnessError> {
    let mut entries = decode_unverified(bytes, anchor).unwrap();
    f(&mut entries[k]);
    let mut parent = entries[k - 1].block().hash();
    for e in &mut entries[k..] {
        e.body.header.prev_block_hash = parent;
        let block = e.block();
        e.body.header.tx_root = lottery_v2::root(&block, &e.records).unwrap();
        parent = e.block().hash();
    }
    let forged_anchor = TrustedCheckpoint::new(anchor.network(), anchor.height(), parent);
    VerifiedSnapshot::verify(&encode(&entries).unwrap(), &forged_anchor).err()
}

#[test]
fn lineage_is_derived_not_asserted_even_under_a_forged_anchor() {
    let chain = Chain::build();
    let (anchor, bytes) = chain.witness();
    let entries = decode_unverified(&bytes, &anchor).unwrap();
    let award = entries.iter().position(|e| e.records.len() == 3).unwrap();
    let nested = award + 1;
    let tx_block = entries
        .iter()
        .position(|e| !e.body.transactions.is_empty())
        .unwrap();
    let derivation = Some(WitnessError::Lineage("payout record derivation"));

    // Control: a no-op re-link verifies.
    assert_eq!(forged(&bytes, &anchor, award, |_| {}), None);
    // Altered context, amount, envelope or target.
    assert_eq!(
        forged(&bytes, &anchor, award, |e| e.records[1].context.tweak[0] ^=
            1),
        derivation
    );
    assert_eq!(
        forged(&bytes, &anchor, award, |e| e.records[1]
            .context
            .base_index = 0),
        derivation
    );
    assert_eq!(
        forged(&bytes, &anchor, nested, |e| e.records[0].target =
            e.records[0].public_key),
        derivation
    );
    assert_eq!(
        forged(&bytes, &anchor, award, |e| e.records[1].ciphertext = None),
        derivation
    );
    // A V1-shaped payout (the winner's target copied) is not derivable.
    let source_target = {
        let snapshot = VerifiedSnapshot::verify(&bytes, &anchor).unwrap();
        snapshot.outputs[&chain.indexed_source.to_bytes()]
            .0
            .output
            .target_key
    };
    assert_eq!(
        forged(&bytes, &anchor, award, |e| e.records[1].target =
            source_target),
        derivation
    );
    // Missing dependency: a winner that never existed, or one only created
    // LATER (which is how a cycle would have to look).
    assert_eq!(
        forged(&bytes, &anchor, award, |e| e.records[0].winner.hash =
            [0xee; 32]),
        Some(WitnessError::Lineage("missing payout source"))
    );
    let later = UtxoId::new(entries[nested + 1].block().hash(), 1);
    assert_eq!(
        forged(&bytes, &anchor, nested, |e| e.records[0].winner =
            Outpoint {
                hash: later.tx_hash,
                index: later.output_index,
            }),
        Some(WitnessError::Lineage("missing payout source"))
    );
    // Duplicate transaction / key image across the history.
    let tx = entries[tx_block].body.transactions[0].clone();
    assert_eq!(
        forged(&bytes, &anchor, award, |e| e
            .body
            .transactions
            .push(tx.clone())),
        Some(WitnessError::Lineage("duplicate transaction"))
    );
    let mut images_only = tx;
    images_only.fee += 1;
    assert_eq!(
        forged(&bytes, &anchor, award, |e| e
            .body
            .transactions
            .push(images_only)),
        Some(WitnessError::Lineage("duplicate key image"))
    );
}

#[test]
fn export_reads_accepted_contexts_strictly() {
    let chain = Chain::build();
    let payout = chain.payouts[3].0;
    let mut txn = chain.ledger.env.write_txn().unwrap();
    chain
        .ledger
        .contexts_db
        .delete(&mut txn, &payout.to_bytes())
        .unwrap();
    txn.commit().unwrap();
    let height = chain.ledger.get_chain_state().unwrap().height;
    assert!(matches!(
        chain.ledger.export_wallet_witness(height),
        Err(LedgerError::MissingContext(_))
    ));
}

/// The natural path at the real 720-block lottery maturity: V2 producer ->
/// V2 validator -> persisted contexts -> witness -> detached verification,
/// discovery (equal to the native wallet), signing -> accepted spend of a
/// naturally drawn payout and its source in one block, then replay rejection.
/// `#[ignore]`d for cost like the other real-maturity profiles; run on the
/// checked-arithmetic `ci` profile:
/// `cargo test --profile ci -p botho --lib
/// ledger::store::v2::detached::tests::detached_wallet_spends_natural_payout_at_real_maturity
/// -- --exact --ignored --nocapture`.
#[test]
#[ignore = "real-maturity profile (~750 blocks); run on the ci profile"]
fn detached_wallet_spends_natural_payout_at_real_maturity() {
    const MAX_HEIGHT: u64 = 1_100;
    let dir = TempDir::new().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    let owner = Wallet::from_mnemonic(OWNER_WORDS).unwrap();
    assert_eq!(lottery_v2::canonical_config().draw_config.min_utxo_age, 720);
    let mut win: Option<(UtxoId, UtxoId, u64)> = None;
    loop {
        let state = ledger.get_chain_state().unwrap();
        let height = state.height;
        assert!(height < MAX_HEIGHT, "no natural V2 award by {MAX_HEIGHT}");
        if matches!(win, Some((.., at)) if height >= at + 10) {
            break;
        }
        let mut txs = Vec::new();
        if height > 720 {
            // One fee-paying self-spend per block funds the pool; never the
            // tracked pair.
            let tracked: Vec<UtxoId> = win.iter().flat_map(|(p, s, _)| [*p, *s]).collect();
            if let Some(input) = ledger
                .unspent_wallet_outputs(&owner)
                .unwrap()
                .into_iter()
                .find(|o| {
                    !o.lottery_payout
                        && height - o.utxo.created_at >= 10
                        && !tracked.contains(&o.utxo.id)
                })
            {
                let output = TxOutput::new(
                    input.utxo.output.amount - 10 * FEE,
                    &owner.default_address(),
                );
                txs.push(
                    owner
                        .create_private_transaction(
                            std::slice::from_ref(&input.utxo),
                            vec![output],
                            10 * FEE,
                            height,
                            &ledger,
                        )
                        .unwrap(),
                );
            }
        }
        let block = mine(&ledger, &owner, txs);
        if win.is_none() {
            if let Some((i, o)) = block.lottery_outputs.iter().enumerate().next() {
                let source = UtxoId::new(o.winner_tx_hash, o.winner_output_index);
                win = Some((
                    UtxoId::new(block.hash(), i as u32 + 1),
                    source,
                    block.height(),
                ));
            }
        }
    }
    let (payout, source, _) = win.unwrap();
    let anchor = ledger.accepted_checkpoint().unwrap();
    let bytes = ledger.export_wallet_witness(anchor.height()).unwrap();
    eprintln!(
        "witness: {} blocks, {} bytes",
        anchor.height() + 1,
        bytes.len()
    );
    let snapshot = VerifiedSnapshot::verify(&bytes, &anchor).unwrap();
    let owned = by_id(snapshot.discover(&owner));
    let native = ledger.discover_wallet_outputs(&owner).unwrap();
    assert_eq!(owned.len(), native.len());
    for n in &native {
        let d = &owned[&n.utxo.id.to_bytes()];
        assert_eq!((d.key_image, d.spent_at), (n.key_image, n.spent_at));
    }
    let p = &owned[&payout.to_bytes()];
    let s = &owned[&source.to_bytes()];
    assert_eq!(p.winner, Some(source));
    assert_ne!(p.key_image, s.key_image);
    // A natural payout can be smaller than the fee: add a mature direct helper.
    let helper = owned
        .values()
        .find(|o| {
            o.winner.is_none()
                && o.spent_at.is_none()
                && o.utxo.id != source
                && anchor.height() - o.utxo.created_at >= 10
        })
        .unwrap();
    let mut spends = Vec::new();
    let mut sets = vec![vec![p.clone(), helper.clone()]];
    // The source may have been spent (by a funding spend) before it won.
    if s.spent_at.is_none() {
        sets.push(vec![s.clone()]);
    }
    for inputs in sets {
        let total: u64 = inputs.iter().map(|i| i.utxo.output.amount).sum();
        spends.push(
            snapshot
                .create_transaction(
                    &owner,
                    &anchor,
                    &inputs,
                    vec![TxOutput::new(total - FEE, &owner.default_address())],
                    FEE,
                )
                .unwrap(),
        );
    }
    mine(&ledger, &owner, spends.clone());
    let replay = sealed(&ledger, &owner, vec![spends[0].clone()]);
    assert!(ledger.add_block(&replay).is_err());
    let state = ledger.get_chain_state().unwrap();
    let balance: u128 = ledger
        .unspent_wallet_outputs(&owner)
        .unwrap()
        .iter()
        .map(|o| o.utxo.output.amount as u128)
        .sum();
    assert_eq!(
        balance,
        state.total_mined - state.total_fees_burned - ledger.get_lottery_pool().unwrap()
    );
}
