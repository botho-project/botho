//! The CLI `botho balance` / `botho send` cores against the production
//! `botho.ledger.2` wallet fixture (#1662).
//!
//! Both commands used to look outputs up by address, which can never match a
//! one-time stealth target key, so they saw no outputs at all. They now go
//! through P7's view-key discovery (`unspent_wallet_outputs`), the same path
//! as `wallet_getBalance`.
use super::{fixture::*, *};
use crate::commands::{balance, send};
use std::collections::BTreeSet;

/// BIP39 test-vector mnemonic for a fresh CLI wallet (test only).
const CLI_WORDS: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// One-time key images of a transaction's CLSAG inputs.
fn key_images(tx: &crate::transaction::Transaction) -> BTreeSet<[u8; 32]> {
    tx.inputs
        .clsag()
        .iter()
        .map(|input| input.key_image)
        .collect()
}

/// Verify `tx` against the ledger and apply it in a new block.
fn confirm(f: &Fixture, tx: crate::transaction::Transaction) {
    tx.verify_ring_signatures().unwrap();
    f.ledger.verify_transaction(&tx).unwrap();
    f.ledger.verify_ring_members(&tx).unwrap();
    let block = f.block_with(vec![tx]);
    f.ledger.add_block(&block).unwrap();
}

/// Apply `count` empty blocks (spend/decoy maturity padding).
fn pad(f: &Fixture, count: usize) {
    for _ in 0..count {
        let block = f.block_with(vec![]);
        f.ledger.add_block(&block).unwrap();
    }
}

#[test]
fn cli_balance_counts_stealth_outputs_and_v2_payouts() {
    let f = Fixture::build();
    // The retired address lookup found nothing: every output is stealth.
    assert!(f
        .ledger
        .get_utxos_for_address(&f.owner.default_address())
        .unwrap()
        .is_empty());

    let summary = balance::summarize(&f.ledger, &f.owner).unwrap();
    assert_eq!(summary.picocredits, f.owned_total());
    assert_eq!(summary.utxo_count, f.owned_ids().len());
    assert_eq!(summary.lottery_payout_count, f.payouts.len());

    // CLI balance agrees with the discovery the wallet RPC uses.
    let unspent = f.ledger.unspent_wallet_outputs(&f.owner).unwrap();
    assert_eq!(
        summary.picocredits,
        unspent.iter().map(|o| o.utxo.output.amount).sum::<u64>()
    );
    assert_eq!(summary.utxo_count, unspent.len());
}

#[test]
fn cli_send_spends_v2_payouts_and_balance_sees_the_change() {
    let f = Fixture::build();
    let discovered: BTreeSet<_> = f
        .ledger
        .unspent_wallet_outputs(&f.owner)
        .unwrap()
        .iter()
        .map(|o| o.key_image)
        .collect();
    // Leaving less than the smallest owned output (20 BTH) forces every
    // input, the five V2 payouts included, into the send.
    let amount = f.owned_total() - 10 * FEE;
    let prepared = send::prepare(
        &f.ledger,
        &f.owner,
        &f.other.default_address(),
        amount,
        None,
    )
    .unwrap();
    assert_eq!(prepared.tx.inputs.len(), f.owned_ids().len());
    assert_eq!(key_images(&prepared.tx), discovered);
    assert!(prepared.change > 0);
    assert_eq!(f.owned_total() - amount - prepared.fee, prepared.change);
    // A seven-input send outgrows the typical-size fee estimate; the CLI
    // re-signs it at the consensus floor so the block accepts it.
    let floor = f.ledger.consensus_fee_floor(&prepared.tx, TIP + 1).unwrap();
    assert!(
        prepared.fee >= floor,
        "fee {} < floor {floor}",
        prepared.fee
    );
    confirm(&f, prepared.tx);

    // Every input is now spent; the change is a fresh stealth self-send
    // (hybrid, output index 1) that the CLI wallet discovers.
    let summary = balance::summarize(&f.ledger, &f.owner).unwrap();
    assert_eq!(summary.utxo_count, 1);
    assert_eq!(summary.lottery_payout_count, 0);
    assert_eq!(summary.picocredits, prepared.change);
}

