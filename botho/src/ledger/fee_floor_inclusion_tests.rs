//! #1920: an admitted transfer that fails block validation at the block's own
//! height must never reach an applied block.
//!
//! Real, signed spends on a mined chain (the #1858 fixture helpers in
//! `mempool_tag_gate_tests.rs`), checked through the three enforcement points:
//! the block builder's seal-time skip (`BlockBuilder::finalize_for_apply`), the
//! SCP value check for a slot (`ledger_transfer_slot_check` inside
//! `ConsensusService`'s validity function), and the ledger primitive both use
//! (`Ledger::check_transfer_for_block`). Kept out of `store.rs` so the tests do
//! not churn that file's SHA-256 pin in
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

/// The ledger primitive: a verdict only for a block on the current tip.
#[test]
fn check_transfer_for_block_verdicts() {
    let (_dir, ledger, wallet, blocks) = chain(30);
    let (under, adequate) = under_and_adequate(&ledger, &wallet, &coinbase(&ledger, &blocks[2]));
    let tip = ledger.get_chain_state().unwrap().height;

    assert_eq!(
        ledger.check_transfer_for_block(&adequate, tip + 1).unwrap(),
        TransferBlockCheck::Valid
    );
    match ledger.check_transfer_for_block(&under, tip + 1).unwrap() {
        TransferBlockCheck::Invalid(reason) => {
            assert!(reason.contains("consensus fee floor"), "{reason}")
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    for height in [tip, tip + 2] {
        assert_eq!(
            ledger.check_transfer_for_block(&adequate, height).unwrap(),
            TransferBlockCheck::NotAtParent { tip_height: tip }
        );
    }
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
