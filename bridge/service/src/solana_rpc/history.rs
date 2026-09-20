//! Strict finalized historical evidence. Unsupported versions remain
//! inconclusive.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistorySignature {
    pub signature: String,
    pub slot: u64,
    pub failed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoricalTransaction {
    pub signature: String,
    pub slot: u64,
    pub instructions: Vec<Instruction>,
    pub inner: Vec<(usize, Vec<Instruction>)>,
    pub logs: Vec<String>,
    pub evidence_hash: String,
}
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing historical {key}"))
}
fn number(v: &Value, key: &str) -> Result<usize, String> {
    usize::try_from(
        v.get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("missing historical {key}"))?,
    )
    .map_err(|e| e.to_string())
}
fn array<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    v.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing historical {key}"))
}
fn signature(value: &str) -> Result<(), String> {
    if bs58::decode(value)
        .into_vec()
        .map_err(|e| e.to_string())?
        .len()
        != 64
    {
        return Err("invalid historical signature".into());
    }
    Ok(())
}
pub fn parse_page(v: &Value) -> Result<Vec<HistorySignature>, String> {
    let rows = v.as_array().ok_or("history page not an array")?;
    rows.iter()
        .map(|r| {
            let s = string(r, "signature")?;
            signature(s)?;
            if string(r, "confirmationStatus")? != "finalized" {
                return Err("history is not finalized".into());
            }
            let err = r.get("err").ok_or("missing history err")?;
            if !(err.is_null() || err.is_object() || err.is_string()) {
                return Err("malformed historical error field".into());
            }
            Ok(HistorySignature {
                signature: s.into(),
                slot: r
                    .get("slot")
                    .and_then(Value::as_u64)
                    .ok_or("missing history slot")?,
                failed: !err.is_null(),
            })
        })
        .collect()
}
pub fn parse_transaction(
    v: &Value,
    requested: &str,
) -> Result<Option<HistoricalTransaction>, String> {
    if v.is_null() {
        return Ok(None);
    }
    signature(requested)?;
    if v.get("version").and_then(Value::as_str) != Some("legacy") {
        return Err("unsupported historical transaction version".into());
    }
    let slot = v
        .get("slot")
        .and_then(Value::as_u64)
        .ok_or("missing transaction slot")?;
    let tx = v.get("transaction").ok_or("missing transaction")?;
    let signatures = array(tx, "signatures")?;
    if signatures.first().and_then(Value::as_str) != Some(requested) {
        return Err("historical signature mismatch".into());
    }
    let m = tx.get("message").ok_or("missing message")?;
    let keys: Vec<Pubkey> = array(m, "accountKeys")?
        .iter()
        .map(|k| Pubkey::from_base58(k.as_str().ok_or("invalid account key")?))
        .collect::<Result<_, String>>()?;
    let h = m.get("header").ok_or("missing header")?;
    let ns = number(h, "numRequiredSignatures")?;
    let rs = number(h, "numReadonlySignedAccounts")?;
    let ru = number(h, "numReadonlyUnsignedAccounts")?;
    if ns == 0 || ns > keys.len() || rs > ns || ru > keys.len() - ns || signatures.len() != ns {
        return Err("invalid history header".into());
    }
    if keys.iter().enumerate().any(|(i, k)| keys[..i].contains(k)) {
        return Err("duplicate historical account key".into());
    }
    for s in signatures {
        signature(s.as_str().ok_or("invalid signature")?)?;
    }
    Pubkey::from_base58(string(m, "recentBlockhash")?)?;
    if m.get("addressTableLookups")
        .is_some_and(|a| a.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err("historical lookup tables unsupported".into());
    }
    let meta = v
        .get("meta")
        .filter(|m| m.is_object())
        .ok_or("missing transaction metadata")?;
    if !meta
        .get("err")
        .ok_or("missing transaction error field")?
        .is_null()
    {
        return Err("historical transaction failed".into());
    }
    if let Some(a) = meta.get("loadedAddresses") {
        if !array(a, "writable")?.is_empty() || !array(a, "readonly")?.is_empty() {
            return Err("historical loaded addresses unsupported".into());
        }
    }
    let decode = |x: &Value| -> Result<Instruction, String> {
        let program = *keys
            .get(number(x, "programIdIndex")?)
            .ok_or("invalid program index")?;
        let accounts = array(x, "accounts")?
            .iter()
            .map(|i| {
                let i = usize::try_from(i.as_u64().ok_or("invalid account index")?)
                    .map_err(|e| e.to_string())?;
                Ok(AccountMeta {
                    pubkey: *keys.get(i).ok_or("account index out of range")?,
                    is_signer: i < ns,
                    is_writable: if i < ns {
                        i < ns - rs
                    } else {
                        i < keys.len() - ru
                    },
                })
            })
            .collect::<Result<_, String>>()?;
        let data = bs58::decode(string(x, "data")?)
            .into_vec()
            .map_err(|e| e.to_string())?;
        Ok(Instruction {
            program_id: program,
            accounts,
            data,
        })
    };
    let instructions = array(m, "instructions")?
        .iter()
        .map(decode)
        .collect::<Result<Vec<_>, _>>()?;
    let inner = array(meta, "innerInstructions")?
        .iter()
        .map(|g| {
            let index = number(g, "index")?;
            if index >= instructions.len() {
                return Err("inner instruction index out of range".into());
            }
            Ok((
                index,
                array(g, "instructions")?
                    .iter()
                    .map(decode)
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    if inner
        .iter()
        .enumerate()
        .any(|(i, g)| inner[..i].iter().any(|p| p.0 == g.0))
    {
        return Err("duplicate inner group".into());
    }
    let logs = array(meta, "logMessages")?
        .iter()
        .map(|s| {
            s.as_str()
                .map(str::to_owned)
                .ok_or("invalid log".to_string())
        })
        .collect::<Result<_, _>>()?;
    use sha2::Digest;
    let evidence_hash = hex::encode(sha2::Sha256::digest(
        serde_json::to_vec(v).map_err(|e| e.to_string())?,
    ));
    Ok(Some(HistoricalTransaction {
        signature: requested.into(),
        slot,
        instructions,
        inner,
        logs,
        evidence_hash,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transaction() -> (String, Value) {
        let sig = bs58::encode([1; 64]).into_string();
        let key = Pubkey([2; 32]).to_base58();
        (
            sig.clone(),
            json!({"version":"legacy","slot":10,"transaction":{"signatures":[sig],"message":{"accountKeys":[key],"header":{"numRequiredSignatures":1,"numReadonlySignedAccounts":0,"numReadonlyUnsignedAccounts":0},"recentBlockhash":key,"instructions":[{"programIdIndex":0,"accounts":[0],"data":"1"}]}},"meta":{"err":null,"innerInstructions":[],"logMessages":[]}}),
        )
    }
    #[test]
    fn strict_rpc_evidence_rejects_missing_failed_and_unsupported_shapes() {
        let (sig, v) = transaction();
        assert!(parse_transaction(&v, &sig).unwrap().is_some());
        assert!(parse_transaction(&Value::Null, &sig).unwrap().is_none());
        for (pointer, value) in [
            ("/meta", Value::Null),
            ("/meta/err", json!({"InstructionError":[0,"Custom"]})),
            ("/version", json!(0)),
            (
                "/transaction/signatures/0",
                json!(bs58::encode([3; 64]).into_string()),
            ),
            (
                "/transaction/message/header/numRequiredSignatures",
                json!(9),
            ),
            (
                "/transaction/message/instructions/0/programIdIndex",
                json!(9),
            ),
            ("/transaction/message/addressTableLookups", json!([{}])),
            ("/meta/logMessages", Value::Null),
            ("/meta/innerInstructions", Value::Null),
        ] {
            let mut bad = v.clone();
            if let Some(target) = bad.pointer_mut(pointer) {
                *target = value;
            } else {
                bad["transaction"]["message"]["addressTableLookups"] = value;
            }
            assert!(parse_transaction(&bad, &sig).is_err(), "{pointer}");
        }
        let mut bad = v;
        bad["meta"].as_object_mut().unwrap().remove("err");
        assert!(parse_transaction(&bad, &sig).is_err());
    }
    #[test]
    fn signature_page_requires_finalized_error_and_slot_fields() {
        let sig = bs58::encode([1; 64]).into_string();
        let page = json!([{"signature":sig,"slot":9,"err":null,"confirmationStatus":"finalized"}]);
        assert_eq!(parse_page(&page).unwrap().len(), 1);
        for field in ["slot", "err", "confirmationStatus"] {
            let mut bad = page.clone();
            bad[0].as_object_mut().unwrap().remove(field);
            assert!(parse_page(&bad).is_err());
        }
        let mut bad = page;
        bad[0]["confirmationStatus"] = json!("confirmed");
        assert!(parse_page(&bad).is_err());
    }
}
