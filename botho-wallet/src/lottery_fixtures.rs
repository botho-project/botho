//! Test-only V2 lottery payout fixtures (#1642).
//!
//! Payouts are derived with the real `bth_transaction_clsag::lottery_v2`
//! rules (the #1302 primitives the ledger uses), then emitted in the frozen
//! `chain_getOutputs` row shape from #1641: `cryptoOutputIndex` = base index,
//! `lotteryContext {version, baseIndex, tweak, ordinal, height}`,
//! `winnerOutpoint` and `clusterTags`.
use bth_transaction_clsag::{
    lottery_v2::{derive, Context, Domain, Outpoint, Source},
    TxOutput,
};
use serde_json::{json, Value};

/// A derived payout plus the facts needed to render its RPC row.
pub(crate) struct Payout {
    pub output: TxOutput,
    pub context: Context,
    pub winner: Outpoint,
    pub ordinal: u32,
    pub height: u64,
}

/// Derive the V2 payout that `source` (at `winner`, with derivation context
/// `source_context`) receives at `height`. The payout keeps the source's
/// ephemeral key and ML-KEM ciphertext and moves only the target.
pub(crate) fn derive_payout(
    source: &TxOutput,
    winner: Outpoint,
    source_context: Context,
    height: u64,
    ordinal: u32,
    amount: u64,
) -> Payout {
    let derived = derive(
        &Source {
            outpoint: winner.clone(),
            target: source.target_key,
            context: source_context,
        },
        &Domain {
            genesis: [1; 32],
            parent: [2; 32],
            height,
            ordinary_root: [4; 32],
            manifest: [5; 32],
            ordinal,
            amount,
        },
    )
    .expect("fixture payout derives");
    let mut output = source.clone();
    output.amount = amount;
    output.target_key = derived.target;
    Payout {
        output,
        context: derived.context,
        winner,
        ordinal,
        height,
    }
}

/// An ordinary `chain_getOutputs` row.
pub(crate) fn ordinary_row(out: &TxOutput, tx_hash: [u8; 32], index: u32) -> Value {
    json!({
        "txHash": hex::encode(tx_hash), "outputIndex": index, "cryptoOutputIndex": index,
        "targetKey": hex::encode(out.target_key), "publicKey": hex::encode(out.public_key),
        "amountCommitment": hex::encode(out.amount.to_le_bytes()), "clusterTags": [],
        "kemCiphertext": out.kem_ciphertext.as_ref().map(hex::encode),
    })
}

/// A V2 payout row in the frozen #1641 shape. `txHash` is the paying block's
/// hash and `outputIndex` the payout outpoint index; neither is the
/// derivation index.
pub(crate) fn payout_row(payout: &Payout, block_hash: [u8; 32], index: u32) -> Value {
    json!({
        "txHash": hex::encode(block_hash), "outputIndex": index,
        "cryptoOutputIndex": payout.context.base_index,
        "ledgerOutpoint": {"txHash": hex::encode(block_hash), "outputIndex": index},
        "targetKey": hex::encode(payout.output.target_key),
        "publicKey": hex::encode(payout.output.public_key),
        "amountCommitment": hex::encode(payout.output.amount.to_le_bytes()),
        "clusterTags": [[7u64, 1_000_000u64]],
        "kemCiphertext": payout.output.kem_ciphertext.as_ref().map(hex::encode),
        "lottery": true,
        "lotteryContext": {"version": 2, "baseIndex": payout.context.base_index,
            "tweak": hex::encode(payout.context.tweak), "ordinal": payout.ordinal,
            "height": payout.height},
        "winnerOutpoint": format!("{}:{}", hex::encode(payout.winner.hash), payout.winner.index),
    })
}

/// A legacy V1 payout row: the winner's own target and keys, `lottery: true`
/// and no context.
pub(crate) fn legacy_row(
    source: &TxOutput,
    block_hash: [u8; 32],
    index: u32,
    amount: u64,
) -> Value {
    json!({
        "txHash": hex::encode(block_hash), "outputIndex": index,
        "targetKey": hex::encode(source.target_key), "publicKey": hex::encode(source.public_key),
        "amountCommitment": hex::encode(amount.to_le_bytes()), "clusterTags": [],
        "kemCiphertext": source.kem_ciphertext.as_ref().map(hex::encode),
        "lottery": true,
    })
}
