//! `chain_getOutputs` lottery payout rows: the frozen V2 wire contract
//! (#1641 / #1286 P4).
//!
//! P3 (protocol 7.0.0 producer/validator) is not on this branch, so real V2
//! payouts cannot be mined yet. These tests seed fixture blocks through the
//! shared writer (`Ledger::seed_fixture_block_unvalidated`) with payout
//! records, i.e. the `botho.ledger.2` Lottery contexts P3 will produce. The
//! legacy (V1-shaped) row is never emitted by the handler (a payout without a
//! context is corruption, -32603); its golden is built from the pure row
//! helper so consumers can test their exclusion path.
//!
//! Golden rows live in `botho/tests/fixtures/rpc-lottery-rows.json`.
//! Regenerate with `BOTHO_BLESS_RPC_GOLDEN=1` only for a deliberate,
//! contract-reviewed wire change: P5/P6/P8/P9 consume this shape.
use super::*;
use crate::{
    block::{Block, LotteryOutput},
    ledger::{DerivationContext, Ledger, LOTTERY_RULES_VERSION},
    mempool::Mempool,
    transaction::{Transaction, TxOutput, Utxo, UtxoId},
};
use bth_transaction_clsag::lottery_v2::{Context, Outpoint, Record};
use bth_transaction_types::{ClusterId, ClusterTagVector};
use tempfile::TempDir;

const GOLDEN: &str = "tests/fixtures/rpc-lottery-rows.json";
/// Derivation index j of the winning output. Deliberately different from the
/// payout's outpoint index (1) so a consumer conflating them fails.
const WINNER_INDEX: u32 = 2;

fn golden_block() -> Block {
    let golden: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/v1-writer-effects.json")).unwrap();
    bincode::deserialize(&hex::decode(golden["block_hex"].as_str().unwrap()).unwrap()).unwrap()
}

/// A canonical, non-zero scalar tweak T.
fn tweak() -> [u8; 32] {
    let mut t = [0u8; 32];
    t[0] = 7;
    t[1] = 0x5a;
    t
}

fn tagged_output(seed: u8) -> TxOutput {
    TxOutput {
        amount: 1_000 + seed as u64,
        target_key: [seed; 32],
        public_key: [seed.wrapping_add(1); 32],
        e_memo: None,
        // Two clusters, so an emitted-empty or truncated vector is visible.
        cluster_tags: ClusterTagVector::from_pairs(&[
            (ClusterId(0x1111), 300_000),
            (ClusterId(0x2222), 500_000 + seed as u32),
        ]),
        kem_ciphertext: None,
    }
}

fn child(parent: &Block) -> Block {
    let mut block = parent.clone();
    block.header.height = parent.height() + 1;
    block.header.prev_block_hash = parent.hash();
    block.transactions = vec![];
    block.lottery_outputs = vec![];
    block
}

/// The seeded chain: block 1 = the golden V1 block (live `add_block`),
/// block 2 = one stub transaction with three tagged outputs, block 3 = one
/// payout to block 2's output `WINNER_INDEX`.
struct Chain {
    _dir: TempDir,
    state: RpcState,
    winner: UtxoId,
    b3: Block,
}

/// Seeded with `botho.ledger.2` contexts (Lottery for the payout).
fn seeded_chain() -> Chain {
    let dir = TempDir::new().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    ledger.set_difficulty(u64::MAX).unwrap();
    let b1 = golden_block();
    ledger.add_block(&b1).unwrap();

    let mut b2 = child(&b1);
    let mut tx = Transaction::new_stub_with_fee(0);
    tx.outputs = vec![
        tagged_output(0x31),
        tagged_output(0x32),
        tagged_output(0x33),
    ];
    let winner = UtxoId::new(tx.hash(), WINNER_INDEX);
    b2.transactions = vec![tx];
    ledger.seed_fixture_block_unvalidated(&b2, &[]).unwrap();

    let mut b3 = child(&b2);
    let record = Record {
        ordinal: 0,
        winner: Outpoint {
            hash: winner.tx_hash,
            index: winner.output_index,
        },
        amount: 77,
        target: [0x41; 32],
        public_key: [0x42; 32],
        ciphertext: None,
        context: Context {
            base_index: WINNER_INDEX,
            tweak: tweak(),
        },
    };
    b3.lottery_outputs = vec![LotteryOutput {
        winner_tx_hash: winner.tx_hash,
        winner_output_index: winner.output_index,
        payout: record.amount,
        target_key: record.target,
        public_key: record.public_key,
        kem_ciphertext: None,
    }];
    let records = [record];
    ledger
        .seed_fixture_block_unvalidated(&b3, &records)
        .unwrap();

    let state = RpcState::new(
        ledger,
        Mempool::new(),
        Network::Testnet,
        None,
        None,
        vec![],
        Arc::new(WsBroadcaster::new(16)),
    );
    Chain {
        _dir: dir,
        state,
        winner,
        b3,
    }
}

