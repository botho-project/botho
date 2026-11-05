//! #1920: an admitted transfer that fails block validation at the block's own
//! height must never reach an applied block.
//!
//! Real, signed spends on a mined chain (the #1858 fixture helpers in
//! `mempool_tag_gate_tests.rs`), checked through the three enforcement points:
//! the block builder's seal-time skip (`BlockBuilder::finalize_for_apply`), the
//! SCP value check for a slot (`ledger_transfer_slot_check` inside
//! `ConsensusService`'s validity function, on the slot's recorded parent,
//! including under the #421 slot/height drift), and the ledger primitive both
//! use (`Ledger::check_transfer_for_block`). Kept out of `store.rs` so the
//! tests do not churn that file's SHA-256 pin in
//! `scripts/research/fixtures/ct-contract-inventory.json`.

use super::mempool_tag_gate_tests::{chain, coinbase, stock_spend, template};
use crate::{
    block::Block,
    consensus::{
        ledger_transfer_slot_check, BlockBuilder, ConsensusConfig, ConsensusService, ConsensusValue,
    },
    ledger::{Ledger, LedgerError, TransferBlockCheck},
    transaction::{Transaction, Utxo},
    wallet::Wallet,
};
use bth_consensus_scp::{QuorumSet, QuorumSetMember};
use bth_consensus_scp_types::test_utils::test_node_id;
use std::sync::{Arc, RwLock};

/// A signed spend of `input` whose fee is below the consensus fee floor of the
/// next block, and one of the same input whose fee covers it.
fn under_and_adequate(
    ledger: &Ledger,
    wallet: &Wallet,
    input: &Utxo,
) -> (Transaction, Transaction) {
    let next = ledger.get_chain_state().unwrap().height + 1;
    let probe = stock_spend(ledger, wallet, input, 1);
    let floor = ledger.consensus_fee_floor(&probe, next).unwrap();
    assert!(floor > 1, "fixture needs a non-trivial floor, got {floor}");

    let under = stock_spend(ledger, wallet, input, floor / 2);
    let under_floor = ledger.consensus_fee_floor(&under, next).unwrap();
    assert!(
        under.fee < under_floor,
        "fixture: fee {} must be below the floor {under_floor}",
        under.fee
    );
    let adequate = stock_spend(ledger, wallet, input, floor * 2);
    let adequate_floor = ledger.consensus_fee_floor(&adequate, next).unwrap();
    assert!(
        adequate.fee >= adequate_floor,
        "fixture: fee {} must cover the floor {adequate_floor}",
        adequate.fee
    );
    (under, adequate)
}

fn is_fee_floor_rejection(error: &LedgerError) -> bool {
    matches!(error, LedgerError::InvalidBlock(msg) if msg.contains("fee-floor validation failed"))
}

/// Builder plus apply round trip (#1920). Sealing the externalized block as
/// before #1920 (`apply_lottery`, no skip) yields a block `add_block` rejects
/// for its under-floor transfer: the run loop's "Terminal consensus failure:
/// externalized block could not be applied" path on every node. With the
/// seal-time skip the under-floor transfer is dropped, the adequate one is
/// kept, and the block applies, so that path is no longer reachable through
/// the fee floor.
#[test]
fn builder_skips_under_floor_transfer_and_block_applies() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let (under, _) = under_and_adequate(&ledger, &wallet, &coinbase(&ledger, &blocks[2]));
    let (_, adequate) = under_and_adequate(&ledger, &wallet, &coinbase(&ledger, &blocks[3]));
    let externalized: Block = template(&ledger, &wallet, vec![under.clone(), adequate.clone()]);
    let tip = ledger.get_chain_state().unwrap().height;

    // Before #1920: the sealed block is invalid on every node.
    let unfiltered = BlockBuilder::apply_lottery(externalized.clone(), &ledger).unwrap();
    let error = ledger.add_block(&unfiltered).unwrap_err();
    assert!(is_fee_floor_rejection(&error), "{error:?}");
    assert_eq!(ledger.get_chain_state().unwrap().height, tip);

    // #1920: the under-floor transfer is skipped and the block applies.
    let sealed = BlockBuilder::finalize_for_apply(externalized, &ledger).unwrap();
    let hashes: Vec<_> = sealed.transactions.iter().map(Transaction::hash).collect();
    assert_eq!(hashes, vec![adequate.hash()]);
    ledger.add_block(&sealed).unwrap();
    assert_eq!(ledger.get_chain_state().unwrap().height, tip + 1);
}

