#![no_main]

use libfuzzer_sys::fuzz_target;

use botho::block::{Block, BlockHeader, MintingTx};

// Fuzz target for Block deserialization.
//
// Blocks are received during sync from potentially malicious peers.
// A malformed block must never cause crashes, only validation failures.
//
// Security rationale: During initial sync, nodes receive thousands of blocks.
// An attacker could craft malicious blocks to crash syncing nodes.
fuzz_target!(|data: &[u8]| {
    // Test full block deserialization
    let _ = bincode::deserialize::<Block>(data);

    // Test block header separately (smaller, parsed first in many contexts)
    let _ = bincode::deserialize::<BlockHeader>(data);

    // Test minting transaction
    let _ = bincode::deserialize::<MintingTx>(data);

    // If deserialization succeeds, verify methods don't panic
    if let Ok(block) = bincode::deserialize::<Block>(data) {
        let _ = block.hash();
        let _ = block.header.hash();
        let _ = block.header.height;
        let _ = block.header.timestamp;
        let _ = block.header.prev_block_hash;
        let _ = block.transactions.len();
        let _ = block.height();
        let _ = block.is_genesis();
        let _ = block.genesis_network();

        // Verify iteration over transactions doesn't panic
        for tx in &block.transactions {
            let _ = tx.hash();
        }
    }

    if let Ok(header) = bincode::deserialize::<BlockHeader>(data) {
        let _ = header.hash();
        let _ = header.height;
        let _ = header.is_genesis();
    }
});
