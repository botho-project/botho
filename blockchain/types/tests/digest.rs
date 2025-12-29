// Copyright (c) 2018-2022 The Botho Foundation

use bth_account_keys::AccountKey;
use bth_blockchain_test_utils::get_blocks_with_recipients;
use bth_blockchain_types::{Block, BlockVersion};
use bth_crypto_digestible_test_utils::*;
use bth_crypto_keys::RistrettoPrivate;
use bth_transaction_core::{tokens::Bth, tx::TxOut, Amount, Token};
use bth_util_from_random::FromRandom;
use bth_util_test_helper::{RngCore, RngType as FixedRng, SeedableRng};

fn test_accounts() -> Vec<AccountKey> {
    let mut rng: FixedRng = SeedableRng::from_seed([12u8; 32]);
    (0..5).map(|_i| AccountKey::random(&mut rng)).collect()
}

fn test_origin_tx_outs() -> Vec<TxOut> {
    let mut rng: FixedRng = SeedableRng::from_seed([11u8; 32]);

    let accounts = test_accounts();

    accounts
        .iter()
        .map(|acct| {
            TxOut::new(
                BlockVersion::ZERO,
                Amount {
                    value: rng.next_u32() as u64,
                    token_id: Bth::ID,
                },
                &acct.default_subaddress(),
                &RistrettoPrivate::from_random(&mut rng),
            )
            .expect("Could not create TxOut")
        })
        .collect()
}

#[test]
fn tx_out_digestible_ast() {
    let tx_out = &test_origin_tx_outs()[0];

    // NOTE: Values updated after TxOut structure change (fog hint removal)
    let expected_ast = ASTNode::from(ASTAggregate {
        context: b"test",
        name: b"TxOut".to_vec(),
        is_completed: true,
        elems: vec![
            ASTNode::from(ASTAggregate {
                context: b"amount",
                name: b"Amount".to_vec(),
                is_completed: true,
                elems: vec![
                    ASTNode::from(ASTPrimitive {
                        context: b"commitment",
                        type_name: b"ristretto",
                        data: vec![
                            206, 153, 255, 220, 215, 210, 74, 91, 111, 70, 206, 134, 248, 86, 85,
                            164, 29, 195, 87, 125, 222, 158, 46, 42, 11, 204, 217, 90, 242, 107,
                            230, 8,
                        ],
                    }),
                    ASTNode::from(ASTPrimitive {
                        context: b"masked_value",
                        type_name: b"uint",
                        data: vec![92, 252, 97, 92, 139, 187, 13, 169],
                    }),
                ],
            }),
            ASTNode::from(ASTPrimitive {
                context: b"target_key",
                type_name: b"ristretto",
                data: vec![
                    74, 108, 234, 245, 141, 135, 114, 232, 14, 111, 94, 94, 202, 223, 37, 96, 237,
                    23, 223, 163, 176, 238, 18, 38, 149, 117, 77, 63, 25, 93, 251, 42,
                ],
            }),
            ASTNode::from(ASTPrimitive {
                context: b"public_key",
                type_name: b"ristretto",
                data: vec![
                    170, 123, 183, 16, 205, 89, 67, 189, 49, 19, 247, 240, 142, 140, 239, 60, 157,
                    103, 149, 14, 115, 139, 43, 54, 196, 113, 5, 218, 109, 189, 122, 123,
                ],
            }),
        ],
    });

    digestible_test_case_ast("test", tx_out, expected_ast);
}

