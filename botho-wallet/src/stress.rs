//! Offline automation adapter. Networking, identity checks and input
//! reservations belong to the controller; this module has no RPC or broadcast
//! operation. Requests and responses contain wallet-linked data and must stay
//! protected.
use crate::{
    decoy_selection::MIN_DECOY_AGE_BLOCKS,
    fee_estimation::FeeEstimator,
    keys::WalletKeys,
    ring_builder::{canonical_rpc_history, select_rpc_decoys_from_history},
    rpc_pool::BlockOutputs,
    transaction::{OwnedUtxo, TransactionBuilder, WalletScanner, DUST_THRESHOLD, MIN_TX_FEE},
};
use anyhow::{anyhow, ensure, Context, Result};
use bth_address_codec::{decode_address, Network};
use bth_crypto_ring_signature::KeyImage;
use bth_transaction_clsag::{Transaction, MIN_RING_SIZE};
use bth_util_from_random::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};
use zeroize::Zeroizing;

// A full three-day testnet history can exceed 32 MiB. Keep the offline
// adapter bounded while allowing the controller's independently capped run.
pub const MAX_REQUEST: u64 = 128 * 1024 * 1024;
pub const MAX_TX: usize = 256 * 1024;
pub const MAX_FEE: u64 = 5_000_000_000;

fn default_fee_multiplier() -> u64 {
    1
}

fn deserialize_fee_multiplier<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<u64, D::Error> {
    let multiplier = u64::deserialize(deserializer)?;
    if matches!(multiplier, 1 | 2 | 4) {
        Ok(multiplier)
    } else {
        Err(serde::de::Error::custom(
            "fee_multiplier must be 1, 2, or 4",
        ))
    }
}

fn bounded_fee(estimate: u64, multiplier: u64) -> Result<(u64, u64)> {
    ensure!(
        matches!(multiplier, 1 | 2 | 4),
        "fee_multiplier must be 1, 2, or 4"
    );
    let baseline = estimate.max(MIN_TX_FEE);
    let fee = baseline
        .checked_mul(multiplier)
        .context("fee multiplication overflow")?;
    ensure!(fee <= MAX_FEE, "estimated fee exceeds program bound");
    Ok((baseline, fee))
}

fn read_bounded_input(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut input = Vec::new();
    reader.take(limit + 1).read_to_end(&mut input)?;
    ensure!(input.len() as u64 <= limit, "request exceeds byte limit");
    Ok(input)
}

/// Read one offline request without consuming an unbounded input stream.
pub fn read_request(reader: impl Read) -> Result<Request> {
    Ok(serde_json::from_slice(&read_bounded_input(
        reader,
        MAX_REQUEST,
    )?)?)
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Generate {
        wallet: String,
    },
    Scan {
        wallet: String,
        blocks: Vec<BlockOutputs>,
    },
    RestoreCheck {
        wallet: String,
        blocks: Vec<BlockOutputs>,
        expected_address: String,
    },
    Prepare {
        wallet: String,
        blocks: Vec<BlockOutputs>,
        height: u64,
        selected: Vec<String>,
        spent: Vec<Spent>,
        reserved: Vec<String>,
        recipient: String,
        allowed_recipients: Vec<String>,
        amount: u64,
        base_rate: u64,
        #[serde(
            default = "default_fee_multiplier",
            deserialize_with = "deserialize_fee_multiplier"
        )]
        fee_multiplier: u64,
        artifact: String,
    },
    Inspect {
        artifact: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spent {
    pub key_image: String,
    pub spent: bool,
    pub pending: bool,
    pub spent_height: Option<u64>,
}

#[derive(Serialize)]
pub struct Scanned {
    pub id: String,
    pub key_image: String,
    pub utxo: OwnedUtxo,
}

pub fn read_keys(path: &Path) -> Result<WalletKeys> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.permissions().mode() & 0o077 == 0
            && meta.uid() == unsafe { libc::geteuid() },
        "wallet must be an owner-only regular file"
    );
    let mut phrase = Zeroizing::new(String::new());
    file.take(4096).read_to_string(&mut phrase)?;
    WalletKeys::from_mnemonic(phrase.trim()).map_err(|_| anyhow!("invalid protected wallet phrase"))
}

