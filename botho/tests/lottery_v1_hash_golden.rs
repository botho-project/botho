//! Freeze actual pre-proposal V1 kernel hashing; this does not activate V2.
//! Source provenance: BlockHeader::hash / Block::hash at e243d10d.
//! Expected SHA-256 independently computed from explicit LE header bytes.
use botho::block::{Block, BlockHeader};
#[test]
fn v1_header_and_block_hash_stay_frozen() {
    let header = BlockHeader {
        version: 1,
        prev_block_hash: [2; 32],
        tx_root: [3; 32],
        timestamp: 4,
        height: 5,
        difficulty: 6,
        nonce: 7,
        minter_view_key: [8; 32],
        minter_spend_key: [9; 32],
    };
    let expected = "edaa253a91ac3ec3e3f6657a427e05ef8e336e4330b62074edd7cdc5a05c08bb";
    assert_eq!(hex::encode(header.hash()), expected);
    let mut block = Block::genesis();
    block.header = header;
    assert_eq!(hex::encode(block.hash()), expected);
    // V1 really is header-only. Do not silently activate a body commitment.
    block.lottery_summary.amount_burned = 123;
    block.minting_tx.target_key = [42; 32];
    assert_eq!(hex::encode(block.hash()), expected);
}