/// The skip is a pure function of the block and its parent: two seals of the
/// same externalized block on the same parent are byte-identical (no fork),
/// and a block whose parent is not the tip is returned unfiltered (sealing it
/// then fails on the parent check, as before).
#[test]
fn builder_skip_is_deterministic_and_parent_gated() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let (under, adequate) = under_and_adequate(&ledger, &wallet, &coinbase(&ledger, &blocks[2]));
    let externalized = template(&ledger, &wallet, vec![under, adequate.clone()]);

    let (kept, dropped) = ledger
        .retain_block_valid_transfers(externalized.clone())
        .unwrap();
    let (again, dropped_again) = ledger
        .retain_block_valid_transfers(externalized.clone())
        .unwrap();
    assert_eq!(dropped, dropped_again);
    assert_eq!(
        bincode::serialize(&kept).unwrap(),
        bincode::serialize(&again).unwrap()
    );
    assert_eq!(dropped.len(), 1);
    assert!(
        dropped[0].reason.contains("consensus fee floor"),
        "{dropped:?}"
    );
    let kept: Vec<_> = kept.transactions.iter().map(Transaction::hash).collect();
    assert_eq!(kept, vec![adequate.hash()]);

    let mut orphan = externalized.clone();
    orphan.header.prev_block_hash = [7u8; 32];
    let (unchanged, none) = ledger.retain_block_valid_transfers(orphan.clone()).unwrap();
    assert!(none.is_empty());
    assert_eq!(unchanged.transactions.len(), 2);
}

/// Two transfers spending the same input: the builder keeps the first valid
/// one and skips the repeat, so a block it seals never double-spends.
#[test]
fn builder_skips_repeated_key_image() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let input = coinbase(&ledger, &blocks[2]);
    let (_, adequate) = under_and_adequate(&ledger, &wallet, &input);
    let bumped = stock_spend(&ledger, &wallet, &input, adequate.fee * 2);
    assert_ne!(bumped.hash(), adequate.hash());
    let externalized = template(&ledger, &wallet, vec![adequate.clone(), bumped]);
    let (kept, dropped) = ledger.retain_block_valid_transfers(externalized).unwrap();
    let kept: Vec<_> = kept.transactions.iter().map(Transaction::hash).collect();
    assert_eq!(kept, vec![adequate.hash()]);
    assert_eq!(dropped.len(), 1);
    assert!(dropped[0].reason.contains("key image"), "{dropped:?}");
}