/// Publish a new protected file, never replace an existing artifact. A crash
/// leaves an explicitly incomplete .partial file, never a seemingly ready file.
pub fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing artifact directory")?;
    let meta = fs::symlink_metadata(parent)?;
    ensure!(
        meta.is_dir()
            && meta.permissions().mode() & 0o077 == 0
            && meta.uid() == unsafe { libc::geteuid() },
        "artifact directory must be owner-only"
    );
    let partial = path.with_extension("partial");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::hard_link(&partial, path)?;
    File::open(parent)?.sync_all()?;
    fs::remove_file(partial)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
thread_local! {
    // Count the outputs handed to real ownership crypto, not elapsed time.
    static OWNERSHIP_OUTPUTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn validate_history(blocks: &[BlockOutputs]) -> Result<()> {
    let mut heights = HashSet::new();
    let mut ids = HashSet::new();
    for block in blocks {
        ensure!(heights.insert(block.height), "duplicate block height");
        for output in &block.outputs {
            ensure!(
                ids.insert((&output.tx_hash, output.output_index)),
                "duplicate output id"
            );
            for key in [&output.tx_hash, &output.target_key, &output.public_key] {
                ensure!(hex::decode(key)?.len() == 32, "invalid output key length");
            }
            ensure!(
                hex::decode(&output.amount_commitment)?.len() == 8,
                "invalid amount"
            );
            if let Some(kem) = &output.kem_ciphertext {
                ensure!(hex::decode(kem)?.len() == 1088, "invalid KEM ciphertext");
            }
        }
    }
    Ok(())
}

pub fn scan(keys: &WalletKeys, blocks: &[BlockOutputs]) -> Result<Vec<Scanned>> {
    validate_history(blocks)?;
    scan_validated(keys, blocks)
}

/// Ownership crypto is separated from structural checks so Prepare can validate
/// the entire request before limiting crypto work to caller-selected outputs.
fn scan_validated(keys: &WalletKeys, blocks: &[BlockOutputs]) -> Result<Vec<Scanned>> {
    #[cfg(test)]
    OWNERSHIP_OUTPUTS.with(|count| {
        count.set(count.get() + blocks.iter().map(|b| b.outputs.len()).sum::<usize>());
    });
    WalletScanner::new(keys)
        .scan_outputs(blocks)
        .into_iter()
        .map(|utxo| {
            let private = utxo
                .recover_spend_key(keys)
                .context("owned output key recovery failed")?;
            Ok(Scanned {
                id: format!("{}:{}", hex::encode(utxo.tx_hash), utxo.output_index),
                key_image: hex::encode(KeyImage::from(&private).as_bytes()),
                utxo,
            })
        })
        .collect()
}

fn selected_ownership(
    keys: &WalletKeys,
    blocks: &[BlockOutputs],
    selected: &[String],
) -> Result<(Vec<Scanned>, HashSet<String>)> {
    validate_history(blocks)?;
    // RPC hex is case-insensitive; WalletScanner emits lowercase output IDs.
    let output_id = |output: &crate::rpc_pool::TxOutput| {
        format!(
            "{}:{}",
            output.tx_hash.to_ascii_lowercase(),
            output.output_index
        )
    };
    let mut ids = HashSet::new();
    for output in blocks.iter().flat_map(|block| &block.outputs) {
        ensure!(ids.insert(output_id(output)), "duplicate output id");
    }
    // Resolve first-target identity BEFORE slicing. An older original outside
    // the selected set must still disqualify a later owned alias.
    let canonical = canonical_rpc_history(blocks)
        .iter()
        .flat_map(|block| &block.outputs)
        .map(output_id)
        .filter(|id| selected.contains(id))
        .collect();
    let selected_blocks: Vec<_> = blocks
        .iter()
        .filter_map(|block| {
            let outputs: Vec<_> = block
                .outputs
                .iter()
                .filter(|output| selected.contains(&output_id(output)))
                .cloned()
                .collect();
            (!outputs.is_empty()).then_some(BlockOutputs {
                height: block.height,
                outputs,
            })
        })
        .collect();
    Ok((scan_validated(keys, &selected_blocks)?, canonical))
}

pub fn inspect(bytes: &[u8]) -> Result<Value> {
    ensure!(bytes.len() <= MAX_TX, "transaction byte budget exceeded");
    let tx: Transaction = bincode::deserialize(bytes)?;
    ensure!(
        bincode::serialize(&tx)? == bytes,
        "noncanonical transaction encoding"
    );
    tx.verify_ring_signatures().map_err(|e| anyhow!(e))?;
    ensure!(!tx.is_settlement(), "settlement is outside this program");
    ensure!(
        (MIN_TX_FEE..=MAX_FEE).contains(&tx.fee),
        "fee outside program bounds"
    );
    ensure!(
        (1..=4).contains(&tx.inputs.len()),
        "unsupported input shape"
    );
    ensure!(
        (1..=2).contains(&tx.outputs.len()),
        "unsupported output shape"
    );
    ensure!(
        tx.outputs
            .iter()
            .all(|o| o.amount >= DUST_THRESHOLD && o.kem_ciphertext.is_some()),
        "unexpected output type"
    );
    Ok(
        json!({"hash":hex::encode(tx.hash()), "bytes":bytes.len(), "fee":tx.fee,
        "height":tx.created_at_height, "input_count":tx.inputs.len(),
        "key_images":tx.key_images().iter().map(hex::encode).collect::<Vec<_>>(),
        "outputs":tx.outputs.iter().enumerate().map(|(i,o)| json!({"index":i,
            "amount":o.amount,"target_key":hex::encode(o.target_key),
            "public_key":hex::encode(o.public_key)})).collect::<Vec<_>>() }),
    )
}

pub fn execute(request: Request) -> Result<Value> {
    match request {
        Request::Generate { wallet } => {
            let keys = WalletKeys::generate()?;
            publish(Path::new(&wallet), keys.mnemonic_phrase().as_bytes())?;
            Ok(json!({"address":keys.public_address_string(Network::Testnet)?}))
        }
        Request::Scan { wallet, blocks } => {
            let keys = read_keys(Path::new(&wallet))?;
            Ok(
                json!({"address":keys.public_address_string(Network::Testnet)?, "owned":scan(&keys,&blocks)?}),
            )
        }
        Request::RestoreCheck {
            wallet,
            blocks,
            expected_address,
        } => {
            let keys = read_keys(Path::new(&wallet))?;
            let address = keys.public_address_string(Network::Testnet)?;
            ensure!(
                address == expected_address,
                "restored wallet address differs"
            );
            Ok(json!({"address":address,"owned":scan(&keys,&blocks)?}))
        }
        Request::Inspect { artifact } => {
            let mut bytes = Vec::new();
            File::open(artifact)?
                .take(MAX_TX as u64 + 1)
                .read_to_end(&mut bytes)?;
            inspect(&bytes)
        }
        Request::Prepare {
            wallet,
            blocks,
            height,
            selected,
            spent,
            reserved,
            recipient,
            allowed_recipients,
            amount,
            base_rate,
            fee_multiplier,
            artifact,
        } => {
            ensure!(amount >= DUST_THRESHOLD, "amount is below dust");
            ensure!(
                (1..=4).contains(&selected.len()),
                "select one to four inputs"
            );
            ensure!(
                selected.iter().collect::<HashSet<_>>().len() == selected.len(),
                "duplicate input"
            );
            ensure!(
                allowed_recipients.contains(&recipient),
                "recipient is outside allowlist"
            );
            let (recipient_key, network) = decode_address(&recipient)?;
            ensure!(
                network == Network::Testnet,
                "only testnet recipients are permitted"
            );
            ensure!(
                blocks.iter().all(|b| b.height <= height),
                "scan exceeds pinned height"
            );
            ensure!(base_rate > 0, "missing network fee rate");
            let keys = read_keys(Path::new(&wallet))?;
            let (owned, canonical) = selected_ownership(&keys, &blocks, &selected)?;
            let mut input_keys = HashSet::new();
            let mut inputs = Vec::new();
            let mut input_metadata = Vec::new();
            for id in &selected {
                let output = owned
                    .iter()
                    .find(|o| &o.id == id)
                    .context("selected input is not owned")?;
                ensure!(!reserved.contains(id), "input already reserved");
                ensure!(canonical.contains(id), "input is not canonical");
                ensure!(
                    input_keys.insert(output.utxo.target_key),
                    "duplicate input target"
                );
                let matches: Vec<_> = spent
                    .iter()
                    .filter(|s| s.key_image == output.key_image)
                    .collect();
                ensure!(
                    matches.len() == 1
                        && !matches[0].spent
                        && !matches[0].pending
                        && matches[0].spent_height.is_none(),
                    "missing, spent or pending key image"
                );
                ensure!(
                    height.saturating_sub(output.utxo.created_at) >= MIN_DECOY_AGE_BLOCKS,
                    "immature input"
                );
                ensure!(
                    !output.utxo.lottery,
                    "Legacy lottery payouts cannot be spent independently yet"
                );
                inputs.push(output.utxo.clone());
                input_metadata.push(
                    json!({"id":id,"key_image":output.key_image,"amount":output.utxo.amount}),
                );
            }
            let total = inputs
                .iter()
                .try_fold(0u64, |sum, u| sum.checked_add(u.amount))
                .context("input sum overflow")?;
            let fee_inputs: Vec<_> = inputs.iter().map(|u| (u.amount, u.tags())).collect();
            let fee_refs: Vec<_> = fee_inputs.iter().map(|(v, t)| (*v, t)).collect();
            let estimate = FeeEstimator::with_base_rate(base_rate)
                .estimate_fee(&fee_refs, 2)
                .total_fee;
            let (baseline_fee, fee) = bounded_fee(estimate, fee_multiplier)?;
            ensure!(
                amount.checked_add(fee).is_some_and(|need| need <= total),
                "insufficient funds"
            );
            let excluded: Vec<_> = inputs.iter().map(|u| u.target_key).collect();
            let mut rings = Vec::new();
            for input in &inputs {
                rings.push(select_rpc_decoys_from_history(
                    &blocks,
                    height,
                    height - input.created_at,
                    &excluded,
                    MIN_RING_SIZE - 1,
                    &mut OsRng,
                )?);
            }
            let builder = TransactionBuilder::new(keys, inputs.clone(), height);
            let result = builder.build_signed_transaction(
                &recipient_key,
                amount,
                fee,
                inputs,
                total,
                rings,
            )?;
            let bytes = bincode::serialize(&result.transaction)?;
            let mut info = inspect(&bytes)?;
            let output_sum = result
                .transaction
                .outputs
                .iter()
                .try_fold(0u64, |sum, o| sum.checked_add(o.amount))
                .context("output sum overflow")?;
            ensure!(
                output_sum.checked_add(result.actual_fee) == Some(total),
                "input/output accounting mismatch"
            );
            ensure!(
                result.transaction.outputs[0].amount == amount,
                "recipient amount differs"
            );
            publish(Path::new(&artifact), &bytes)?;
            info["baseline_fee"] = json!(baseline_fee);
            info["fee_multiplier"] = json!(fee_multiplier);
            info["selected"] = json!(input_metadata);
            info["artifact"] = json!(artifact);
            Ok(info)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fee_tiers_apply_after_floor_and_reject_overflow_or_budget_excess() {
        assert_eq!(bounded_fee(1, 4).unwrap(), (MIN_TX_FEE, 4 * MIN_TX_FEE));
        assert_eq!(
            bounded_fee(2 * MIN_TX_FEE, 2).unwrap(),
            (2 * MIN_TX_FEE, 4 * MIN_TX_FEE)
        );
        assert_eq!(bounded_fee(MAX_FEE / 4, 4).unwrap().1, MAX_FEE);
        assert!(bounded_fee(MAX_FEE / 4 + 1, 4).is_err());
        assert!(bounded_fee(u64::MAX, 4)
            .unwrap_err()
            .to_string()
            .contains("overflow"));
        assert!(bounded_fee(MIN_TX_FEE, 3).is_err());
    }

    fn prepare_fixture(dir: &Path) -> Value {
        prepare_shape_fixture(dir, 1)
    }

    fn prepare_shape_fixture(dir: &Path, input_count: usize) -> Value {
        use bth_transaction_clsag::TxOutput;
        use bth_transaction_types::ClusterTagVector;

        let keys = WalletKeys::from_mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art").unwrap();
        let sink = WalletKeys::from_mnemonic("legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title").unwrap();
        let wallet = dir.join("wallet.mnemonic");
        publish(&wallet, keys.mnemonic_phrase().as_bytes()).unwrap();
        let mut outputs = Vec::new();
        // Selected real inputs plus exactly enough age-matched canonical decoys.
        for index in 0..input_count + MIN_RING_SIZE - 1 {
            let owner = if index < input_count { &keys } else { &sink };
            let out = TxOutput::new_hybrid_to_address(
                20_000_000_000,
                &owner.public_address(),
                index as u32,
                None,
                ClusterTagVector::empty(),
            )
            .unwrap();
            outputs.push(json!({
                "txHash": hex::encode([9u8; 32]), "outputIndex": index,
                "targetKey": hex::encode(out.target_key),
                "publicKey": hex::encode(out.public_key),
                "amountCommitment": hex::encode(out.amount.to_le_bytes()),
                "clusterTags": [], "kemCiphertext": out.kem_ciphertext.map(hex::encode),
            }));
        }
        let blocks = json!([{"height":100,"outputs":outputs}]);
        let owned = scan(
            &keys,
            &serde_json::from_value::<Vec<BlockOutputs>>(blocks.clone()).unwrap(),
        )
        .unwrap();
        assert_eq!(owned.len(), input_count);
        let recipient = sink.public_address_string(Network::Testnet).unwrap();
        json!({"operation":"prepare", "wallet":wallet, "blocks":blocks,
            "height":120, "selected":owned.iter().map(|o| &o.id).collect::<Vec<_>>(),
            "spent":owned.iter().map(|o| json!({"keyImage":o.key_image,"spent":false,"pending":false,"spentHeight":null})).collect::<Vec<_>>(),
            "reserved":[], "recipient":recipient, "allowed_recipients":[recipient],
            "amount":10_000_000_000u64,"base_rate":1,"artifact":dir.join("signed.bin")})
    }

    #[test]
    fn prepare_ownership_work_is_bounded_by_selected_inputs() {
        for inputs in [1, 2, 4] {
            let dir = tempfile::tempdir().unwrap();
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
            let request = prepare_shape_fixture(dir.path(), inputs);
            for history_copies in [0, 64] {
                let mut value = request.clone();
                // Additional well-formed, unselected aliases increase history
                // size without requiring costly fixture key generation.
                let extra = value["blocks"][0]["outputs"][inputs].clone();
                for index in 0..history_copies {
                    let mut output = extra.clone();
                    output["txHash"] = json!(hex::encode([index as u8 + 32; 32]));
                    value["blocks"][0]["outputs"]
                        .as_array_mut()
                        .unwrap()
                        .push(output);
                }
                value["artifact"] = json!(dir.path().join(format!("shape-{history_copies}.bin")));
                OWNERSHIP_OUTPUTS.with(|count| count.set(0));
                execute(serde_json::from_value(value).unwrap()).unwrap();
                assert_eq!(
                    OWNERSHIP_OUTPUTS.with(|count| count.get()),
                    inputs,
                    "ownership work grew with unselected history: {history_copies} copies"
                );
            }
        }
    }

    fn prepare_error(value: Value) -> String {
        execute(serde_json::from_value(value).unwrap())
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn scan_and_restore_still_discover_all_owned_outputs() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let value = prepare_shape_fixture(dir.path(), 4);
        let keys = read_keys(Path::new(value["wallet"].as_str().unwrap())).unwrap();
        let address = keys.public_address_string(Network::Testnet).unwrap();
        let outputs = value["blocks"][0]["outputs"].as_array().unwrap().len();
        let mut responses = Vec::new();
        for operation in ["scan", "restore_check"] {
            let mut request =
                json!({"operation":operation,"wallet":value["wallet"],"blocks":value["blocks"]});
            if operation == "restore_check" {
                request["expected_address"] = json!(address);
            }
            OWNERSHIP_OUTPUTS.with(|count| count.set(0));
            let response = execute(serde_json::from_value(request).unwrap()).unwrap();
            assert_eq!(OWNERSHIP_OUTPUTS.with(|count| count.get()), outputs);
            assert_eq!(response["owned"].as_array().unwrap().len(), 4);
            responses.push(response);
        }
        assert_eq!(responses[0], responses[1]);
    }

    #[test]
    fn prepare_rejects_invalid_unselected_history_before_ownership_crypto() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let request = prepare_fixture(dir.path());
        for (field, invalid, expected) in [
            ("txHash", "aa", "invalid output key length"),
            ("targetKey", "aa", "invalid output key length"),
            ("publicKey", "aa", "invalid output key length"),
            ("amountCommitment", "aa", "invalid amount"),
            ("kemCiphertext", "aa", "invalid KEM ciphertext"),
            ("targetKey", "zz", "Invalid character"),
        ] {
            let mut value = request.clone();
            value["blocks"][0]["outputs"][1][field] = json!(invalid);
            OWNERSHIP_OUTPUTS.with(|count| count.set(0));
            let error = prepare_error(value);
            assert!(error.contains(expected), "{field}: {error}");
            assert_eq!(OWNERSHIP_OUTPUTS.with(|count| count.get()), 0);
        }
        let mut duplicate_id = request.clone();
        let output = duplicate_id["blocks"][0]["outputs"][1].clone();
        duplicate_id["blocks"][0]["outputs"]
            .as_array_mut()
            .unwrap()
            .push(output);
        assert!(prepare_error(duplicate_id).contains("duplicate output id"));
        let mut duplicate_height = request.clone();
        duplicate_height["blocks"]
            .as_array_mut()
            .unwrap()
            .push(json!({"height":100,"outputs":[]}));
        assert!(prepare_error(duplicate_height).contains("duplicate block height"));
        assert!(!dir.path().join("signed.bin").exists());
    }

    #[test]
    fn prepare_rejects_older_target_alias_outside_selected_history() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut value = prepare_fixture(dir.path());
        let mut original = value["blocks"][0]["outputs"][0].clone();
        original["txHash"] = json!(hex::encode([0xabu8; 32]).to_uppercase());
        // An unrelated public key makes the original not owned. Canonicality
        // is target-only, independent of ownership and supplied block order.
        original["publicKey"] = value["blocks"][0]["outputs"][1]["publicKey"].clone();
        value["blocks"]
            .as_array_mut()
            .unwrap()
            .push(json!({"height":1,"outputs":[original]}));
        assert!(prepare_error(value).contains("input is not canonical"));
        assert!(!dir.path().join("signed.bin").exists());
    }

    #[test]
    fn prepare_keeps_selected_ownership_and_spend_guards() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let request = prepare_fixture(dir.path());
        let mut unowned = request.clone();
        unowned["selected"] = json!([format!("{}:1", hex::encode([9u8; 32]))]);
        assert!(prepare_error(unowned).contains("not owned"));
        let mut reserved = request.clone();
        reserved["reserved"] = reserved["selected"].clone();
        assert!(prepare_error(reserved).contains("already reserved"));
        for field in ["spent", "pending"] {
            let mut value = request.clone();
            value["spent"][0][field] = json!(true);
            assert!(prepare_error(value).contains("missing, spent or pending"));
        }
        let mut immature = request.clone();
        immature["height"] = json!(101);
        assert!(prepare_error(immature).contains("immature input"));
        let mut lottery = request.clone();
        lottery["blocks"][0]["outputs"][0]["lottery"] = json!(true);
        assert!(prepare_error(lottery).contains("not canonical"));
        let mut duplicate = request.clone();
        duplicate["selected"]
            .as_array_mut()
            .unwrap()
            .push(request["selected"][0].clone());
        assert!(prepare_error(duplicate).contains("duplicate input"));
        assert!(!dir.path().join("signed.bin").exists());
    }

    #[test]
    fn prepare_normalizes_rpc_hex_ids_and_rejects_case_duplicate_ids() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut value = prepare_fixture(dir.path());
        let tx_hash = hex::encode([0xabu8; 32]);
        value["blocks"][0]["outputs"][0]["txHash"] = json!(tx_hash.to_uppercase());
        value["selected"] = json!([format!("{tx_hash}:0")]);
        let mut duplicate = value.clone();
        let mut output = duplicate["blocks"][0]["outputs"][0].clone();
        output["txHash"] = json!(tx_hash);
        duplicate["blocks"][0]["outputs"]
            .as_array_mut()
            .unwrap()
            .push(output);
        assert!(prepare_error(duplicate).contains("duplicate output id"));
        let prepared = execute(serde_json::from_value(value).unwrap()).unwrap();
        assert_eq!(prepared["selected"][0]["id"], format!("{tx_hash}:0"));
    }

    #[cfg(feature = "pq")]
    #[test]
    fn prepare_selected_shapes_pass_ledger_and_exclude_other_reals_from_every_ring() {
        use botho::{
            ledger::{ChainState, Ledger, UtxoSnapshot},
            transaction::{TxOutput, Utxo, UtxoId},
        };
        use bth_transaction_types::ClusterTagVector;
        for inputs in [1, 2, 4] {
            let dir = tempfile::tempdir().unwrap();
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
            let value = prepare_shape_fixture(dir.path(), inputs);
            let blocks: Vec<BlockOutputs> =
                serde_json::from_value(value["blocks"].clone()).unwrap();
            let utxos: Vec<_> = blocks
                .iter()
                .flat_map(|block| {
                    block.outputs.iter().map(|o| Utxo {
                        id: UtxoId::new(
                            hex::decode(&o.tx_hash).unwrap().try_into().unwrap(),
                            o.output_index,
                        ),
                        output: TxOutput {
                            amount: u64::from_le_bytes(
                                hex::decode(&o.amount_commitment)
                                    .unwrap()
                                    .try_into()
                                    .unwrap(),
                            ),
                            target_key: hex::decode(&o.target_key).unwrap().try_into().unwrap(),
                            public_key: hex::decode(&o.public_key).unwrap().try_into().unwrap(),
                            e_memo: None,
                            cluster_tags: ClusterTagVector::empty(),
                            kem_ciphertext: o
                                .kem_ciphertext
                                .as_ref()
                                .map(|k| hex::decode(k).unwrap()),
                        },
                        created_at: block.height,
                    })
                })
                .collect();
            let reals: HashSet<_> = utxos[..inputs]
                .iter()
                .map(|u| u.output.target_key)
                .collect();
            let ledger = Ledger::open(&dir.path().join("ledger")).unwrap();
            let snapshot = UtxoSnapshot::new(
                120,
                [0; 32],
                ChainState {
                    height: 120,
                    ..Default::default()
                },
                utxos,
                vec![],
                vec![],
            )
            .unwrap();
            ledger.load_from_snapshot(&snapshot, None).unwrap();
            let prepared = execute(serde_json::from_value(value).unwrap()).unwrap();
            let bytes = fs::read(dir.path().join("signed.bin")).unwrap();
            let tx: Transaction = bincode::deserialize(&bytes).unwrap();
            ledger.verify_transaction(&tx).unwrap();
            assert_eq!(tx.inputs.len(), inputs);
            let mut seen = HashSet::new();
            for input in tx.inputs.clsag() {
                let members: Vec<_> = input
                    .ring
                    .iter()
                    .filter(|m| reals.contains(&m.target_key))
                    .collect();
                assert_eq!(
                    members.len(),
                    1,
                    "another selected real leaked into a decoy ring"
                );
                assert!(seen.insert(members[0].target_key));
                assert_eq!(input.ring.len(), MIN_RING_SIZE);
            }
            assert_eq!(seen, reals);
            assert_eq!(
                tx.outputs.iter().map(|o| o.amount).sum::<u64>() + tx.fee,
                inputs as u64 * 20_000_000_000
            );
            assert_eq!(prepared["input_count"], inputs);
        }
    }

    #[test]
    fn prepare_fee_tiers_are_signed_and_dust_is_reported_exactly() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let request = prepare_fixture(dir.path());
        // Missing multiplier retains the legacy tier; explicit tiers affect
        // the serialized transaction, not just response metadata.
        for multiplier in [1u64, 2, 4] {
            let mut value = request.clone();
            if multiplier != 1 {
                value["fee_multiplier"] = json!(multiplier);
            }
            let artifact = dir.path().join(format!("tier-{multiplier}.bin"));
            value["artifact"] = json!(artifact);
            let prepared = execute(serde_json::from_value(value).unwrap()).unwrap();
            assert_eq!(prepared["baseline_fee"], MIN_TX_FEE);
            assert_eq!(prepared["fee_multiplier"], multiplier);
            assert_eq!(prepared["fee"], MIN_TX_FEE * multiplier);
            let inspected = inspect(&fs::read(&artifact).unwrap()).unwrap();
            assert_eq!(inspected["fee"], prepared["fee"]);
            assert_eq!(inspected["hash"], prepared["hash"]);
            let output_sum: u64 = inspected["outputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["amount"].as_u64().unwrap())
                .sum();
            assert_eq!(
                output_sum + inspected["fee"].as_u64().unwrap(),
                20_000_000_000
            );
        }
        let mut increased_base = request.clone();
        increased_base["base_rate"] = json!(100_000);
        increased_base["fee_multiplier"] = json!(2);
        increased_base["artifact"] = json!(dir.path().join("increased-base.bin"));
        let prepared = execute(serde_json::from_value(increased_base).unwrap()).unwrap();
        let baseline = prepared["baseline_fee"].as_u64().unwrap();
        assert!(baseline > MIN_TX_FEE);
        assert_eq!(prepared["fee"], baseline * 2);

        let mut insufficient = request.clone();
        insufficient["amount"] = json!(20_000_000_000u64 - MIN_TX_FEE);
        insufficient["fee_multiplier"] = json!(2);
        assert!(execute(serde_json::from_value(insufficient).unwrap())
            .unwrap_err()
            .to_string()
            .contains("insufficient funds"));
        assert!(!dir.path().join("signed.bin").exists());

        let mut dust = request.clone();
        dust["fee_multiplier"] = json!(2);
        dust["amount"] = json!(20_000_000_000u64 - 2 * MIN_TX_FEE - (DUST_THRESHOLD - 1));
        dust["artifact"] = json!(dir.path().join("dust.bin"));
        let prepared = execute(serde_json::from_value(dust).unwrap()).unwrap();
        assert_eq!(prepared["baseline_fee"], MIN_TX_FEE);
        assert_eq!(prepared["fee_multiplier"], 2);
        assert_eq!(prepared["fee"], 2 * MIN_TX_FEE + DUST_THRESHOLD - 1);
        assert_eq!(prepared["outputs"].as_array().unwrap().len(), 1);

        let mut over_cap_dust = request.clone();
        over_cap_dust["base_rate"] = json!(MAX_FEE * 100_000 / baseline);
        over_cap_dust["amount"] = json!(20_000_000_000u64 - MAX_FEE - 1);
        let error = execute(serde_json::from_value(over_cap_dust).unwrap()).unwrap_err();
        assert!(
            error.to_string().contains("fee outside program bounds"),
            "{error:#}"
        );
        assert!(!dir.path().join("signed.bin").exists());
        assert!(!dir.path().join("signed.partial").exists());

        let mut excessive = request;
        excessive["base_rate"] = json!(u64::MAX);
        excessive["fee_multiplier"] = json!(4);
        assert!(execute(serde_json::from_value(excessive).unwrap())
            .unwrap_err()
            .to_string()
            .contains("program bound"));
        assert!(!dir.path().join("signed.bin").exists());
        assert!(!dir.path().join("signed.partial").exists());
    }

    #[test]
    fn prepare_fee_multiplier_rejects_invalid_json_types_and_values() {
        let mut request = json!({"operation":"prepare","wallet":"unused","blocks":[],
            "height":0,"selected":[],"spent":[],"reserved":[],"recipient":"unused",
            "allowed_recipients":[],"amount":1,"base_rate":1,"artifact":"unused"});
        for multiplier in [json!(1), json!(2), json!(4)] {
            request["fee_multiplier"] = multiplier;
            assert!(serde_json::from_value::<Request>(request.clone()).is_ok());
        }
        for invalid in [
            json!(0),
            json!(3),
            json!(5),
            json!(-1),
            json!(1.0),
            json!(true),
            json!(null),
            json!("2"),
            json!(u64::MAX),
        ] {
            request["fee_multiplier"] = invalid.clone();
            assert!(
                serde_json::from_value::<Request>(request.clone()).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn request_reader_enforces_inclusive_boundary_and_bounded_read() {
        use std::io::Cursor;
        assert_eq!(read_bounded_input(&b"1234567"[..], 8).unwrap().len(), 7);
        assert_eq!(read_bounded_input(&b"12345678"[..], 8).unwrap().len(), 8);
        let mut oversized = Cursor::new(b"123456789more unread bytes");
        assert!(read_bounded_input(&mut oversized, 8)
            .unwrap_err()
            .to_string()
            .contains("request exceeds byte limit"));
        assert_eq!(oversized.position(), 9);
    }

    #[test]
    fn request_reader_accepts_history_sized_payload_above_old_limit() {
        let json = br#"{"operation":"scan","wallet":"unused","blocks":[]}"#;
        let mut input = vec![b' '; 33 * 1024 * 1024];
        input[..json.len()].copy_from_slice(json);
        assert!(matches!(
            read_request(input.as_slice()).unwrap(),
            Request::Scan { .. }
        ));
    }

    #[test]
    fn request_reader_preserves_json_validation_and_io_errors() {
        assert!(read_request(&b"not json"[..]).is_err());
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("reader failed"))
            }
        }
        assert!(read_request(Broken)
            .err()
            .unwrap()
            .to_string()
            .contains("reader failed"));
    }
    #[test]
    fn protected_publication_never_overwrites_or_accepts_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("signed.bin");
        publish(&path, b"first").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(publish(&path, b"second").is_err());
        assert_eq!(fs::read(path).unwrap(), b"first");
        let unfinished = dir.path().join("unfinished.bin");
        fs::write(unfinished.with_extension("partial"), b"truncated").unwrap();
        assert!(publish(&unfinished, b"replacement").is_err());
        assert!(!unfinished.exists());
    }
    #[test]
    fn key_permissions_and_symlinks_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("wallet.mnemonic");
        let keys = WalletKeys::generate().unwrap();
        publish(&path, keys.mnemonic_phrase().as_bytes()).unwrap();
        assert!(read_keys(&path).is_ok());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_keys(&link).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_keys(&path).is_err());
    }
}
