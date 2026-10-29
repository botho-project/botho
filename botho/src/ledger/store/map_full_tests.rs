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
    assert_eq!(ledger.get_chain_state().unwrap().height, height);
    ledger.add_block(&refused).unwrap();
    assert_eq!(ledger.get_chain_state().unwrap().height, height + 1);
}
