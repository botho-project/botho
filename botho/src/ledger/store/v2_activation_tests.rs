//! Protocol 7.0.0 LotteryV2 activation on the PRODUCTION ledger (#1286 P3,
//! #1640), at the real 720-block lottery maturity (no eligibility override:
//! the V2 rules read `lottery_v2::canonical_config`, never the environment).
//!
//! One real chain is built per test (sealed by `Ledger::seal_block`, applied
//! by `Ledger::add_block`). The two real-maturity profiles are `#[ignore]`d
//! and run on the optimized profile in CI:
//! `cargo test --profile ci -p botho --lib v2_activation_tests --
//! --include-ignored`. The matrix drives:
//! - independently keyed payouts (target != source target; DH key and
//!   ciphertext inherited; committed Lottery context with the ORIGINAL index);
//! - the forged-payout matrix (target, V1-shaped copied target, public key,
//!   ciphertext, ordinal/order, amount, winner, summary, derivation context,
//!   base index) rejected before any write, and the block hash committing to
//!   every payout field;
//! - repeated awards and source/payout spends in either order through the
//!   production `Wallet` + `WalletRead for Ledger`, with distinct key images
//!   and replay rejection;
//! - full replay and compact-block replay into fresh ledgers reproducing the
//!   identical chain and derivation contexts;
//! - `Domain.genesis` bound to the network genesis hash.
//!
//! Nested awards (a payout that later wins) need a second 720-block maturity
//! window (height >= 1441); that bounded profile is the ignored
//! `nested_award_real_maturity_profile` below (run on the `ci` profile).
use super::*;
use crate::{
    ledger::lottery_v2::{self, DerivationReads, StoredContext},
    mempool::Mempool,
    network::{CompactBlock, ReconstructionResult},
    transaction::{Transaction, TxOutput, Utxo, UtxoId, PICOCREDITS_PER_CREDIT},
    wallet::Wallet,
};
use bth_transaction_clsag::lottery_v2::Record;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};
use tempfile::TempDir;

// Public deterministic test mnemonics, used only for temporary fixtures.
const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
const RECEIVER_WORDS: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
/// Spend fee for source/award spends.
const FEE: u64 = PICOCREDITS_PER_CREDIT;
/// Fee of the per-block self-payment that funds the lottery pool (the
/// emission share alone does not draw at these heights).
const FUNDING_FEE: u64 = 10 * PICOCREDITS_PER_CREDIT;
const MATURITY: u64 = 720;