async fn lottery_rows(state: &RpcState, height: u64) -> Vec<Value> {
    let resp = handle_get_outputs(
        json!(1),
        &json!({"start_height": height, "end_height": height}),
        state,
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
    let blocks = resp.result.unwrap();
    blocks[0]["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["lottery"] == json!(true))
        .cloned()
        .collect()
}

fn stored_tags(state: &RpcState, id: &UtxoId) -> Value {
    let utxo = state.ledger.read().unwrap().get_utxo(id).unwrap().unwrap();
    json!(cluster_tags_json(&utxo.output.cluster_tags))
}

/// Compare `actual` with the golden entry `key`, or rewrite it when blessing.
fn check_golden(key: &str, actual: &Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::var_os("BOTHO_BLESS_RPC_GOLDEN").is_some() {
        let mut golden: Value = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| json!({}));
        golden[key] = actual.clone();
        std::fs::write(&path, serde_json::to_string_pretty(&golden).unwrap() + "\n").unwrap();
        return;
    }
    let golden: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        actual, &golden[key],
        "chain_getOutputs {key} lottery row drifted from {GOLDEN} (frozen #1641 contract)"
    );
}

#[tokio::test]
async fn v2_lottery_row_matches_golden_and_frozen_contract() {
    let chain = seeded_chain();
    let rows = lottery_rows(&chain.state, 3).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    check_golden("v2", row);

    // Contract fields, independently of the golden bytes.
    let payout = UtxoId::new(chain.b3.hash(), 1);
    assert_eq!(row["outputIndex"], json!(1));
    assert_eq!(row["cryptoOutputIndex"], json!(WINNER_INDEX));
    assert_eq!(
        row["cryptoOutputIndex"], row["lotteryContext"]["baseIndex"],
        "cryptoOutputIndex is the base index j, not the payout outpoint index"
    );
    assert_eq!(
        row["lotteryContext"],
        json!({
            "version": 2,
            "baseIndex": WINNER_INDEX,
            "tweak": hex::encode(tweak()),
            "ordinal": 0,
            "height": 3,
        })
    );
    let tweak_hex = row["lotteryContext"]["tweak"].as_str().unwrap();
    assert_eq!(tweak_hex.len(), 64);
    assert_eq!(tweak_hex, tweak_hex.to_lowercase());
    assert_eq!(
        row["winnerOutpoint"],
        json!(format!(
            "{}:{}",
            hex::encode(chain.winner.tx_hash),
            chain.winner.output_index
        ))
    );
    assert_eq!(row["targetKey"], json!(hex::encode([0x41; 32])));

    // The emitted tags ARE the stored payout UTXO's tags (non-empty).
    let tags = stored_tags(&chain.state, &payout);
    assert_eq!(tags.as_array().unwrap().len(), 2);
    assert_eq!(row["clusterTags"], tags);

    // And the accessor the row was built from agrees.
    let (_, context) = chain
        .state
        .ledger
        .read()
        .unwrap()
        .get_output_derivation_context(&payout)
        .unwrap();
    assert_eq!(
        context,
        DerivationContext::Lottery {
            height: 3,
            ordinal: 0,
            base_index: WINNER_INDEX,
            tweak: tweak(),
        }
    );
}