/// The ledger primitive: a verdict only for the block on the given parent,
/// and only when the tip is that parent.
#[test]
fn check_transfer_for_block_verdicts() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let (under, adequate) = under_and_adequate(&ledger, &wallet, &coinbase(&ledger, &blocks[2]));
    let state = ledger.get_chain_state().unwrap();
    let (tip, hash) = (state.height, state.tip_hash);

    assert_eq!(
        ledger
            .check_transfer_for_block(&adequate, tip, hash)
            .unwrap(),
        TransferBlockCheck::Valid
    );
    match ledger.check_transfer_for_block(&under, tip, hash).unwrap() {
        TransferBlockCheck::Invalid(reason) => {
            assert!(reason.contains("consensus fee floor"), "{reason}")
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    let not_at_parent = TransferBlockCheck::NotAtParent {
        tip_height: tip,
        tip_hash: hash,
    };
    let previous = blocks[blocks.len() - 2].hash();
    for (height, parent_hash) in [(tip - 1, previous), (tip + 1, hash), (tip, [7u8; 32])] {
        assert_eq!(
            ledger
                .check_transfer_for_block(&under, height, parent_hash)
                .unwrap(),
            not_at_parent,
            "parent ({height}, {:?})",
            &parent_hash[..4]
        );
    }
}

/// #1920 review: a transfer whose own inputs repeat a key image is rejected
/// by the per-transaction block check, as `add_block` rejects it (its write
/// transaction sees the first input's key image when recording the second).
/// Not reachable through admission today (`is_valid_structure` rejects it),
/// so the fixture repeats a signed input after the fact.
#[test]
fn transfer_repeating_its_own_key_image_is_invalid() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let (_, adequate) = under_and_adequate(&ledger, &wallet, &coinbase(&ledger, &blocks[2]));
    let mut doubled = adequate.clone();
    let mut inputs = doubled.inputs.clsag().to_vec();
    inputs.push(inputs[0].clone());
    doubled.inputs = crate::transaction::TxInputs::new(inputs);
    let state = ledger.get_chain_state().unwrap();

    match ledger
        .check_transfer_for_block(&doubled, state.height, state.tip_hash)
        .unwrap()
    {
        TransferBlockCheck::Invalid(reason) => assert!(
            reason.contains("Input 1 repeats a key image of an earlier input"),
            "{reason}"
        ),
        other => panic!("expected Invalid, got {other:?}"),
    }
    let externalized = template(&ledger, &wallet, vec![doubled.clone(), adequate.clone()]);
    let (kept, dropped) = ledger
        .retain_block_valid_transfers(externalized.clone())
        .unwrap();
    let kept: Vec<_> = kept.transactions.iter().map(Transaction::hash).collect();
    assert_eq!(kept, vec![adequate.hash()]);
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].tx_hash, doubled.hash());
    let unfiltered =
        BlockBuilder::apply_lottery(template(&ledger, &wallet, vec![doubled]), &ledger).unwrap();
    assert!(ledger.add_block(&unfiltered).is_err());
}

/// SCP value validation (#1920): with the production ledger check installed,
/// the validity function rejects a transfer that is under the floor of a
/// block at its slot and accepts the adequately paid spend of the same input.
/// A slot whose parent is not the ledger tip gets no verdict (an error that
/// SCP does not cache), and the verdict for the right slot is unaffected.
#[test]
fn scp_validity_checks_transfers_at_the_slot_height() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let (under, adequate) = under_and_adequate(&ledger, &wallet, &coinbase(&ledger, &blocks[2]));
    let state = ledger.get_chain_state().unwrap();
    let slot = state.height + 1;
    let ledger = Arc::new(RwLock::new(ledger));

    let mut svc = ConsensusService::new(
        test_node_id(1),
        QuorumSet::new(1, vec![QuorumSetMember::Node(test_node_id(1))]),
        ConsensusConfig::fixed_timing(1),
        state,
    );
    let value = |tx: &Transaction| ConsensusValue::from_transaction(tx.hash(), tx.fee);
    for tx in [&under, &adequate] {
        svc.register_transfer_tx(tx.hash(), bincode::serialize(tx).unwrap());
    }

    // Without the ledger check both are intrinsically valid (pre-#1920).
    assert!(svc.validate_value_for_slot(slot, &value(&under)).is_ok());
    assert!(svc.validate_value_for_slot(slot, &value(&adequate)).is_ok());

    svc.set_transfer_slot_check(ledger_transfer_slot_check(ledger.clone()));
    let rejected = svc
        .validate_value_for_slot(slot, &value(&under))
        .unwrap_err();
    assert!(rejected.contains("consensus fee floor"), "{rejected}");
    svc.validate_value_for_slot(slot, &value(&adequate))
        .unwrap();

    let not_yet = svc
        .validate_value_for_slot(slot + 1, &value(&adequate))
        .unwrap_err();
    assert!(not_yet.contains("not yet checkable"), "{not_yet}");
    svc.validate_value_for_slot(slot, &value(&adequate))
        .unwrap();
}

