//! Operator-trusted OFFLINE inventory only. Never opens the source with LMDB.
use anyhow::{bail, ensure, Context, Result};
use bincode::Options;
use botho::{
    block::Block,
    transaction::{Utxo, UtxoId},
};
use heed::{types::Bytes, Database, EnvFlags, EnvOpenOptions};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};
const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_RECORD: usize = 8 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;
const MAX_PAYLOAD: usize = 256 * 1024 * 1024;
const MAX_LINEAGE_STEPS: usize = 1_000_000;
type Id = [u8; 36];
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ensure!(bytes.len() <= MAX_RECORD, "record byte limit");
    Ok(bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_RECORD as u64)
        .reject_trailing_bytes()
        .deserialize(bytes)?)
}
fn number(bytes: &[u8]) -> Result<u64> {
    Ok(u64::from_le_bytes(
        bytes.try_into().context("expected u64 bytes")?,
    ))
}
fn envelope(u: &Utxo) -> String {
    let mut h = Sha256::new();
    h.update(b"legacy-inventory-envelope-v1");
    h.update(u.output.target_key);
    h.update(u.output.public_key);
    match &u.output.kem_ciphertext {
        None => h.update([0]),
        Some(k) => {
            h.update([1]);
            h.update((k.len() as u64).to_le_bytes());
            h.update(k);
        }
    }
    hex::encode(h.finalize())
}
fn inventory(source: &Path) -> Result<Value> {
    let input = source.join("data.mdb");
    ensure!(
        fs::symlink_metadata(&input)?.file_type().is_file(),
        "data.mdb must be a regular file, not a symlink"
    );
    let mut file = File::open(&input)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.len() > 0 && metadata.len() <= MAX_FILE,
        "offline file byte limit"
    );
    // Only this owned copy sees LMDB reader locks. A live file copy is NOT a
    // snapshot.
    let temp = tempfile::tempdir()?;
    let mut output = File::create(temp.path().join("data.mdb"))?;
    let mut digest = Sha256::new();
    let mut copied = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        copied += n as u64;
        ensure!(copied <= MAX_FILE, "offline file grew beyond limit");
        digest.update(&buffer[..n]);
        output.write_all(&buffer[..n])?;
    }
    ensure!(
        copied == metadata.len() && file.metadata()?.modified()? == metadata.modified()?,
        "source changed; require immutable offline snapshot"
    );
    output.sync_all()?;
    drop(output);
    // SAFETY: unique owned temporary environment, no concurrent writer, env drops
    // first.
    let env = unsafe {
        EnvOpenOptions::new()
            .max_dbs(8)
            .flags(EnvFlags::READ_ONLY)
            .open(temp.path())?
    };
    let txn = env.read_txn()?;
    let mut tables = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    let mut counts = BTreeMap::new();
    let mut total = 0usize;
    let mut records = 0usize;
    for name in ["blocks", "meta", "utxos", "key_images"] {
        let db: Database<Bytes, Bytes> = env
            .open_database(&txn, Some(name))?
            .with_context(|| format!("missing {name} table"))?;
        let mut values = vec![];
        let mut hash = Sha256::new();
        hash.update(name.as_bytes());
        for item in db.iter(&txn)? {
            let (k, v) = item?;
            records += 1;
            ensure!(records <= MAX_RECORDS, "record count limit");
            ensure!(
                k.len() <= MAX_RECORD && v.len() <= MAX_RECORD,
                "record byte limit"
            );
            total = total
                .checked_add(k.len() + v.len())
                .context("payload overflow")?;
            ensure!(total <= MAX_PAYLOAD, "total payload limit");
            for b in [k, v] {
                hash.update((b.len() as u64).to_le_bytes());
                hash.update(b);
            }
            values.push((k.to_vec(), v.to_vec()));
        }
        hashes.insert(name, hex::encode(hash.finalize()));
        counts.insert(name, values.len());
        tables.insert(name, values);
    }
    let meta: BTreeMap<_, _> = tables["meta"]
        .iter()
        .map(|(k, v)| (k.as_slice(), v.as_slice()))
        .collect();
    let height = number(meta.get(b"height".as_slice()).context("missing height")?)?;
    let tip: [u8; 32] = (*meta
        .get(b"tip_hash".as_slice())
        .context("missing tip hash")?)
    .try_into()
    .context("tip hash length")?;
    let mut utxos = BTreeMap::<Id, Utxo>::new();
    for (k, v) in &tables["utxos"] {
        let id: Id = k.as_slice().try_into().context("utxo key length")?;
        let u: Utxo = decode(v).context("utxo decode")?;
        ensure!(
            u.id.to_bytes() == id && u.created_at <= height,
            "utxo identity/height mismatch"
        );
        utxos.insert(id, u);
    }
    for (k, v) in &tables["key_images"] {
        ensure!(k.len() == 32, "key image length");
        ensure!(number(v)? <= height, "key image height beyond checkpoint");
    }
    let mut blocks = BTreeMap::new();
    for (k, v) in &tables["blocks"] {
        let h = number(k)?;
        let b: Block = decode(v).context("block decode")?;
        ensure!(
            b.height() == h && h <= height,
            "block identity/height mismatch"
        );
        ensure!(b.header.version == 1, "unsupported block version");
        blocks.insert(h, b);
    }
    ensure!(
        blocks
            .get(&height)
            .context("missing checkpoint block")?
            .hash()
            == tip,
        "checkpoint hash mismatch"
    );
    let mut gaps = vec![];
    let mut next = 0;
    let mut previous = None;
    for (&h, b) in &blocks {
        if h > next {
            gaps.push((next, h - 1));
        }
        if let Some((ph, hash)) = previous {
            if ph + 1 == h {
                ensure!(
                    b.header.prev_block_hash == hash,
                    "broken adjacent history link"
                );
            }
        }
        next = h.checked_add(1).context("height overflow")?;
        previous = Some((h, b.hash()));
    }
    let mut expected = BTreeMap::<Id, Utxo>::new();
    let mut edges = BTreeMap::<Id, Id>::new();
    let mut missing = vec![];
    for (&h, b) in &blocks {
        if h > 0 {
            let u = Utxo {
                id: UtxoId::new(b.hash(), 0),
                output: b.minting_tx.to_tx_output(),
                created_at: h,
            };
            ensure!(
                expected.insert(u.id.to_bytes(), u).is_none(),
                "duplicate output provenance"
            );
        }
        ensure!(
            expected.len() + edges.len() <= MAX_RECORDS,
            "derived record limit"
        );
        for tx in &b.transactions {
            for (i, out) in tx.outputs.iter().enumerate() {
                let u = Utxo {
                    id: UtxoId::new(tx.hash(), u32::try_from(i)?),
                    output: out.clone(),
                    created_at: h,
                };
                ensure!(
                    expected.insert(u.id.to_bytes(), u).is_none(),
                    "duplicate output provenance"
                );
            }
        }
        for (i, p) in b.lottery_outputs.iter().enumerate() {
            let id = UtxoId::new(
                b.hash(),
                u32::try_from(i)?.checked_add(1).context("index overflow")?,
            )
            .to_bytes();
            let source = p.winner_utxo_id();
            ensure!(
                edges.insert(id, source).is_none(),
                "duplicate payout provenance"
            );
            if let Some(u) = utxos.get(&id) {
                ensure!(
                    u.created_at == h
                        && u.output.amount == p.payout
                        && u.output.target_key == p.target_key
                        && u.output.public_key == p.public_key
                        && u.output.kem_ciphertext == p.kem_ciphertext
                        && u.output.e_memo.is_none(),
                    "payout record mismatch"
                );
                if let Some(w) = utxos.get(&source) {
                    ensure!(w.created_at < h, "winner not from prior history");
                    ensure!(
                        envelope(u) == envelope(w)
                            && u.output.cluster_tags == w.output.cluster_tags,
                        "winner envelope mismatch"
                    );
                }
            } else {
                missing.push(json!({"id":hex::encode(id),"winner":hex::encode(source),"nominal_value":p.payout.to_string(),"reason":"payout_missing_from_inventory"}));
            }
        }
    }
    ensure!(
        expected.len() + edges.len() <= MAX_RECORDS,
        "derived record limit"
    );
    let missing_ordinary: Vec<_> = expected.iter().filter(|(id,_)| !utxos.contains_key(*id)).map(|(id,u)|json!({"id":hex::encode(id),"nominal_value":u.output.amount.to_string(),"reason":"ordinary_record_missing_from_inventory"})).collect();
    let mut unresolved = vec![];
    let mut unresolved_value = 0u128;
    let mut total_value = 0u128;
    let mut origins = BTreeMap::new();
    let mut lineage_steps = 0usize;
    for (id, u) in &utxos {
        total_value += u.output.amount as u128;
        if let Some(e) = expected.get(id) {
            ensure!(
                u.id == e.id && u.created_at == e.created_at && u.output == e.output,
                "ordinary output record mismatch"
            );
        }
        let mut cursor = *id;
        let mut seen = BTreeSet::new();
        let reason = loop {
            lineage_steps += 1;
            ensure!(
                lineage_steps <= MAX_LINEAGE_STEPS,
                "lineage work limit; no partial report"
            );
            ensure!(seen.insert(cursor), "cyclic payout provenance");
            if let Some(parent) = edges.get(&cursor) {
                if !utxos.contains_key(parent) {
                    break Some("missing_winner_record");
                }
                cursor = *parent;
            } else if expected.contains_key(&cursor) {
                origins.insert(*id, utxos[&cursor].id.output_index);
                break None;
            } else {
                break Some("missing_creation_history");
            }
        };
        if let Some(reason) = reason {
            unresolved_value += u.output.amount as u128;
            unresolved.push(json!({"id":hex::encode(id),"nominal_value":u.output.amount.to_string(),"reason":reason}));
        }
    }
    let alias_envelopes: BTreeSet<_> = edges
        .keys()
        .filter_map(|id| utxos.get(id))
        .map(envelope)
        .collect();
    let mut grouped = BTreeMap::<String, Vec<Id>>::new();
    for (id, u) in &utxos {
        let e = envelope(u);
        if alias_envelopes.contains(&e) {
            grouped.entry(e).or_default().push(*id);
        }
    }
    let groups:Vec<_>=grouped.into_iter().map(|(e,ids)|{let value: u128=ids.iter().map(|id|utxos[id].output.amount as u128).sum();let awards:Vec<_>=ids.iter().filter_map(|id|edges.get(id).map(|winner|json!({"payout":hex::encode(id),"winner":hex::encode(winner),"original_index":origins.get(id)}))).collect();json!({"envelope_hash":e,"records":ids.iter().map(hex::encode).collect::<Vec<_>>(),"nominal_value":value.to_string(),"awards":awards})}).collect();
    Ok(
        json!({"schema":1,"tool_source_sha256":hex::encode(Sha256::digest(include_bytes!("legacy_lottery_inventory.rs"))),"limits":{"file_bytes":MAX_FILE,"record_bytes":MAX_RECORD,"records":MAX_RECORDS,"payload_bytes":MAX_PAYLOAD,"lineage_steps":MAX_LINEAGE_STEPS},"trust":"operator-trusted offline copy; structural inventory only, not consensus revalidation or independently authenticated history; nominal values are NOT spendable balances; key images cannot attribute consumed alias claims","source_data_sha256":hex::encode(digest.finalize()),"table_sha256":hashes,"checkpoint_height":height,"checkpoint_hash":hex::encode(tip),"genesis_hash":blocks.get(&0).map(|b|hex::encode(b.hash())),"missing_height_ranges":gaps,"record_counts":counts,"key_image_observations_unattributed":tables["key_images"].len(),"utxo_nominal_value":total_value.to_string(),"unresolved_utxo_value":unresolved_value.to_string(),"unresolved_utxos":unresolved,"missing_payout_records":missing,"missing_ordinary_records":missing_ordinary,"groups":groups}),
    )
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 || args[0] != "--offline-copy" {
        bail!("usage: legacy_lottery_inventory --offline-copy IMMUTABLE_SNAPSHOT_DIRECTORY (never a live ledger)");
    }
    let report = inventory(Path::new(&args[1]))?;
    serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use botho::{
        block::LotteryOutput,
        transaction::{Transaction, TxOutput},
    };
    use bth_transaction_types::ClusterTagVector;

    // Deliberately unsigned structural records, NOT consensus-accepted blocks.
    fn fixture() -> (Vec<Block>, Vec<Utxo>) {
        let mut blocks = vec![Block::genesis()];
        let mut outputs = vec![];
        let source = TxOutput {
            amount: 100,
            target_key: [7; 32],
            public_key: [8; 32],
            e_memo: None,
            cluster_tags: ClusterTagVector::empty(),
            kem_ciphertext: Some(vec![6; 1088]),
        };
        let mut winner = None;
        let mut first_award = None;
        for height in 1..=4 {
            let mut b = Block::genesis();
            b.header.height = height;
            b.header.prev_block_hash = blocks.last().unwrap().hash();
            b.minting_tx.block_height = height;
            if height == 1 {
                let mut other = source.clone();
                other.target_key = [3; 32];
                let tx = Transaction::new_clsag(vec![], vec![other, source.clone()], 0, 1);
                winner = Some(UtxoId::new(tx.hash(), 1));
                for (index, out) in tx.outputs.iter().enumerate() {
                    outputs.push(Utxo {
                        id: UtxoId::new(tx.hash(), index as u32),
                        output: out.clone(),
                        created_at: height,
                    });
                }
                b.transactions.push(tx);
                b.header.tx_root = Block::compute_tx_root(&b.transactions);
            } else {
                let id = if height == 4 {
                    first_award.unwrap()
                } else {
                    winner.unwrap()
                };
                b.lottery_outputs.push(LotteryOutput {
                    winner_tx_hash: id.tx_hash,
                    winner_output_index: id.output_index,
                    payout: 10,
                    target_key: source.target_key,
                    public_key: source.public_key,
                    kem_ciphertext: source.kem_ciphertext.clone(),
                });
                let payout = UtxoId::new(b.hash(), 1);
                if height == 2 {
                    first_award = Some(payout);
                }
                let mut out = source.clone();
                out.amount = 10;
                outputs.push(Utxo {
                    id: payout,
                    output: out,
                    created_at: height,
                });
            }
            outputs.push(Utxo {
                id: UtxoId::new(b.hash(), 0),
                output: b.minting_tx.to_tx_output(),
                created_at: height,
            });
            blocks.push(b);
        }
        (blocks, outputs)
    }
    fn write_fixture(path: &Path, blocks: &[Block], outputs: &[Utxo], image: bool) {
        let env = unsafe {
            EnvOpenOptions::new()
                .max_dbs(8)
                .map_size(32 * 1024 * 1024)
                .open(path)
                .unwrap()
        };
        let mut txn = env.write_txn().unwrap();
        let db: Database<Bytes, Bytes> = env.create_database(&mut txn, Some("blocks")).unwrap();
        for b in blocks {
            db.put(
                &mut txn,
                &b.height().to_le_bytes(),
                &bincode::serialize(b).unwrap(),
            )
            .unwrap();
        }
        let db: Database<Bytes, Bytes> = env.create_database(&mut txn, Some("utxos")).unwrap();
        for u in outputs {
            db.put(&mut txn, &u.id.to_bytes(), &bincode::serialize(u).unwrap())
                .unwrap();
        }
        let db: Database<Bytes, Bytes> = env.create_database(&mut txn, Some("meta")).unwrap();
        let tip = blocks.last().unwrap();
        db.put(&mut txn, b"height", &tip.height().to_le_bytes())
            .unwrap();
        db.put(&mut txn, b"tip_hash", &tip.hash()).unwrap();
        let db: Database<Bytes, Bytes> = env.create_database(&mut txn, Some("key_images")).unwrap();
        if image {
            db.put(&mut txn, &[9; 32], &3u64.to_le_bytes()).unwrap();
        }
        txn.commit().unwrap();
        env.force_sync().unwrap();
    }
    fn bytes(path: &Path) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(path)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                (
                    p.file_name().unwrap().to_str().unwrap().to_owned(),
                    fs::read(p).unwrap(),
                )
            })
            .collect()
    }
    #[test]
    fn repeated_nested_nonzero_index_deterministic_and_source_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let (b, u) = fixture();
        write_fixture(dir.path(), &b, &u, true);
        let before = bytes(dir.path());
        let r = inventory(dir.path()).unwrap();
        assert_eq!(r, inventory(dir.path()).unwrap());
        assert_eq!(before, bytes(dir.path()));
        assert_eq!(r["missing_height_ranges"], json!([]));
        assert_eq!(r["unresolved_utxo_value"], "0");
        assert_eq!(r["groups"].as_array().unwrap().len(), 1);
        let g = &r["groups"][0];
        assert_eq!(g["records"].as_array().unwrap().len(), 4);
        assert_eq!(g["nominal_value"], "130");
        assert!(g["awards"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["original_index"] == 1));
        assert_eq!(r["key_image_observations_unattributed"], 1);
        // A stored image does not establish which (if any) alias claim was used.
        let no_image = tempfile::tempdir().unwrap();
        write_fixture(no_image.path(), &b, &u, false);
        let clean = inventory(no_image.path()).unwrap();
        assert_eq!(r["groups"], clean["groups"]);
        assert_eq!(r["utxo_nominal_value"], clean["utxo_nominal_value"]);
    }
    #[test]
    fn incomplete_history_and_missing_winner_remain_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, mut u) = fixture();
        b.remove(1);
        write_fixture(dir.path(), &b, &u, false);
        let before = bytes(dir.path());
        let r = inventory(dir.path()).unwrap();
        assert_eq!(before, bytes(dir.path()));
        assert_eq!(r["missing_height_ranges"], json!([[1, 1]]));
        assert!(r["groups"][0]["awards"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["original_index"].is_null()));
        assert!(
            r["unresolved_utxo_value"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                >= 130
        );
        let missing = tempfile::tempdir().unwrap();
        let winner = b[1].lottery_outputs[0].winner_utxo_id();
        u.retain(|u| u.id.to_bytes() != winner);
        write_fixture(missing.path(), &b, &u, false);
        let r = inventory(missing.path()).unwrap();
        assert!(r["unresolved_utxos"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["reason"] == "missing_winner_record"));
    }
    #[test]
    fn missing_payout_is_not_silently_counted_as_an_existing_claim() {
        let dir = tempfile::tempdir().unwrap();
        let (b, mut u) = fixture();
        let absent = UtxoId::new(b[3].hash(), 1);
        u.retain(|u| u.id != absent);
        write_fixture(dir.path(), &b, &u, false);
        let r = inventory(dir.path()).unwrap();
        assert_eq!(r["missing_payout_records"].as_array().unwrap().len(), 1);
        assert_eq!(r["missing_payout_records"][0]["nominal_value"], "10");
        assert_eq!(
            r["missing_payout_records"][0]["winner"],
            hex::encode(b[3].lottery_outputs[0].winner_utxo_id())
        );
    }
    #[test]
    fn corrupt_records_fail_and_source_remains_unchanged() {
        for table in ["utxos", "blocks", "meta", "key_images"] {
            let dir = tempfile::tempdir().unwrap();
            let (b, u) = fixture();
            write_fixture(dir.path(), &b, &u, true);
            let env = unsafe { EnvOpenOptions::new().max_dbs(8).open(dir.path()).unwrap() };
            let mut txn = env.write_txn().unwrap();
            let db: Database<Bytes, Bytes> = env.open_database(&txn, Some(table)).unwrap().unwrap();
            let key = db.iter(&txn).unwrap().next().unwrap().unwrap().0.to_vec();
            db.put(&mut txn, &key, &[255]).unwrap();
            txn.commit().unwrap();
            drop(env);
            let before = bytes(dir.path());
            assert!(inventory(dir.path()).is_err(), "{table}");
            assert_eq!(before, bytes(dir.path()));
        }
    }
    #[test]
    fn mismatched_payout_and_unsupported_version_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (b, mut u) = fixture();
        let payout = UtxoId::new(b[2].hash(), 1);
        u.iter_mut().find(|u| u.id == payout).unwrap().output.amount += 1;
        write_fixture(dir.path(), &b, &u, false);
        assert!(
            format!("{:#}", inventory(dir.path()).unwrap_err()).contains("payout record mismatch")
        );
        let bad = tempfile::tempdir().unwrap();
        let (mut b, u) = fixture();
        b[0].header.version = 2;
        write_fixture(bad.path(), &b, &u, false);
        assert!(format!("{:#}", inventory(bad.path()).unwrap_err())
            .contains("unsupported block version"));
    }
    #[test]
    fn missing_input_never_initializes_and_resource_limits_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        assert!(inventory(dir.path()).is_err());
        assert!(bytes(dir.path()).is_empty());
        let file = File::create(dir.path().join("data.mdb")).unwrap();
        file.set_len(MAX_FILE + 1).unwrap();
        assert!(
            format!("{:#}", inventory(dir.path()).unwrap_err()).contains("offline file byte limit")
        );
        assert_eq!(file.metadata().unwrap().len(), MAX_FILE + 1);
        assert!(decode::<Utxo>(&vec![0; MAX_RECORD + 1]).is_err());
    }
    #[test]
    fn missing_table_is_an_error_without_source_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let env = unsafe { EnvOpenOptions::new().max_dbs(8).open(dir.path()).unwrap() };
        let mut txn = env.write_txn().unwrap();
        let _: Database<Bytes, Bytes> = env.create_database(&mut txn, Some("meta")).unwrap();
        txn.commit().unwrap();
        drop(env);
        let before = bytes(dir.path());
        assert!(
            format!("{:#}", inventory(dir.path()).unwrap_err()).contains("missing blocks table")
        );
        assert_eq!(before, bytes(dir.path()));
    }
    #[cfg(unix)]
    #[test]
    fn symlink_data_file_is_rejected_without_touching_target() {
        let original = tempfile::tempdir().unwrap();
        let alias = tempfile::tempdir().unwrap();
        let (b, u) = fixture();
        write_fixture(original.path(), &b, &u, false);
        std::os::unix::fs::symlink(
            original.path().join("data.mdb"),
            alias.path().join("data.mdb"),
        )
        .unwrap();
        let before = bytes(original.path());
        assert!(format!("{:#}", inventory(alias.path()).unwrap_err()).contains("not a symlink"));
        assert_eq!(before, bytes(original.path()));
        assert_eq!(fs::read_dir(alias.path()).unwrap().count(), 1);
    }
}