fn template(ledger: &Ledger, wallet: &Wallet, txs: Vec<Transaction>) -> Block {
    let state = ledger.get_chain_state().unwrap();
    let tip = ledger.get_tip().unwrap();
    let mut block = Block::new_template_with_txs(
        &tip,
        &wallet.default_address(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        txs,
    );
    // Monotonic timestamps even when blocks are produced within one second.
    block.header.timestamp = block.header.timestamp.max(state.tip_timestamp);
    block.minting_tx.timestamp = block.header.timestamp;
    while !block.header.is_valid_pow() {
        block.header.nonce += 1;
    }
    block.minting_tx.nonce = block.header.nonce;
    block
}

/// Seal (V2 producer) and apply (V2 validator) the next block.
fn mine(ledger: &Ledger, wallet: &Wallet, txs: Vec<Transaction>) -> Block {
    let block = ledger
        .seal_block(template(ledger, wallet, txs))
        .expect("seal");
    ledger
        .add_block(&block)
        .expect("V2 validator accepts the V2 producer");
    block
}

fn open(dir: &Path) -> Ledger {
    let ledger = Ledger::open(dir).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    ledger
}

/// The canonical records the V2 producer derives for `block` (unsealed or
/// sealed) over the ledger's current pre-state.
fn records(ledger: &Ledger, block: &Block) -> Vec<Record> {
    let txn = ledger.env.read_txn().unwrap();
    lottery_v2::derive(&ledger.production_view(&txn), block.clone())
        .unwrap()
        .1
}

fn utxo(ledger: &Ledger, id: &UtxoId) -> Utxo {
    ledger.get_utxo(id).unwrap().expect("persisted output")
}

/// Ordered outpoint key (`UtxoId` is not `Ord`).
type Key = [u8; 36];
fn id(key: &Key) -> UtxoId {
    UtxoId::from_bytes(key).unwrap()
}

/// payout outpoint -> its winning source outpoint, for every block so far.
fn awards(blocks: &[Block]) -> BTreeMap<Key, Key> {
    let mut map = BTreeMap::new();
    for block in blocks {
        for (i, o) in block.lottery_outputs.iter().enumerate() {
            map.insert(
                UtxoId::new(block.hash(), i as u32 + 1).to_bytes(),
                o.winner_utxo_id(),
            );
        }
    }
    map
}

fn spend(ledger: &Ledger, owner: &Wallet, to: &Wallet, input: &Utxo, fee: u64) -> Transaction {
    let height = ledger.get_chain_state().unwrap().height;
    owner
        .create_private_transaction(
            std::slice::from_ref(input),
            vec![TxOutput::new_hybrid_to_address(
                input.output.amount - fee,
                &to.default_address(),
                0,
                None,
                Default::default(),
            )
            .unwrap()],
            fee,
            height,
            ledger,
        )
        .expect("production wallet signs the input")
}

/// A fee-paying self-payment that funds the next draw. It spends the coinbase
/// mined 21 blocks earlier, which is mature for ring use but far too young to
/// be a lottery candidate, so funding never consumes a winner or an award.
fn funding(ledger: &Ledger, wallet: &Wallet, blocks: &[Block]) -> Transaction {
    let coinbase = UtxoId::new(blocks[blocks.len() - 21].hash(), 0);
    spend(
        ledger,
        wallet,
        wallet,
        &utxo(ledger, &coinbase),
        FUNDING_FEE,
    )
}

/// A forged variant of `honest` must be rejected before any write.
fn assert_rejected(ledger: &Ledger, forged: &Block, what: &str) {
    let before = ledger.get_chain_state().unwrap();
    let error = ledger.add_block(forged).expect_err(what);
    assert!(
        matches!(error, LedgerError::InvalidBlock(_)),
        "{what}: expected an invalid-block verdict, got {error:?}"
    );
    let after = ledger.get_chain_state().unwrap();
    assert_eq!(
        (after.height, after.tip_hash),
        (before.height, before.tip_hash),
        "{what}: rejected block wrote state"
    );
}

/// Domain.genesis is the network genesis hash (#1640).
#[test]
fn payout_domain_genesis_is_the_network_genesis_hash() {
    let mut seen = BTreeSet::new();
    for network in [Network::Testnet, Network::Mainnet] {
        let dir = TempDir::new().unwrap();
        let ledger = Ledger::open_for_network(dir.path(), network).unwrap();
        let txn = ledger.env.read_txn().unwrap();
        let genesis = ledger.production_view(&txn).genesis_hash().unwrap();
        assert_eq!(genesis, Block::genesis_for_network(network).hash());
        assert!(seen.insert(genesis), "networks must not share a domain");
    }
}

/// ~75 s on the optimized `ci` profile, ~215 s in debug: an explicit
/// optimized-profile run in CI (`workspace-build.yml`), like the nested
/// profile below.
#[test]
#[ignore = "real-maturity profile (~770 blocks); run on the ci profile"]
fn v2_activation_real_maturity_matrix() {
    let began = Instant::now();
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let receiver = Wallet::from_mnemonic(RECEIVER_WORDS).unwrap();
    assert_eq!(
        lottery_v2::canonical_config().draw_config.min_utxo_age,
        MATURITY
    );

    // ---- Maturity: no payout can exist before height 721. ------------------
    let mut blocks = Vec::new();
    for _ in 1..=MATURITY {
        let block = mine(&ledger, &wallet, vec![]);
        assert!(block.lottery_outputs.is_empty(), "payout before maturity");
        blocks.push(block);
    }
    eprintln!(
        "V2_ACTIVATION phase=maturity height={MATURITY} elapsed_ms={}",
        began.elapsed().as_millis()
    );

    // ---- Forged-payout matrix on the first paying block. -------------------
    // The reward cap scales with the candidate set, so the first mature
    // blocks draw a single small award; use the first block with >= 2.
    let honest = loop {
        let fund = funding(&ledger, &wallet, &blocks);
        let sealed = ledger
            .seal_block(template(&ledger, &wallet, vec![fund]))
            .unwrap();
        if blocks.len() as u64 == MATURITY {
            assert!(
                !sealed.lottery_outputs.is_empty(),
                "block {} must pay its first mature candidate: {:?}",
                MATURITY + 1,
                sealed.lottery_summary
            );
        }
        if sealed.lottery_outputs.len() >= 2 {
            break sealed;
        }
        ledger.add_block(&sealed).unwrap();
        blocks.push(sealed);
        assert!(blocks.len() < 800, "no multi-winner draw within bound");
    };
    let honest_records = records(&ledger, &honest);
    assert_eq!(
        lottery_v2::root(&honest, &honest_records).unwrap(),
        honest.header.tx_root,
        "header commits the V2 body root"
    );
    assert_ne!(
        honest.header.tx_root,
        Block::compute_tx_root(&honest.transactions)
    );
    let source_of = |i: usize| {
        let o = &honest.lottery_outputs[i];
        utxo(
            &ledger,
            &UtxoId::new(o.winner_tx_hash, o.winner_output_index),
        )
    };
    let with = |edit: &dyn Fn(&mut Block)| {
        let mut b = honest.clone();
        edit(&mut b);
        b
    };
    // Wire-field forgeries under the honest root.
    let source0 = source_of(0);
    let mut cases: Vec<(&str, Block)> = vec![
        (
            "forged target",
            with(&|b| b.lottery_outputs[0].target_key[0] ^= 1),
        ),
        (
            "V1-shaped payout (winner target copied)",
            with(&|b| b.lottery_outputs[0].target_key = source0.output.target_key),
        ),
        (
            "forged public key",
            with(&|b| b.lottery_outputs[0].public_key[0] ^= 1),
        ),
        ("forged amount", with(&|b| b.lottery_outputs[0].payout += 1)),
        (
            "forged winner",
            with(&|b| b.lottery_outputs[0].winner_output_index ^= 1),
        ),
        (
            "forged ordinal (order)",
            with(&|b| b.lottery_outputs.swap(0, 1)),
        ),
        (
            "dropped payout",
            with(&|b| {
                b.lottery_outputs.pop();
            }),
        ),
        (
            "forged summary",
            with(&|b| b.lottery_summary.pool_distributed += 1),
        ),
        (
            "forged ciphertext",
            with(&|b| {
                let o = &mut b.lottery_outputs[0];
                o.kem_ciphertext = match o.kem_ciphertext.take() {
                    Some(mut ct) => {
                        ct[0] ^= 1;
                        Some(ct)
                    }
                    None => Some(vec![0; 1088]),
                };
            }),
        ),
        (
            "stripped ciphertext",
            with(&|b| {
                let o = &mut b.lottery_outputs[0];
                o.kem_ciphertext = if o.kem_ciphertext.is_some() {
                    None
                } else {
                    Some(vec![1; 1088])
                };
            }),
        ),
    ];
    // Forged derivation context / base index: not on the wire, so a forger
    // must commit it through a recomputed body root.
    for (what, edit) in [
        (
            "forged context tweak",
            (|r: &mut Record| r.context.tweak[0] ^= 1) as fn(&mut Record),
        ),
        ("forged base index", |r: &mut Record| {
            r.context.base_index ^= 1
        }),
    ] {
        let mut forged = honest_records.clone();
        edit(&mut forged[0]);
        let mut b = honest.clone();
        b.header.tx_root = lottery_v2::root(&b, &forged).unwrap();
        assert_ne!(
            b.hash(),
            honest.hash(),
            "{what}: hash must commit the context"
        );
        cases.push((what, b));
    }
    // #1676: the committed seed is the PoW-bound V2 seed of the parent,
    // never the V1 header-hash seed; a block committing the V1 seed (under a
    // recomputed root, so only the seed differs) is rejected.
    let parent = blocks.last().unwrap();
    let seed = crate::consensus::lottery_seed_v2(
        &parent.header.pow_hash(),
        &parent.header.prev_block_hash,
        honest.height(),
    );
    assert_eq!(
        honest.lottery_summary.lottery_seed, seed,
        "V2 PoW-bound seed"
    );
    let v1_seed: [u8; 32] = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"LOTTERY_SEED_V1");
        h.update(honest.header.prev_block_hash);
        h.update(honest.height().to_le_bytes());
        h.finalize().into()
    };
    assert_ne!(v1_seed, seed);
    for (what, forged_seed) in [
        ("V1 header-hash seed", v1_seed),
        ("forged seed", {
            let mut x = seed;
            x[0] ^= 1;
            x
        }),
    ] {
        let mut b = honest.clone();
        b.lottery_summary.lottery_seed = forged_seed;
        b.header.tx_root = lottery_v2::root(&b, &honest_records).unwrap();
        assert_ne!(b.hash(), honest.hash(), "{what}: seed committed");
        cases.push((what, b));
    }
    // Wire forgeries carried under a RECOMPUTED root as well.
    // (Only payout-changing cases: the summary and context cases keep the
    // honest payouts, so re-deriving their root would rebuild the honest
    // block.)
    for (what, block) in cases.clone() {
        if block.lottery_outputs.len() == honest.lottery_outputs.len()
            && block.lottery_outputs != honest.lottery_outputs
        {
            let mut forged = honest_records.clone();
            for (r, o) in forged.iter_mut().zip(&block.lottery_outputs) {
                r.winner.hash = o.winner_tx_hash;
                r.winner.index = o.winner_output_index;
                r.amount = o.payout;
                r.target = o.target_key;
                r.public_key = o.public_key;
                r.ciphertext = o.kem_ciphertext.clone();
            }
            if let Ok(root) = lottery_v2::root(&block, &forged) {
                let mut b = block.clone();
                b.header.tx_root = root;
                // Mutating a payout changes the block hash (#1640).
                assert_ne!(b.hash(), honest.hash(), "{what}: payout not committed");
                cases.push((what, b));
            }
        }
    }
    for (what, forged) in &cases {
        assert_rejected(&ledger, forged, what);
    }
    ledger.add_block(&honest).expect("honest V2 block accepted");
    blocks.push(honest.clone());

    // ---- Independently keyed payouts with committed contexts. --------------
    for (i, o) in honest.lottery_outputs.iter().enumerate() {
        let payout_id = UtxoId::new(honest.hash(), i as u32 + 1);
        let payout = utxo(&ledger, &payout_id);
        let source = source_of(i);
        assert_ne!(
            payout.output.target_key, source.output.target_key,
            "target differs"
        );
        assert_eq!(payout.output.public_key, source.output.public_key);
        assert_eq!(payout.output.kem_ciphertext, source.output.kem_ciphertext);
        assert_eq!(payout.output.amount, o.payout);
        match ledger.get_derivation_context(&payout_id).unwrap() {
            StoredContext::Lottery {
                height,
                ordinal,
                context,
            } => {
                assert_eq!((height, ordinal), (honest.height(), i as u32));
                assert_eq!(context, honest_records[i].context);
                assert_eq!(context.base_index, source.id.output_index);
                assert_ne!(context.tweak, [0; 32]);
            }
            other => panic!("payout context must be Lottery, got {other:?}"),
        }
    }

    // ---- Repeated awards: mine until one source has won twice. -------------
    // Only awards worth more than the spend fee are spent below.
    let usable = |ledger: &Ledger, key: &Key| utxo(ledger, &id(key)).output.amount > 2 * FEE;
    let repeated = loop {
        let fund = funding(&ledger, &wallet, &blocks);
        let block = mine(&ledger, &wallet, vec![fund]);
        blocks.push(block);
        let mut by_source: BTreeMap<Key, Vec<Key>> = BTreeMap::new();
        for (payout, source) in awards(&blocks) {
            if usable(&ledger, &payout) {
                by_source.entry(source).or_default().push(payout);
            }
        }
        if let Some((source, payouts)) = by_source.into_iter().find(|(_, p)| p.len() >= 2) {
            break (source, payouts);
        }
        assert!(blocks.len() < 1000, "no repeated award within bound");
    };
    eprintln!(
        "V2_ACTIVATION phase=repeated height={} elapsed_ms={}",
        blocks.len(),
        began.elapsed().as_millis()
    );
    let all = awards(&blocks);
    let targets: BTreeSet<_> = all
        .keys()
        .map(|k| utxo(&ledger, &id(k)).output.target_key)
        .collect();
    assert_eq!(
        targets.len(),
        all.len(),
        "every award has its own one-time target"
    );

    // Inputs need ten confirmations for the production wallet.
    for _ in 0..10 {
        blocks.push(mine(&ledger, &wallet, vec![]));
    }

    // ---- Spends in either order, distinct key images, replay rejected. -----
    let (repeat_source, repeat_payouts) = repeated;
    let other_family = all
        .iter()
        .find(|(p, s)| **s != repeat_source && !repeat_payouts.contains(p) && usable(&ledger, p))
        .map(|(p, s)| (*s, *p))
        .expect("a second family");
    // Order 1: source, then each repeated payout. Order 2: payout, then source.
    let order = [
        repeat_source,
        repeat_payouts[0],
        repeat_payouts[1],
        other_family.1,
        other_family.0,
    ];
    let mut images = BTreeSet::new();
    for key in order {
        let input = utxo(&ledger, &id(&key));
        let tx = spend(&ledger, &wallet, &receiver, &input, FEE);
        let image = tx.inputs.clsag()[0].key_image;
        assert!(
            images.insert(image),
            "key image reused across source/awards"
        );
        let block = mine(&ledger, &wallet, vec![tx.clone()]);
        assert!(ledger.is_key_image_spent(&image).unwrap().is_some());
        // Replay of the same spend is rejected.
        let replay = ledger
            .seal_block(template(&ledger, &wallet, vec![tx]))
            .unwrap();
        assert_rejected(&ledger, &replay, "replayed spend");
        blocks.push(block);
    }
    eprintln!(
        "V2_ACTIVATION phase=spent height={} elapsed_ms={}",
        blocks.len(),
        began.elapsed().as_millis()
    );

    // ---- Full replay and compact-block replay are deterministic. -----------
    for compact in [false, true] {
        let dir = TempDir::new().unwrap();
        let replica = open(dir.path());
        let mempool = Mempool::new();
        for block in &blocks {
            let block = if compact {
                let prefill: Vec<usize> = (0..block.transactions.len()).collect();
                match CompactBlock::from_block_with_prefilled(block, &prefill).reconstruct(&mempool)
                {
                    ReconstructionResult::Complete(b) => b,
                    other => panic!("compact replay incomplete: {other:?}"),
                }
            } else {
                block.clone()
            };
            replica
                .add_block(&block)
                .expect("replica accepts the chain");
        }
        assert_eq!(
            replica.get_chain_state().unwrap().tip_hash,
            ledger.get_chain_state().unwrap().tip_hash
        );
        for key in all.keys() {
            assert_eq!(
                replica.get_derivation_context(&id(key)).unwrap(),
                ledger.get_derivation_context(&id(key)).unwrap(),
                "replica context (compact={compact})"
            );
        }
    }
    eprintln!(
        "V2_ACTIVATION phase=done height={} awards={} elapsed_ms={}",
        blocks.len(),
        all.len(),
        began.elapsed().as_millis()
    );
}