/// Decide one solo slot on `svc` with `mint_block`'s minting tx (and any
/// pending transfers), then build, seal and apply the block exactly as the
/// run loop's apply job does (`build_from_externalized`, then
/// `finalize_for_apply`, then `add_block`), and advance the slot. A failed
/// apply is the #421 benign skip. Returns the externalized values and the
/// applied block, if any.
fn decide_and_apply(
    svc: &mut ConsensusService,
    ledger: &Arc<RwLock<Ledger>>,
    mint_block: &Block,
) -> (Vec<ConsensusValue>, Option<Block>) {
    let mint = &mint_block.minting_tx;
    svc.submit_minting_tx(
        mint.hash(),
        mint.pow_priority(),
        bincode::serialize(mint).unwrap(),
    );
    svc.tick();
    let values = loop {
        match svc.next_event() {
            Some(crate::consensus::ConsensusEvent::SlotExternalized { values, .. }) => {
                break values
            }
            Some(_) => continue,
            None => panic!("solo slot did not externalize"),
        }
    };
    let built = BlockBuilder::build_from_externalized(
        &values,
        |hash| {
            svc.get_tx_data(hash)
                .and_then(|bytes| bincode::deserialize(&bytes).ok())
        },
        |hash| {
            svc.get_tx_data(hash)
                .and_then(|bytes| bincode::deserialize(&bytes).ok())
        },
    )
    .unwrap();
    let applied = {
        let ledger = ledger.read().unwrap();
        BlockBuilder::finalize_for_apply(built.block, &ledger)
            .ok()
            .filter(|sealed| ledger.add_block(sealed).is_ok())
    };
    svc.advance_slot();
    (values, applied)
}

fn solo_service_on(ledger: &Arc<RwLock<Ledger>>) -> ConsensusService {
    let state = ledger.read().unwrap().get_chain_state().unwrap();
    let mut svc = ConsensusService::new(
        test_node_id(1),
        QuorumSet::new(1, vec![QuorumSetMember::Node(test_node_id(1))]),
        ConsensusConfig::fixed_timing(0),
        state,
    );
    svc.set_transfer_slot_check(ledger_transfer_slot_check(ledger.clone()));
    svc
}

/// #1920 review, on a real ledger: a lost-race decision (a second mint for a
/// height already filled) moves the SCP slot one past the block height
/// (#421). The paid transfer proposed next is decided and lands in the very
/// next applied block, and the under-floor spend is dropped; the slot's
/// parent is the decided block, and the checked height is the block's own.
#[test]
fn real_ledger_transfer_included_under_slot_height_drift() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let (under, adequate) = under_and_adequate(&ledger, &wallet, &coinbase(&ledger, &blocks[2]));
    let ledger = Arc::new(RwLock::new(ledger));
    let mut svc = solo_service_on(&ledger);
    let start = ledger.read().unwrap().get_chain_state().unwrap().height;
    assert_eq!(svc.current_slot(), start + 1);

    // Slot start+1 decides block start+1. A rival mint for the same height is
    // built first, on the same parent, with a different timestamp.
    let block = template(&ledger.read().unwrap(), &wallet, vec![]);
    let mut rival = block.clone();
    rival.minting_tx.timestamp += 1;
    rival.header.timestamp += 1;
    assert_ne!(rival.minting_tx.hash(), block.minting_tx.hash());
    let (_, applied) = decide_and_apply(&mut svc, &ledger, &block);
    let applied = applied.expect("block start+1 applies");

    // Slot start+2 decides the rival: the height is filled, nothing applies.
    let (_, none) = decide_and_apply(&mut svc, &ledger, &rival);
    assert!(none.is_none(), "the lost-race decision adds no block");
    let tip = ledger.read().unwrap().get_chain_state().unwrap();
    assert_eq!(tip.height, start + 1);
    assert_eq!(svc.current_slot(), start + 3, "a +1 slot/height drift");
    assert_eq!(
        svc.slot_parent_for_test(start + 3),
        Some(crate::consensus::SlotParent {
            height: applied.height(),
            hash: applied.hash(),
        })
    );

    for tx in [&under, &adequate] {
        svc.submit_transaction(tx.hash(), tx.fee, bincode::serialize(tx).unwrap());
    }
    let value = |tx: &Transaction| ConsensusValue::from_transaction(tx.hash(), tx.fee);
    let err = svc
        .validate_value_for_slot(start + 3, &value(&under))
        .unwrap_err();
    assert!(err.contains(&format!("(height {})", start + 2)), "{err}");
    assert!(err.contains("consensus fee floor"), "{err}");
    svc.validate_value_for_slot(start + 3, &value(&adequate))
        .unwrap();

    let next = template(&ledger.read().unwrap(), &wallet, vec![]);
    let (values, applied) = decide_and_apply(&mut svc, &ledger, &next);
    assert!(values.iter().any(|v| v.tx_hash == adequate.hash()));
    assert!(!values.iter().any(|v| v.tx_hash == under.hash()));
    let applied = applied.expect("the block after the drift applies");
    let hashes: Vec<_> = applied.transactions.iter().map(Transaction::hash).collect();
    assert_eq!(hashes, vec![adequate.hash()]);
    assert_eq!(applied.height(), start + 2);
}

