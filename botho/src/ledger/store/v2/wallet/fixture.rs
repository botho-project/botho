//! TEST ONLY: a production `botho.ledger.2` ledger holding wallet-owned direct
//! sources plus repeated and nested V2 lottery payouts (#1286 P7).
//!
//! The ordinary outputs are seeded with P2's fixture seeder (Direct contexts);
//! each payout block is written through the production writer with real V2
//! records: every payout key is the clsag `lottery_v2::derive` of its source,
//! and every record passes `Record::validate`. The domain's `ordinary_root`
//! is a fixed fixture value, not a consensus draw: this exercises the wallet's
//! derivation-context path, not P3's producer/validator.
use super::*;
use crate::{
    block::{calculate_block_reward, LotteryOutput},
    ledger::{lottery_v2::DerivationReads, ChainState, UtxoSnapshot},
    transaction::{Transaction, MIN_RING_SIZE},
};
use bth_transaction_clsag::lottery_v2::{
    derive, manifest, Award, Domain, Outpoint, Record, Source,
};
use bth_transaction_types::ClusterTagVector;

/// Public deterministic test mnemonics, used only for temporary fixtures.
pub(crate) const OWNER_WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
pub(crate) const OTHER_WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

pub(crate) const SEEDED_HEIGHT: u64 = 100;
pub(crate) const SOURCE_AMOUNT: u64 = 50_000_000_000_000;
pub(crate) const FEE: u64 = 1_000_000_000_000;
/// Tip height after the payout blocks and the maturity padding.
pub(crate) const TIP: u64 = 116;

pub(crate) struct Fixture {
    // Declared before `_dir` so the environment closes before the directory
    // is removed.
    pub(crate) ledger: Ledger,
    pub(crate) owner: Wallet,
    pub(crate) other: Wallet,
    /// Hybrid direct source at a nonzero output index (base index 3).
    pub(crate) hybrid_source: UtxoId,
    /// Classical direct source.
    pub(crate) classical_source: UtxoId,
    /// Two awards to `hybrid_source` in one block (ordinals 0 and 1).
    pub(crate) repeated: [UtxoId; 2],
    /// Award to `classical_source` in the same block (ordinal 2).
    pub(crate) classical_payout: UtxoId,
    /// Award to `repeated[0]` (a payout winning again).
    pub(crate) nested: UtxoId,
    /// Award to `nested` (two levels of nesting).
    pub(crate) nested_twice: UtxoId,
    /// Payout amounts by outpoint.
    pub(crate) payouts: Vec<(UtxoId, u64)>,
    /// Keep alive as long as `ledger` (move it out alongside the ledger).
    pub(crate) dir: tempfile::TempDir,
}

