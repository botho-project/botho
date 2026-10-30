//! V2 block-validation hardening on the production ledger (#1670, #1677).
//!
//! - #1670 §1: the cheap header-only checks run BEFORE the V2 candidate scan,
//!   so a fork block (wrong parent) or a wrong-difficulty block is rejected
//!   with its precise error and never pays for a scan.
//! - #1670 §2: the strict scan's creating-block hash comes from the stored
//!   header (identical to a full decode) and fails closed on a corrupt row.
//! - #1677: `add_block` enforces the coinbase and ordinary-output ML-KEM
//!   ciphertext shape exactly like SCP's intrinsic validation, so a coinbase
//!   carrying `Some(vec![])` (which `MintingTx::hash` aliases with `None`) or
//!   any wrong-length ciphertext cannot be applied by sync or replay.
use super::{
    v2::{stored_block_hash, SCAN_BLOCK_LOOKUPS, STRICT_SCANS},
    *,
};
use crate::{
    consensus::{KEM_CIPHERTEXT_ENFORCED, ML_KEM_768_CIPHERTEXT_BYTES},
    transaction::{Transaction, TxOutput, Utxo, UtxoId, PICOCREDITS_PER_CREDIT},
    wallet::Wallet,
};
use tempfile::TempDir;

// Public deterministic test mnemonic, used only for temporary fixtures.
const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

/// A fixture address that publishes an ML-KEM-768 key (fixed seed), so the
/// coinbases and hybrid outputs built for it carry the 1088-byte ciphertext
/// `add_block` requires (#1677). Classical `AccountKey::random` addresses
/// publish none and yield KEM-less coinbases, which SCP and now `add_block`
/// both reject.
pub(super) fn kem_address(address: PublicAddress) -> PublicAddress {
    let kem = bth_crypto_pq::MlKem768KeyPair::from_seed(&[0x5A; 32]);
    address.with_pq_keys(kem.public_key().as_bytes().to_vec(), Vec::new())
}

fn open(dir: &Path) -> Ledger {
    let ledger = Ledger::open(dir).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    ledger
}

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
    block.header.timestamp = block.header.timestamp.max(state.tip_timestamp);
    block.minting_tx.timestamp = block.header.timestamp;
    block
}

fn mine(ledger: &Ledger, wallet: &Wallet) -> Block {
    let block = ledger.seal_block(template(ledger, wallet, vec![])).unwrap();
    ledger.add_block(&block).unwrap();
    block
}

fn scans() -> u64 {
    STRICT_SCANS.with(|n| n.get())
}

/// `add_block` rejects `block` with an `InvalidBlock` whose message contains
/// `needle`, writes nothing, and never starts a strict candidate scan.
fn assert_rejected_before_scan(ledger: &Ledger, block: &Block, needle: &str) {
    let before = ledger.get_chain_state().unwrap();
    let scans_before = scans();
    let error = ledger.add_block(block).expect_err(needle);
    match &error {
        LedgerError::InvalidBlock(msg) => {
            assert!(msg.contains(needle), "expected {needle:?}, got {msg:?}")
        }
        other => panic!("expected InvalidBlock({needle:?}), got {other:?}"),
    }
    assert_eq!(
        scans(),
        scans_before,
        "{needle}: rejected before any candidate scan"
    );
    let after = ledger.get_chain_state().unwrap();
    assert_eq!(
        (after.height, after.tip_hash),
        (before.height, before.tip_hash),
        "{needle}: rejected block wrote state"
    );
}

/// #1670 §1: a fork block (wrong parent, otherwise self-consistent) fails
/// with the precise "Previous block hash mismatch" and no scan; so do a wrong
/// height and a wrong difficulty. The honest block then scans exactly once.
#[test]
fn header_failures_are_rejected_before_the_candidate_scan() {
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    for _ in 0..3 {
        mine(&ledger, &wallet);
    }
    let honest = ledger
        .seal_block(template(&ledger, &wallet, vec![]))
        .unwrap();

    // Fork: builds on a different parent (header and minting tx agree).
    let mut fork = honest.clone();
    fork.header.prev_block_hash = [0xAB; 32];
    fork.minting_tx.prev_block_hash = [0xAB; 32];
    assert_rejected_before_scan(&ledger, &fork, "Previous block hash mismatch");

    let mut tall = honest.clone();
    tall.header.height += 1;
    tall.minting_tx.block_height += 1;
    assert_rejected_before_scan(&ledger, &tall, "Expected height");

    let mut easy = honest.clone();
    easy.header.difficulty -= 1;
    easy.minting_tx.difficulty -= 1;
    assert_rejected_before_scan(&ledger, &easy, "does not match expected");

    let scans_before = scans();
    ledger.add_block(&honest).unwrap();
    assert_eq!(scans(), scans_before + 1, "the honest block scans once");
}

