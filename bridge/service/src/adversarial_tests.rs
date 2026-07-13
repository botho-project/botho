// Copyright (c) 2024 The Botho Foundation

//! Adversarial peg-invariant property tests (bridge epic #816, Phase 3,
//! issue #829).
//!
//! The #825 property test
//! (`reserve::tests::prop_invariant_holds_across_mint_burn_sequences`)
//! drives randomized mint/burn sequences and asserts the reserve ledger's
//! locked total tracks the expected wrapped supply. This module EXTENDS that
//! coverage with the two chain events that test can't: **reorg-orphaned
//! deposits** (a locked deposit whose mint never lands because its BTH
//! deposit was reorged out — the backing must be released so the ledger does
//! not permanently overcount) interleaved with mints and burns.
//!
//! The peg invariant under attack (ADR 0003 / ADR 0005):
//!
//! ```text
//! locked_reserve_total() == Σ over chains of (locked backing value)
//! ```
//!
//! must hold after EVERY settled operation — mint (lock), burn (FIFO
//! release spend), and reorg-unwind (unlock the orphaned deposit's backing).
//! No interleaving may drive the locked total negative, overcount, or
//! undercount. All locked backing is fungible in the ledger, so the model
//! tracks a single per-chain expected value and reconciles it against the
//! ledger after each op.

use bth_bridge_core::Chain;
use proptest::prelude::*;
use uuid::Uuid;

use crate::db::Database;

fn setup_db() -> Database {
    let db = Database::open_in_memory().unwrap();
    db.migrate().unwrap();
    db
}

fn chain_of(ix: u8) -> Chain {
    if ix == 0 {
        Chain::Ethereum
    } else {
        Chain::Solana
    }
}

/// One step in a randomized mint / burn / reorg sequence.
#[derive(Debug, Clone)]
enum Op {
    /// A confirmed deposit is locked as backing for a pending mint.
    Mint { chain_ix: u8, net: u64 },
    /// A pending (not-yet-confirmed) deposit's mint is finalized — after
    /// this the deposit is beyond the finality window and can never be
    /// reorged out.
    Confirm { pick: u8 },
    /// A reorg orphans a still-pending deposit before its mint lands: the
    /// locked backing must be unwound so the ledger stops counting it.
    Reorg { pick: u8 },
    /// A confirmed burn spends `fraction`% of a chain's locked backing FIFO.
    Burn { chain_ix: u8, fraction: u8 },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0u8..2, 1u64..5_000_000).prop_map(|(chain_ix, net)| Op::Mint { chain_ix, net }),
        1 => (0u8..255).prop_map(|pick| Op::Confirm { pick }),
        1 => (0u8..255).prop_map(|pick| Op::Reorg { pick }),
        2 => (0u8..2, 1u8..=100).prop_map(|(chain_ix, fraction)| Op::Burn { chain_ix, fraction }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Peg invariant holds across randomized mint / burn / reorg sequences.
    #[test]
    fn prop_invariant_survives_reorg_interleaving(
        ops in proptest::collection::vec(op_strategy(), 1..48)
    ) {
        let db = setup_db();
        // Per-chain expected locked backing (the chain-side ground truth the
        // ledger must equal). All locked backing is fungible.
        let mut expected = [0u128; 2];
        // Pending deposits that can still be reorged out: (order id, chain, net).
        let mut pending: Vec<(Uuid, u8, u64)> = Vec::new();

        for op in ops {
            match op {
                Op::Mint { chain_ix, net } => {
                    let chain = chain_of(chain_ix);
                    let order = Uuid::new_v4();
                    let output_id = format!("dep:{}", order);
                    prop_assert!(db
                        .record_locked_output(&output_id, chain, net, &order)
                        .unwrap());
                    expected[chain_ix as usize] += net as u128;
                    pending.push((order, chain_ix, net));
                }
                Op::Confirm { pick } => {
                    // Finalize a pending deposit: it leaves the reorg-eligible
                    // set (its backing stays locked and now backs real supply).
                    if !pending.is_empty() {
                        let idx = pick as usize % pending.len();
                        pending.remove(idx);
                    }
                }
                Op::Reorg { pick } => {
                    // Orphan a still-pending deposit: unwind its backing.
                    if pending.is_empty() {
                        continue;
                    }
                    let idx = pick as usize % pending.len();
                    let (order, chain_ix, net) = pending[idx];
                    // Only unwind when the chain's locked backing can cover
                    // the net (FIFO burns may already have consumed part of
                    // this deposit's value; the value-based unlock needs the
                    // remaining locked total to cover it).
                    if expected[chain_ix as usize] >= net as u128 {
                        let unlocked = db
                            .unlock_backing_for_order(&order, chain_of(chain_ix), net)
                            .unwrap();
                        prop_assert!(unlocked, "first unwind of a deposit must apply");
                        expected[chain_ix as usize] -= net as u128;
                        pending.remove(idx);

                        // A second unwind of the SAME orphaned deposit is a
                        // no-op (exactly-once): a duplicated reorg signal can
                        // never double-release the backing.
                        let again = db
                            .unlock_backing_for_order(&order, chain_of(chain_ix), net)
                            .unwrap();
                        prop_assert!(!again, "re-unwinding an orphaned deposit must be a no-op");
                    }
                }
                Op::Burn { chain_ix, fraction } => {
                    let outstanding = expected[chain_ix as usize];
                    let amount = (outstanding * fraction as u128 / 100).min(outstanding) as u64;
                    if amount == 0 {
                        continue;
                    }
                    let chain = chain_of(chain_ix);
                    let release = Uuid::new_v4();
                    prop_assert!(db.apply_release_spend(&release, chain, amount).unwrap());
                    expected[chain_ix as usize] -= amount as u128;
                }
            }

            // The peg invariant after every settled op — per chain and total.
            let expected_total: u128 = expected.iter().sum();
            prop_assert_eq!(db.locked_reserve_total().unwrap() as u128, expected_total);
            prop_assert_eq!(
                db.locked_reserve_by_chain(Chain::Ethereum).unwrap() as u128,
                expected[0]
            );
            prop_assert_eq!(
                db.locked_reserve_by_chain(Chain::Solana).unwrap() as u128,
                expected[1]
            );
        }

        // Terminal safety: a burn exceeding the outstanding locked backing
        // can never settle (no interleaving drives the total negative).
        let over = db.locked_reserve_by_chain(Chain::Ethereum).unwrap() + 1;
        prop_assert!(db
            .apply_release_spend(&Uuid::new_v4(), Chain::Ethereum, over)
            .is_err());
    }
}
