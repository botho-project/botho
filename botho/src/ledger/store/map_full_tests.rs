//! LMDB map exhaustion: what the ledger does when `data.mdb` reaches the
//! configured map size.
use super::*;
use tempfile::tempdir;

/// 1 MiB: a multiple of both 4 KiB and 16 KiB pages, large enough to hold
/// genesis, small enough to fill with a few hundred blocks.
const SMALL_MAP: usize = 1024 * 1024;
/// Upper bound on blocks written, so a map that never fills fails the test
/// instead of looping.
const MAX_BLOCKS: u64 = 100_000;

/// Next empty trivial-PoW, V2-sealed block on the current tip.
fn next_block(ledger: &Ledger, minter: &PublicAddress) -> Block {
    let state = ledger.get_chain_state().unwrap();
    let mut block = Block::new_template_with_txs(
        &Block::genesis(),
        minter,
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        vec![],
    );
    block.header.height = state.height + 1;
    block.header.prev_block_hash = state.tip_hash;
    block.minting_tx.block_height = block.header.height;
    block.minting_tx.prev_block_hash = state.tip_hash;
    while !block.header.is_valid_pow() {
        block.header.nonce += 1;
    }
    block.minting_tx.nonce = block.header.nonce;
    // V2 body root and summary (protocol 7.0.0, #1640).
    ledger.seal_block(block).unwrap()
}

/// Fill a small map with valid blocks until LMDB refuses a write, then check
/// that the failure is a node-local storage error, that the refused block left
/// no partial state, and that reopening with a larger map recovers.
#[test]
fn map_full_fails_closed_and_recovers_with_larger_map() {
    use rand::{rngs::StdRng, SeedableRng};
    let dir = tempdir().unwrap();
    let minter = super::block_validation_tests::kem_address(
        AccountKey::random(&mut StdRng::seed_from_u64(1618)).default_subaddress(),
    );

    let (height, refused) = {
        let ledger = Ledger::open_with_map_size(dir.path(), SMALL_MAP).unwrap();
        ledger.set_difficulty(u64::MAX).unwrap();
        let (error, block) = loop {
            let height = ledger.get_chain_state().unwrap().height;
            assert!(
                height < MAX_BLOCKS,
                "map never filled after {MAX_BLOCKS} blocks"
            );
            let block = next_block(&ledger, &minter);
            if let Err(error) = ledger.add_block(&block) {
                break (error, block);
            }
        };

        // A full map is a local storage fault. It must not be reported as an
        // invalid block, which would make this node reject a block its peers
        // accept.
        match &error {
            LedgerError::Database(message) => assert!(
                message.contains("MDB_MAP_FULL"),
                "expected an LMDB map-full error, got {message:?}"
            ),
            other => panic!("map exhaustion must be LedgerError::Database, got {other:?}"),
        }

        // The refused block committed nothing, and the ledger still reads.
        let state = ledger.get_chain_state().unwrap();
        assert_eq!(
            state.height + 1,
            block.header.height,
            "refused block advanced height"
        );
        assert_eq!(ledger.get_tip().unwrap().hash(), state.tip_hash);
        assert!(
            ledger.get_block(state.height + 1).is_err(),
            "refused block is readable"
        );

        assert!(error.is_map_full(), "{error:?}");

        // Status/metrics see the full map (#1535): the flag is set and the
        // high-water mark is at the map.
        let usage = ledger.storage_usage();
        assert!(usage.map_full, "{usage:?}");
        assert_eq!(usage.map_size_bytes, SMALL_MAP as u64);
        assert!(
            usage.used_bytes <= usage.map_size_bytes && usage.used_fraction() > 0.9,
            "{usage:?}"
        );

        // Retrying without more space fails the same way.
        assert!(matches!(
            ledger.add_block(&block),
            Err(LedgerError::Database(_))
        ));
        eprintln!(
            "map full at height {} with a {} KiB map",
            state.height,
            SMALL_MAP / 1024
        );
        (state.height, block)
    };

    // Reopening with a larger map keeps the chain and accepts the refused block.
    let ledger = Ledger::open_with_map_size(dir.path(), 4 * SMALL_MAP).unwrap();
    assert!(!ledger.storage_usage().map_full);
    assert_eq!(ledger.get_chain_state().unwrap().height, height);
    ledger.add_block(&refused).unwrap();
    assert_eq!(ledger.get_chain_state().unwrap().height, height + 1);
}

/// The production map (#1535; at least 64 GiB on 64-bit hosts, checked at
/// compile time next to `LEDGER_MAP_SIZE`) opens on this platform, and reports
/// its usage. Only address space is reserved: the data file stays small.
#[test]
fn production_map_size_opens_and_reports_usage() {
    let dir = tempdir().unwrap();
    let ledger = Ledger::open_with_map_size(dir.path(), LEDGER_MAP_SIZE).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    use rand::{rngs::StdRng, SeedableRng};
    let minter = super::block_validation_tests::kem_address(
        AccountKey::random(&mut StdRng::seed_from_u64(1535)).default_subaddress(),
    );
    let before = ledger.storage_usage();
    for _ in 0..3 {
        let block = next_block(&ledger, &minter);
        ledger.add_block(&block).unwrap();
    }
    let after = ledger.storage_usage();
    assert_eq!(after.map_size_bytes, LEDGER_MAP_SIZE as u64);
    assert!(!after.map_full);
    assert!(after.used_bytes >= before.used_bytes && after.used_bytes > 0);
    assert!(after.used_fraction() < 0.001, "{after:?}");
    let file = std::fs::metadata(dir.path().join("data.mdb"))
        .unwrap()
        .len();
    assert!(
        file < 64 * 1024 * 1024,
        "data file is {file} B: the map must be sparse address space"
    );
}

/// A non-map-full database error, and an invalid block, are not mistaken
/// for a full map.
#[test]
fn only_map_full_errors_classify_as_map_full() {
    assert!(LedgerError::Database(
        "Failed to commit: MDB_MAP_FULL: Environment mapsize limit reached".into()
    )
    .is_map_full());
    assert!(
        !LedgerError::Database("Failed to start write txn: Invalid argument".into()).is_map_full()
    );
    assert!(!LedgerError::InvalidBlock("MDB_MAP_FULL".into()).is_map_full());
}