/// Nested awards at the real maturity: a lottery payout that is itself at
/// least 720 blocks old wins again, so the new payout's context composes the
/// parent's (ORIGINAL base index, cumulative tweak), and the production
/// wallet spends the nested payout through it. Needs a second maturity
/// window (height >= 1441), so it is an explicit optimized-profile run:
/// `cargo test --profile ci -p botho --lib nested_award_real_maturity_profile
/// -- --ignored --nocapture`.
#[test]
#[ignore = "real-maturity nested-award profile (>= 1441 blocks); run on the ci profile"]
fn nested_award_real_maturity_profile() {
    let began = Instant::now();
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let receiver = Wallet::from_mnemonic(RECEIVER_WORDS).unwrap();
    let mut blocks = Vec::new();
    for _ in 1..=MATURITY {
        blocks.push(mine(&ledger, &wallet, vec![]));
    }
    // Fund every block until a payout outpoint itself wins an award.
    let (nested, parent) = loop {
        let fund = funding(&ledger, &wallet, &blocks);
        let block = mine(&ledger, &wallet, vec![fund]);
        blocks.push(block);
        let all = awards(&blocks);
        if let Some((payout, source)) = all.iter().find(|(_, source)| all.contains_key(*source)) {
            break (*payout, *source);
        }
        if blocks.len() % 100 == 0 {
            eprintln!(
                "V2_NESTED height={} awards={} elapsed_ms={}",
                blocks.len(),
                all.len(),
                began.elapsed().as_millis()
            );
        }
        assert!(blocks.len() < 3_000, "no nested award within bound");
    };
    let nested_height = blocks.len();
    assert!(
        nested_height as u64 > 2 * MATURITY,
        "a payout matures only after 720 blocks"
    );
    let context = |key: &Key| match ledger.get_derivation_context(&id(key)).unwrap() {
        StoredContext::Lottery { context, .. } => context,
        other => panic!("payout context must be Lottery, got {other:?}"),
    };
    let (nested_ctx, parent_ctx) = (context(&nested), context(&parent));
    // The parent payout's own source carries the ORIGINAL derivation index.
    let original = awards(&blocks)[&parent];
    assert_eq!(
        nested_ctx.base_index, parent_ctx.base_index,
        "base index is the original"
    );
    assert_eq!(parent_ctx.base_index, id(&original).output_index);
    assert_ne!(nested_ctx.tweak, parent_ctx.tweak, "tweak is cumulative");
    let (nested_out, parent_out) = (utxo(&ledger, &id(&nested)), utxo(&ledger, &id(&parent)));
    assert_ne!(nested_out.output.target_key, parent_out.output.target_key);
    assert_eq!(nested_out.output.public_key, parent_out.output.public_key);
    assert_eq!(
        nested_out.output.kem_ciphertext,
        parent_out.output.kem_ciphertext
    );

    for _ in 0..10 {
        blocks.push(mine(&ledger, &wallet, vec![]));
    }
    // Spend nested then parent: distinct key images, both accepted. Awards
    // can be small, so the fee is a quarter of the award (capped at FEE),
    // still far above the consensus floor.
    let mut images = BTreeSet::new();
    for key in [nested, parent] {
        let input = utxo(&ledger, &id(&key));
        let fee = FEE.min(input.output.amount / 4);
        let tx = spend(&ledger, &wallet, &receiver, &input, fee);
        let image = tx.inputs.clsag()[0].key_image;
        assert!(images.insert(image), "nested/parent key images must differ");
        blocks.push(mine(&ledger, &wallet, vec![tx]));
        assert!(ledger.is_key_image_spent(&image).unwrap().is_some());
    }
    assert_eq!(images.len(), 2, "nested and parent payouts both spent");
    strict_scan_matches_full_decode_reference(&ledger, &wallet, &blocks);
    eprintln!(
        "V2_NESTED done nested_height={nested_height} height={} awards={} spent={} elapsed_ms={}",
        blocks.len(),
        awards(&blocks).len(),
        images.len(),
        began.elapsed().as_millis()
    );
}