/// The legacy (V1-shaped) row, built from the pure helper: the handler never
/// emits it on a `botho.ledger.2` store.
#[test]
fn v1_legacy_lottery_row_matches_golden_and_omits_context() {
    let (lottery_output, _) = row_inputs();
    let row = lottery_payout_row(&[0x0b; 32], 0, &lottery_output, None).unwrap();
    check_golden("v1", &row);
    // Legacy marker: no context, so consumers exclude the row (#1450).
    for field in ["lotteryContext", "cryptoOutputIndex", "winnerOutpoint"] {
        assert!(row.get(field).is_none(), "legacy row must omit {field}");
    }
    assert_eq!(row["lottery"], json!(true));
}

/// A payout whose context is missing (corruption on `botho.ledger.2`) fails
/// the call closed instead of degrading to a legacy row (#1655 review).
#[tokio::test]
async fn missing_payout_context_fails_closed() {
    let chain = seeded_chain();
    let payout = UtxoId::new(chain.b3.hash(), 1);
    assert!(chain
        .state
        .ledger
        .read()
        .unwrap()
        .fixture_delete_derivation_context(&payout)
        .unwrap());
    let resp = handle_get_outputs(
        json!(1),
        &json!({"start_height": 1, "end_height": 3}),
        &chain.state,
    )
    .await;
    assert!(resp.result.is_none());
    let error = resp.error.expect("missing context must be an error");
    assert_eq!(error.code, -32603);
    assert!(error.message.contains("context"), "{}", error.message);
}

/// Ordinary rows keep their shape: no lottery fields leak onto them.
#[tokio::test]
async fn ordinary_rows_are_unchanged_by_v2_lottery_fields() {
    let chain = seeded_chain();
    let resp = handle_get_outputs(
        json!(1),
        &json!({"start_height": 2, "end_height": 2}),
        &chain.state,
    )
    .await;
    let blocks = resp.result.unwrap();
    for row in blocks[0]["outputs"].as_array().unwrap() {
        assert!(row.get("lotteryContext").is_none());
        assert!(row.get("winnerOutpoint").is_none());
        assert!(!row["cryptoOutputIndex"].is_null());
    }
}

fn row_inputs() -> (LotteryOutput, Utxo) {
    let lottery_output = LotteryOutput {
        winner_tx_hash: [0x0a; 32],
        winner_output_index: 4,
        payout: 9,
        target_key: [0x41; 32],
        public_key: [0x42; 32],
        kem_ciphertext: None,
    };
    let utxo = Utxo {
        id: UtxoId::new([0x0b; 32], 1),
        output: lottery_output.to_tx_output(ClusterTagVector::single(ClusterId(5))),
        created_at: 8,
    };
    (lottery_output, utxo)
}

#[test]
fn payout_row_rejects_a_direct_context_and_a_mismatched_utxo() {
    let (lottery_output, utxo) = row_inputs();
    let direct = DerivationContext::Direct {
        height: 8,
        base_index: 1,
    };
    assert!(
        lottery_payout_row(&[0x0b; 32], 0, &lottery_output, Some((&utxo, &direct)))
            .unwrap_err()
            .contains("non-lottery")
    );

    let lottery = DerivationContext::Lottery {
        height: 8,
        ordinal: 0,
        base_index: 4,
        tweak: tweak(),
    };
    let mut other = utxo.clone();
    other.output.target_key = [0x99; 32];
    assert!(
        lottery_payout_row(&[0x0b; 32], 0, &lottery_output, Some((&other, &lottery)))
            .unwrap_err()
            .contains("disagrees")
    );
    // Ordinal must match the payout's position in the block.
    assert!(lottery_payout_row(&[0x0b; 32], 1, &lottery_output, Some((&utxo, &lottery))).is_err());
    assert!(lottery_payout_row(&[0x0b; 32], 0, &lottery_output, Some((&utxo, &lottery))).is_ok());
}

#[tokio::test]
async fn chain_info_and_identity_report_lottery_rules() {
    let chain = seeded_chain();
    let state = chain.state.with_identity(NodeIdentity {
        protocol_version: "7.0.0".into(),
        ..NodeIdentity::default()
    });
    assert_eq!(LOTTERY_RULES_VERSION, 2);
    let info = handle_chain_info(json!(1), &state).await.result.unwrap();
    assert_eq!(info["lotteryRules"], json!(2));
    assert_eq!(info["protocolVersion"], json!("7.0.0"));
    let identity = handle_node_identity(json!(1), &state).await.result.unwrap();
    assert_eq!(identity["lotteryRules"], json!(2));
}