#[test]
fn cli_wallet_receives_and_spends_a_stealth_output() {
    let f = Fixture::build();
    let cli = Wallet::from_mnemonic(CLI_WORDS).unwrap();
    assert_eq!(
        balance::summarize(&f.ledger, &cli).unwrap(),
        balance::WalletBalance::default()
    );

    let received = 30 * FEE;
    let payment = send::prepare(
        &f.ledger,
        &f.owner,
        &cli.default_address(),
        received,
        Some("cli stealth receive"),
    )
    .unwrap();
    confirm(&f, payment.tx);

    // The received one-time output shows up in balance, though the address
    // lookup still cannot see it.
    let summary = balance::summarize(&f.ledger, &cli).unwrap();
    assert_eq!(summary.picocredits, received);
    assert_eq!(summary.utxo_count, 1);
    assert_eq!(summary.lottery_payout_count, 0);
    assert!(f
        .ledger
        .get_utxos_for_address(&cli.default_address())
        .unwrap()
        .is_empty());

    // Once mature, `send` spends it.
    pad(&f, 12);
    let sent = 10 * FEE;
    let spend = send::prepare(&f.ledger, &cli, &f.owner.default_address(), sent, None).unwrap();
    assert_eq!(spend.tx.inputs.len(), 1);
    let change = received - sent - spend.fee;
    assert_eq!(spend.change, change);
    confirm(&f, spend.tx);
    let summary = balance::summarize(&f.ledger, &cli).unwrap();
    assert_eq!(summary.picocredits, change);
    assert_eq!(summary.utxo_count, 1);
}

#[test]
fn cli_balance_and_send_exclude_legacy_aliases() {
    let f = Fixture::build();
    // A V1-shaped alias of the hybrid source (same target key, later
    // outpoint), exactly as in the node-wallet exclusion test.
    let original = f.utxo(&f.hybrid_source);
    let alias = Utxo {
        id: UtxoId::new([0xcc; 32], 3),
        output: original.output.clone(),
        created_at: original.created_at,
    };
    let mut txn = f.ledger.env.write_txn().unwrap();
    f.ledger
        .utxo_db
        .put(
            &mut txn,
            &alias.id.to_bytes(),
            &bincode::serialize(&alias).unwrap(),
        )
        .unwrap();
    let context = StoredContext::Direct {
        height: alias.created_at,
        base_index: 3,
    };
    f.ledger
        .contexts_db
        .put(&mut txn, &alias.id.to_bytes(), &context.encode().unwrap())
        .unwrap();
    f.ledger.add_to_address_index(&mut txn, &alias).unwrap();
    txn.commit().unwrap();

    let summary = balance::summarize(&f.ledger, &f.owner).unwrap();
    assert_eq!(summary.picocredits, f.owned_total());
    assert_eq!(summary.utxo_count, f.owned_ids().len());

    // Counting the alias would cover this; excluding it cannot.
    let error = send::prepare(
        &f.ledger,
        &f.owner,
        &f.other.default_address(),
        f.owned_total() + FEE,
        None,
    )
    .err()
    .unwrap();
    assert!(
        error.to_string().contains("Insufficient balance"),
        "{error}"
    );

    // A send of everything selects each canonical output once, never the
    // alias (which would repeat the source's key image).
    let prepared = send::prepare(
        &f.ledger,
        &f.owner,
        &f.other.default_address(),
        f.owned_total() - 10 * FEE,
        None,
    )
    .unwrap();
    assert_eq!(prepared.tx.inputs.len(), f.owned_ids().len());
    assert_eq!(key_images(&prepared.tx).len(), f.owned_ids().len());
    confirm(&f, prepared.tx);
}