/// #1670 §2 at real payout scale: the production strict scan (memoized,
/// header-only creating-block hash, no output re-read) returns exactly the
/// candidate set of the pre-#1670 reference (full `read_context` per row,
/// whole creating block decoded per payout row), reads each creating block at
/// most once, and reports both scan times.
fn strict_scan_matches_full_decode_reference(ledger: &Ledger, wallet: &Wallet, blocks: &[Block]) {
    use super::{
        v2::{read_context_with, SCAN_BLOCK_LOOKUPS},
        validation::{self, ReadPolicy},
    };
    let next = template(ledger, wallet, vec![]);
    let config = lottery_v2::canonical_config().draw_config;
    let payouts: usize = blocks.iter().map(|b| b.lottery_outputs.len()).sum();
    let paying_blocks = blocks
        .iter()
        .filter(|b| !b.lottery_outputs.is_empty())
        .count() as u64;
    let txn = ledger.env.read_txn().unwrap();
    let view = ledger.production_view(&txn);
    let lookups = SCAN_BLOCK_LOOKUPS.with(|n| n.get());
    let began = Instant::now();
    let seed = lottery_v2::seed(&view, &next).unwrap();
    let production = view.lottery_candidates(&next, &seed, &config).unwrap();
    let production_us = began.elapsed().as_micros();
    let lookups = SCAN_BLOCK_LOOKUPS.with(|n| n.get()) - lookups;
    assert!(
        lookups <= paying_blocks,
        "{lookups} creating-block reads for {paying_blocks} paying blocks"
    );
    let began = Instant::now();
    let reference = validation::lottery_candidates(
        &ledger.write_tables(),
        &txn,
        next.height(),
        &seed,
        &config,
        ReadPolicy::Strict,
        &mut |u| {
            read_context_with(ledger, &txn, &u.id, &mut |height| {
                ledger
                    .blocks_db
                    .get(&txn, &height)
                    .unwrap()
                    .map(|b| crate::ledger::experimental::decode::<Block>(b).map(|b| b.hash()))
                    .transpose()
            })
            .map(|_| ())
        },
    )
    .unwrap();
    let reference_us = began.elapsed().as_micros();
    let ids = |c: &[bth_cluster_tax::LotteryCandidate]| -> Vec<_> {
        c.iter()
            .map(|c| (c.id, c.value, c.cluster_factor, c.creation_block))
            .collect()
    };
    assert_eq!(ids(&production), ids(&reference), "identical eligible set");
    eprintln!(
        "V2_SCAN payouts={payouts} paying_blocks={paying_blocks} block_reads={lookups} \
         candidates={} production_us={production_us} reference_us={reference_us}",
        production.len()
    );
}