/// #1920 review, on a real ledger: once the ledger moves past the slot's
/// decided parent (here a block applied directly, as gossip or sync would),
/// no transfer of that slot is Invalid, neither the one that is under the
/// floor on the parent nor the one the new block just spent. The ledger holds
/// the block built on the decided parent, so the slot is closed (decided by
/// a quorum) and its transfers get the intrinsic checks only; the seal-time
/// filter and the duplicate-height skip keep the apply safe.
#[test]
fn real_ledger_past_parent_gives_unavailable_not_invalid() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let (under, adequate) = under_and_adequate(&ledger, &wallet, &coinbase(&ledger, &blocks[2]));
    let ledger = Arc::new(RwLock::new(ledger));
    let mut svc = solo_service_on(&ledger);
    let block = template(&ledger.read().unwrap(), &wallet, vec![]);
    let (_, applied) = decide_and_apply(&mut svc, &ledger, &block);
    let applied = applied.unwrap();
    let slot = svc.current_slot();
    assert_eq!(
        svc.slot_parent_for_test(slot).map(|p| p.hash),
        Some(applied.hash())
    );
    for tx in [&under, &adequate] {
        svc.register_transfer_tx(tx.hash(), bincode::serialize(tx).unwrap());
    }
    let value = |tx: &Transaction| ConsensusValue::from_transaction(tx.hash(), tx.fee);
    let (under_floor, _) = {
        let l = ledger.read().unwrap();
        under_and_adequate(&l, &wallet, &coinbase(&l, &blocks[3]))
    };
    svc.register_transfer_tx(
        under_floor.hash(),
        bincode::serialize(&under_floor).unwrap(),
    );
    let err = svc
        .validate_value_for_slot(slot, &value(&under_floor))
        .unwrap_err();
    assert!(err.contains("invalid in a block"), "{err}");

    // The slot's block arrives without this node deciding it.
    super::mempool_tag_gate_tests::mine_with(
        &ledger.read().unwrap(),
        &wallet,
        vec![adequate.clone()],
    )
    .unwrap();
    for tx in [&under, &adequate, &under_floor] {
        svc.validate_value_for_slot(slot, &value(tx))
            .expect("a closed decided slot accepts on intrinsic checks, never Invalid");
    }
    // The slot's parent is unchanged: the closed-slot answer never moves it.
    assert_eq!(
        svc.slot_parent_for_test(slot).map(|p| p.hash),
        Some(applied.hash())
    );
}