#[test]
fn origin_block_digestible_ast() {
    let origin = Block::new_origin_block(&test_origin_tx_outs());

    let root_element_ast = ASTNode::from(ASTAggregate {
        context: b"root_element",
        name: b"TxOutMembershipElement".to_vec(),
        is_completed: true,
        elems: vec![
            ASTNode::from(ASTAggregate {
                context: b"range",
                name: b"Range".to_vec(),
                is_completed: true,
                elems: vec![
                    ASTNode::from(ASTPrimitive {
                        context: b"from",
                        type_name: b"uint",
                        data: vec![0; 8],
                    }),
                    ASTNode::from(ASTPrimitive {
                        context: b"to",
                        type_name: b"uint",
                        data: vec![0; 8],
                    }),
                ],
            }),
            ASTNode::from(ASTPrimitive {
                context: b"hash",
                type_name: b"bytes",
                data: vec![0; 32],
            }),
        ],
    });

    digestible_test_case_ast(
        "root_element",
        &origin.root_element,
        root_element_ast.clone(),
    );

    // NOTE: Values updated after TxOut structure change (fog hint removal)
    let expected_ast = ASTNode::from(ASTAggregate {
        context: b"test",
        name: b"Block".to_vec(),
        is_completed: true,
        elems: vec![
            ASTNode::from(ASTPrimitive {
                context: b"id",
                type_name: b"bytes",
                data: vec![
                    39, 81, 163, 82, 37, 16, 193, 6, 101, 9, 124, 158, 212, 101, 32, 202, 79, 70,
                    11, 127, 28, 16, 21, 147, 80, 198, 37, 132, 76, 246, 102, 38,
                ],
            }),
            ASTNode::from(ASTPrimitive {
                context: b"version",
                type_name: b"uint",
                data: vec![0; 4],
            }),
            ASTNode::from(ASTPrimitive {
                context: b"parent_id",
                type_name: b"bytes",
                data: vec![0; 32],
            }),
            ASTNode::from(ASTPrimitive {
                context: b"index",
                type_name: b"uint",
                data: vec![0; 8],
            }),
            ASTNode::from(ASTPrimitive {
                context: b"cumulative_txo_count",
                type_name: b"uint",
                data: vec![5, 0, 0, 0, 0, 0, 0, 0],
            }),
            root_element_ast,
            ASTNode::from(ASTPrimitive {
                context: b"contents_hash",
                type_name: b"bytes",
                data: vec![
                    94, 68, 139, 62, 53, 52, 237, 30, 98, 86, 44, 94, 73, 63, 106, 253, 54, 125,
                    58, 47, 115, 184, 38, 254, 101, 211, 190, 96, 113, 196, 189, 153,
                ],
            }),
        ],
    });

    digestible_test_case_ast("test", &origin, expected_ast);
}

fn test_blockchain(block_version: BlockVersion) -> Vec<[u8; 32]> {
    let origin = Block::new_origin_block(&test_origin_tx_outs());

    let recipients = test_accounts()
        .iter()
        .map(|account| account.default_subaddress())
        .collect::<Vec<_>>();

    let mut rng = FixedRng::from_seed([10u8; 32]);
    get_blocks_with_recipients(block_version, 3, &recipients, 1, 5, 50, origin, &mut rng)
        .into_iter()
        .map(|block_data| {
            let hash = &block_data.block().contents_hash;
            // Sanity check
            assert_eq!(hash, &block_data.contents().hash());
            hash.0
        })
        .collect()
}

// Test digest of block contents at versions 0 and 1.
// NOTE: Values updated after TxOut structure change (fog hint removal)
#[test]
fn block_contents_digestible_v0() {
    assert_eq!(
        test_blockchain(BlockVersion::ZERO),
        [
            [
                244, 192, 93, 139, 11, 204, 144, 246, 158, 247, 159, 155, 117, 1, 87, 12, 235, 3,
                32, 11, 228, 80, 15, 20, 68, 184, 221, 134, 172, 20, 98, 28
            ],
            [
                89, 44, 79, 242, 135, 124, 111, 199, 207, 205, 87, 77, 175, 60, 213, 181, 180, 81,
                193, 49, 47, 66, 195, 168, 175, 211, 58, 108, 88, 206, 122, 148
            ],
            [
                214, 30, 140, 28, 131, 53, 253, 168, 62, 182, 206, 142, 112, 40, 90, 81, 192, 248,
                187, 27, 215, 152, 64, 157, 142, 130, 58, 59, 242, 65, 9, 184
            ]
        ]
    );
}

// NOTE: Values updated after TxOut structure change (fog hint removal)
#[test]
fn block_contents_digestible_v1() {
    assert_eq!(
        test_blockchain(BlockVersion::ONE),
        [
            [
                14, 196, 230, 52, 150, 168, 112, 108, 152, 151, 43, 238, 58, 40, 5, 241, 110, 101,
                211, 187, 90, 235, 201, 219, 251, 154, 238, 34, 14, 72, 139, 63
            ],
            [
                113, 164, 238, 77, 174, 22, 204, 61, 175, 209, 70, 180, 81, 65, 207, 51, 255, 113,
                76, 177, 168, 247, 240, 187, 96, 79, 18, 167, 124, 91, 216, 18
            ],
            [
                148, 135, 147, 246, 200, 106, 221, 113, 61, 231, 180, 172, 168, 170, 35, 123, 117,
                246, 156, 33, 171, 137, 38, 219, 67, 147, 199, 79, 116, 70, 88, 250
            ]
        ]
    );
}