/// A writer-applied block on the current tip, coinbase to `minter`.
fn next_block(ledger: &Ledger, minter: &Wallet, transactions: Vec<Transaction>) -> Block {
    let state = ledger.get_chain_state().unwrap();
    let mut block = Block::new_template_with_txs(
        &Block::genesis(),
        &minter.default_address(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        transactions,
    );
    block.header.height = state.height + 1;
    block.header.prev_block_hash = state.tip_hash;
    block.minting_tx.block_height = block.header.height;
    block.minting_tx.prev_block_hash = state.tip_hash;
    block
}

/// Apply `block` (+ its V2 `records`) through the production writer.
fn write(ledger: &Ledger, block: &Block, records: &[Record]) {
    let state = ledger.get_chain_state().unwrap();
    let bytes = bincode::serialize(block).unwrap();
    let mut txn = ledger.env.write_txn().unwrap();
    ledger
        .write_tables()
        .block_effects(
            &mut txn,
            block,
            &state,
            0,
            None,
            &bytes,
            writer::Representation::Production {
                contexts: ledger.contexts_db,
                records,
            },
            &mut |_| Ok(()),
            &mut || Ok(()),
        )
        .unwrap();
    txn.commit().unwrap();
}

/// A block on the tip awarding `amounts[i]` to `winners[i]`, with honest V2
/// records derived from each winner's persisted output and context.
fn payout_block(ledger: &Ledger, minter: &Wallet, winners: &[(UtxoId, u64)]) -> Block {
    let mut block = next_block(ledger, minter, vec![]);
    let txn = ledger.env.read_txn().unwrap();
    let view = ledger.production_view(&txn);
    let genesis = view.genesis_hash().unwrap();
    let awards: Vec<Award> = winners
        .iter()
        .map(|(id, amount)| Award {
            winner: Outpoint {
                hash: id.tx_hash,
                index: id.output_index,
            },
            amount: *amount,
        })
        .collect();
    let mut records = Vec::new();
    for (ordinal, (id, amount)) in winners.iter().enumerate() {
        let (source_utxo, context) = view.output_context(id).unwrap();
        let source = Source {
            outpoint: awards[ordinal].winner.clone(),
            target: source_utxo.output.target_key,
            context: context.derivation(),
        };
        let domain = Domain {
            genesis,
            parent: block.header.prev_block_hash,
            height: block.height(),
            ordinary_root: [0x5a; 32],
            manifest: manifest(&awards).unwrap(),
            ordinal: ordinal as u32,
            amount: *amount,
        };
        let derived = derive(&source, &domain).unwrap();
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
        block.lottery_outputs.push(LotteryOutput {
            winner_tx_hash: id.tx_hash,
            winner_output_index: id.output_index,
            payout: record.amount,
            target_key: record.target,
            public_key: record.public_key,
            kem_ciphertext: record.ciphertext.clone(),
        });
        records.push(record);
    }
    drop(txn);
    write(ledger, &block, &records);
    block
}

impl Fixture {
    pub(crate) fn build() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let owner = Wallet::from_mnemonic(OWNER_WORDS).unwrap();
        let other = Wallet::from_mnemonic(OTHER_WORDS).unwrap();

        let hybrid = Utxo {
            id: UtxoId::new([0xa1; 32], 3),
            output: TxOutput::new_hybrid_to_address(
                SOURCE_AMOUNT,
                &owner.default_address(),
                3,
                None,
                ClusterTagVector::empty(),
            )
            .unwrap(),
            created_at: 1,
        };
        let classical = Utxo {
            id: UtxoId::new([0xb1; 32], 0),
            output: TxOutput::new(SOURCE_AMOUNT, &owner.default_address()),
            created_at: 1,
        };
        let mut utxos = vec![hybrid.clone(), classical.clone()];
        for index in 0..MIN_RING_SIZE + 4 {
            utxos.push(Utxo {
                id: UtxoId::new([index as u8 + 0x10; 32], 0),
                output: TxOutput::new(SOURCE_AMOUNT, &other.default_address()),
                created_at: 1,
            });
        }
        let state = ChainState {
            height: SEEDED_HEIGHT,
            difficulty: u64::MAX,
            ..ChainState::default()
        };
        let snapshot =
            UtxoSnapshot::new(SEEDED_HEIGHT, [0; 32], state, utxos, vec![], vec![]).unwrap();
        ledger.seed_fixture_snapshot_unvalidated(&snapshot).unwrap();

        // Height 101: a repeated award (twice to the hybrid source) and an
        // award to the classical source.
        let amounts = [20, 21, 22].map(|n| n * FEE);
        let b1 = payout_block(
            &ledger,
            &other,
            &[
                (hybrid.id, amounts[0]),
                (hybrid.id, amounts[1]),
                (classical.id, amounts[2]),
            ],
        );
        let repeated = [UtxoId::new(b1.hash(), 1), UtxoId::new(b1.hash(), 2)];
        let classical_payout = UtxoId::new(b1.hash(), 3);
        // Height 102: the first repeated payout wins again (nested).
        let nested_amount = 23 * FEE;
        let b2 = payout_block(&ledger, &other, &[(repeated[0], nested_amount)]);
        let nested = UtxoId::new(b2.hash(), 1);
        // Height 103: the nested payout wins again.
        let twice_amount = 24 * FEE;
        let b3 = payout_block(&ledger, &other, &[(nested, twice_amount)]);
        let nested_twice = UtxoId::new(b3.hash(), 1);
        // Pad past the ten-confirmation spend maturity.
        while ledger.get_chain_state().unwrap().height < TIP {
            let block = next_block(&ledger, &other, vec![]);
            write(&ledger, &block, &[]);
        }

        Self {
            ledger,
            owner,
            other,
            hybrid_source: hybrid.id,
            classical_source: classical.id,
            repeated,
            classical_payout,
            nested,
            nested_twice,
            payouts: vec![
                (repeated[0], amounts[0]),
                (repeated[1], amounts[1]),
                (classical_payout, amounts[2]),
                (nested, nested_amount),
                (nested_twice, twice_amount),
            ],
            dir,
        }
    }

    pub(crate) fn utxo(&self, id: &UtxoId) -> Utxo {
        self.ledger.get_utxo(id).unwrap().unwrap()
    }

    /// The owner's direct sources and payouts, in a fixed order.
    pub(crate) fn owned_ids(&self) -> Vec<UtxoId> {
        let mut ids = vec![self.hybrid_source, self.classical_source];
        ids.extend(self.payouts.iter().map(|(id, _)| *id));
        ids
    }

    /// Sum of every owned output (sources + payouts).
    pub(crate) fn owned_total(&self) -> u64 {
        2 * SOURCE_AMOUNT + self.payouts.iter().map(|(_, a)| a).sum::<u64>()
    }

    /// The native node wallet's spend of `id` to `other`, against this ledger.
    pub(crate) fn spend(&self, id: &UtxoId) -> Transaction {
        let input = self.utxo(id);
        let height = self.ledger.get_chain_state().unwrap().height;
        let output = TxOutput::new(input.output.amount - FEE, &self.other.default_address());
        self.owner
            .create_private_transaction(&[input], vec![output], FEE, height, &self.ledger)
            .unwrap()
    }

    /// A PoW-valid block on the tip carrying `transactions`, for `add_block`.
    /// Sealed by P3's (#1640) shared producer `Ledger::seal_block` (the
    /// `run.rs` mining path), so the lottery summary, any payouts and the V2
    /// body root are exactly what protocol 7.0.0 validation re-derives.
    pub(crate) fn block_with(&self, transactions: Vec<Transaction>) -> Block {
        let mut block = self
            .ledger
            .seal_block(next_block(&self.ledger, &self.other, transactions))
            .unwrap();
        while !block.header.is_valid_pow() {
            block.header.nonce += 1;
        }
        block.minting_tx.nonce = block.header.nonce;
        block
    }
}
