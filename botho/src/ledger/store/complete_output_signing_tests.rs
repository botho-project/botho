//! #1638 / #1286 D0: the transaction signing hash commits to every output
//! field. A node-wallet-signed transaction is admitted; the same transaction
//! with `kem_ciphertext`, `e_memo` or `cluster_tags` mutated after signing is
//! rejected by ledger validation, block application and the mempool.
use super::*;
use crate::{
    ledger::{ChainState, UtxoSnapshot},
    mempool::Mempool,
    transaction::{MemoPayload, Transaction, TxOutput, Utxo, UtxoId, MIN_RING_SIZE},
    wallet::Wallet,
};
use bth_transaction_types::{ClusterId, ClusterTagEntry};

const HEIGHT: u64 = 100;
const INPUT_AMOUNT: u64 = 50_000_000_000_000;
const PAYMENT: u64 = 60_000_000_000_000;
const FEE: u64 = 1_000_000_000_000;
const CHANGE: u64 = 2 * INPUT_AMOUNT - PAYMENT - FEE;

fn block_with(ledger: &Ledger, tx: Transaction) -> Block {
    use bth_account_keys::AccountKey;
    use rand::{rngs::StdRng, SeedableRng};
    let state = ledger.get_chain_state().unwrap();
    let minter = AccountKey::random(&mut StdRng::seed_from_u64(1638));
    let mut block = Block::new_template_with_txs(
        &Block::genesis(),
        &minter.default_subaddress(),
        state.difficulty,
        calculate_block_reward(state.height + 1, state.total_mined),
        vec![tx],
    );
    block.header.height = state.height + 1;
    block.header.prev_block_hash = state.tip_hash;
    block.minting_tx.block_height = block.header.height;
    block.minting_tx.prev_block_hash = state.tip_hash;
    block.lottery_summary.amount_burned =
        LotteryFeeConfig::default().split_fees(block.total_fees()).1;
    while !block.header.is_valid_pow() {
        block.header.nonce += 1;
    }
    block.minting_tx.nonce = block.header.nonce;
    block
}

#[test]
fn output_field_mutations_after_signing_are_rejected_by_the_ledger() {
    // Public deterministic test mnemonics, used only for temporary fixtures.
    let sender = Wallet::from_mnemonic(
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
    )
    .unwrap();
    let recipient = Wallet::from_mnemonic(
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();

    let inputs: Vec<Utxo> = (0..2u8)
        .map(|i| Utxo {
            id: UtxoId::new([i + 1; 32], 0),
            output: TxOutput::new_hybrid_to_address(
                INPUT_AMOUNT,
                &sender.default_address(),
                0,
                None,
                ClusterTagVector::empty(),
            )
            .unwrap(),
            created_at: 1,
        })
        .collect();
    let mut utxos = inputs.clone();
    for index in 0..MIN_RING_SIZE + 2 {
        utxos.push(Utxo {
            id: UtxoId::new([index as u8 + 3; 32], 0),
            output: TxOutput::new(INPUT_AMOUNT, &recipient.default_address()),
            created_at: 1,
        });
    }
    let state = ChainState {
        height: HEIGHT,
        difficulty: u64::MAX,
        ..ChainState::default()
    };
    let snapshot = UtxoSnapshot::new(HEIGHT, [0; 32], state, utxos, vec![], vec![]).unwrap();
    ledger.load_from_snapshot(&snapshot, None).unwrap();

    let outputs = vec![
        TxOutput::new_hybrid_to_address(
            PAYMENT,
            &recipient.default_address(),
            0,
            Some(MemoPayload::destination("complete-output signing fixture")),
            ClusterTagVector::empty(),
        )
        .unwrap(),
        TxOutput::new_hybrid_to_address(
            CHANGE,
            &sender.default_address(),
            1,
            None,
            ClusterTagVector::empty(),
        )
        .unwrap(),
    ];
    let tx = sender
        .create_private_transaction_impl(&inputs, outputs, FEE, HEIGHT, &ledger, None)
        .unwrap();
    assert!(tx.outputs[0].e_memo.is_some());
    #[cfg(feature = "pq")]
    assert!(tx.outputs.iter().all(|o| o.kem_ciphertext.is_some()));

    // Control: the honestly signed transaction is admitted.
    tx.verify_ring_signatures().unwrap();
    ledger.verify_transaction(&tx).unwrap();
    ledger.verify_ring_members(&tx).unwrap();
    let mut mempool = Mempool::new();
    assert_eq!(mempool.add_tx(tx.clone(), &ledger).unwrap(), tx.hash());

    type Mutation = fn(&mut Transaction);
    let mut mutations: Vec<(&str, Mutation)> = vec![
        ("e_memo byte", |t| {
            t.outputs[0].e_memo.as_mut().unwrap().ciphertext[0] ^= 1
        }),
        ("e_memo strip", |t| t.outputs[0].e_memo = None),
        ("e_memo add", |t| {
            t.outputs[1].e_memo = t.outputs[0].e_memo.clone()
        }),
        ("cluster_tags add entry", |t| {
            t.outputs[0].cluster_tags.entries.push(ClusterTagEntry {
                cluster_id: ClusterId(7),
                weight: 1_000,
            })
        }),
        ("cluster_tags decay state", |t| {
            t.outputs[0].cluster_tags.decay_state =
                ClusterTagVector::empty_at_block(HEIGHT + 1).decay_state
        }),
        ("kem_ciphertext add", |t| {
            t.outputs[0].kem_ciphertext = Some(vec![0; 1088])
        }),
    ];
    if tx.outputs[0].kem_ciphertext.is_some() {
        mutations.push(("kem_ciphertext byte", |t| {
            t.outputs[0].kem_ciphertext.as_mut().unwrap()[0] ^= 1
        }));
        mutations.push(("kem_ciphertext strip", |t| {
            t.outputs[0].kem_ciphertext = None
        }));
    }
    for (name, mutate) in mutations {
        let mut forged = tx.clone();
        mutate(&mut forged);
        // Skip a no-op mutation (e.g. "add" when the field is already equal).
        assert_ne!(
            forged.outputs, tx.outputs,
            "{name}: mutation must change outputs"
        );
        assert_ne!(
            forged.signing_hash(),
            tx.signing_hash(),
            "{name}: signing hash"
        );
        assert_ne!(forged.hash(), tx.hash(), "{name}: tx hash");
        assert!(forged.verify_ring_signatures().is_err(), "{name}: CLSAG");
        assert!(
            matches!(ledger.verify_transaction(&forged),
                Err(LedgerError::InvalidBlock(m)) if m.contains("Invalid ring signature")),
            "{name}: ledger must reject on the ring signature"
        );
        assert!(
            ledger
                .add_block(&block_with(&ledger, forged.clone()))
                .is_err(),
            "{name}: block application must reject"
        );
        let mut fresh = Mempool::new();
        assert!(
            fresh.add_tx(forged, &ledger).is_err(),
            "{name}: mempool must reject"
        );
    }
    // Nothing was applied.
    assert_eq!(ledger.get_chain_state().unwrap().height, HEIGHT);
    for input in tx.inputs.clsag() {
        assert_eq!(ledger.is_key_image_spent(&input.key_image).unwrap(), None);
    }
}