/// #1677: a coinbase whose ML-KEM ciphertext is `Some(vec![])` (the
/// `MintingTx::hash` alias of `None`), `None`, or any length but 1088 is
/// rejected by `add_block` itself (not only by SCP's intrinsic check), before
/// the candidate scan and before any write.
#[test]
fn add_block_rejects_a_misshapen_coinbase_ciphertext() {
    assert!(KEM_CIPHERTEXT_ENFORCED, "test presumes a 7.0.0 + pq build");
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    mine(&ledger, &wallet);
    let honest = template(&ledger, &wallet, vec![]);
    assert_eq!(
        honest.minting_tx.kem_ciphertext.as_ref().map(Vec::len),
        Some(ML_KEM_768_CIPHERTEXT_BYTES),
        "the honest coinbase carries one ML-KEM-768 ciphertext"
    );

    // The alias #1677 names: `Some(vec![])` and `None` hash identically.
    let mut empty = honest.minting_tx.clone();
    empty.kem_ciphertext = Some(vec![]);
    let mut none = honest.minting_tx.clone();
    none.kem_ciphertext = None;
    assert_eq!(empty.hash(), none.hash());

    for (ciphertext, what) in [
        (Some(vec![]), "Some(empty)"),
        (None, "None"),
        (
            Some(vec![7u8; ML_KEM_768_CIPHERTEXT_BYTES - 1]),
            "1087 bytes",
        ),
        (
            Some(vec![7u8; ML_KEM_768_CIPHERTEXT_BYTES + 1]),
            "1089 bytes",
        ),
    ] {
        let mut forged = honest.clone();
        forged.minting_tx.kem_ciphertext = ciphertext;
        // Seal so the forged block is otherwise fully consistent (its V2
        // body root commits the forged minting tx).
        let forged = ledger.seal_block(forged).unwrap();
        let before = ledger.get_chain_state().unwrap();
        let error = ledger.add_block(&forged).expect_err(what);
        assert!(
            matches!(&error, LedgerError::InvalidBlock(m) if m.contains("Coinbase ML-KEM ciphertext")),
            "{what}: got {error:?}"
        );
        assert_eq!(ledger.get_chain_state().unwrap().tip_hash, before.tip_hash);
    }
    // The empty case is also rejected before any scan.
    let mut forged = honest.clone();
    forged.minting_tx.kem_ciphertext = Some(vec![]);
    let forged = ledger.seal_block(forged).unwrap();
    assert_rejected_before_scan(&ledger, &forged, "Coinbase ML-KEM ciphertext");

    ledger
        .add_block(&ledger.seal_block(honest).unwrap())
        .expect("the honest block still applies");
}

/// #1677 (ordinary outputs): `add_block` applies `validate_transfer_tx`'s
/// output-ciphertext shape rule to every ordinary transaction output.
#[test]
fn add_block_rejects_a_misshapen_transaction_output_ciphertext() {
    assert!(KEM_CIPHERTEXT_ENFORCED, "test presumes a 7.0.0 + pq build");
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    let mut blocks = Vec::new();
    // Enough coinbases for a full ring (MIN_RING_SIZE) of mature decoys.
    for _ in 0..30 {
        blocks.push(mine(&ledger, &wallet));
    }
    let input: Utxo = ledger
        .get_utxo(&UtxoId::new(blocks[2].hash(), 0))
        .unwrap()
        .unwrap();
    let fee = PICOCREDITS_PER_CREDIT;
    let height = ledger.get_chain_state().unwrap().height;
    let tx = wallet
        .create_private_transaction(
            std::slice::from_ref(&input),
            vec![TxOutput::new_hybrid_to_address(
                input.output.amount - fee,
                &wallet.default_address(),
                0,
                None,
                ClusterTagVector::empty(),
            )
            .unwrap()],
            fee,
            height,
            &ledger,
        )
        .unwrap();
    assert_eq!(
        tx.outputs[0].kem_ciphertext.as_ref().map(Vec::len),
        Some(ML_KEM_768_CIPHERTEXT_BYTES)
    );
    for (ciphertext, what) in [
        (Some(vec![]), "Some(empty)"),
        (None, "None"),
        (Some(vec![7u8; 33]), "33 bytes"),
    ] {
        let mut forged_tx = tx.clone();
        forged_tx.outputs[0].kem_ciphertext = ciphertext;
        let forged = ledger
            .seal_block(template(&ledger, &wallet, vec![forged_tx]))
            .unwrap();
        let before = ledger.get_chain_state().unwrap();
        let error = ledger.add_block(&forged).expect_err(what);
        assert!(
            matches!(&error, LedgerError::InvalidBlock(m)
                if m.contains("Transaction 0 output 0 ML-KEM ciphertext")),
            "{what}: got {error:?}"
        );
        assert_eq!(ledger.get_chain_state().unwrap().tip_hash, before.tip_hash);
    }
    ledger
        .add_block(
            &ledger
                .seal_block(template(&ledger, &wallet, vec![tx]))
                .unwrap(),
        )
        .expect("the honest spend still applies");
}

/// #1670 §2: the header-only creating-block hash equals the full-decode
/// `Block::hash` for every stored block, is `None` past the tip, and fails
/// closed (an error, never `None` or a wrong hash) on a truncated row or a
/// row whose header disagrees with its height key.
#[test]
fn stored_block_hash_matches_full_decode_and_fails_closed() {
    let dir = TempDir::new().unwrap();
    let ledger = open(dir.path());
    let wallet = Wallet::from_mnemonic(WORDS).unwrap();
    for _ in 0..3 {
        mine(&ledger, &wallet);
    }
    let next = template(&ledger, &wallet, vec![]);
    let full: Vec<[u8; 32]> = (0..=3)
        .map(|h| ledger.get_block(h).unwrap().hash())
        .collect();
    {
        let txn = ledger.env.read_txn().unwrap();
        for height in 0..=3 {
            assert_eq!(
                stored_block_hash(&ledger, &txn, height).unwrap(),
                Some(full[height as usize]),
                "height {height}"
            );
        }
        assert_eq!(stored_block_hash(&ledger, &txn, 4).unwrap(), None);
        // A strict scan reads each creating block at most once; no payout
        // exists yet, so this one reads none.
        let lookups = SCAN_BLOCK_LOOKUPS.with(|n| n.get());
        let view = ledger.production_view(&txn);
        let seed = crate::ledger::lottery_v2::seed(&view, &next).unwrap();
        crate::ledger::lottery_v2::DerivationReads::lottery_candidates(
            &view,
            &next,
            &seed,
            &crate::ledger::lottery_v2::canonical_config().draw_config,
        )
        .unwrap();
        assert_eq!(SCAN_BLOCK_LOOKUPS.with(|n| n.get()), lookups);
    }

    let row = {
        let txn = ledger.env.read_txn().unwrap();
        ledger.blocks_db.get(&txn, &2).unwrap().unwrap().to_vec()
    };
    let put = |height: u64, bytes: &[u8]| {
        let mut txn = ledger.env.write_txn().unwrap();
        ledger.blocks_db.put(&mut txn, &height, bytes).unwrap();
        txn.commit().unwrap();
    };
    // Truncated inside the header.
    put(2, &row[..100]);
    {
        let txn = ledger.env.read_txn().unwrap();
        assert!(matches!(
            stored_block_hash(&ledger, &txn, 2),
            Err(LedgerError::StorageEncoding(_))
        ));
    }
    // Block 2's (intact) row stored under height 3.
    put(2, &row);
    put(3, &row);
    let txn = ledger.env.read_txn().unwrap();
    assert!(matches!(
        stored_block_hash(&ledger, &txn, 3),
        Err(LedgerError::InconsistentRecord(_))
    ));
}
