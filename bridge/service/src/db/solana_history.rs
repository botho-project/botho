//! Historical recovery journal. Policy never changes once an order is claimed.
use super::*;
pub const MAX_SOLANA_HISTORY_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct SolanaHistory {
    pub policy: String,
    pub progress: String,
    pub revision: i64,
}
impl Database {
    pub fn solana_history(&self, order: &Uuid) -> Result<Option<SolanaHistory>, String> {
        let c = self.conn.lock().map_err(|e| e.to_string())?;
        c.query_row(
            "SELECT policy,progress,revision FROM solana_history WHERE order_id=?1",
            [order.to_string()],
            |r| {
                Ok(SolanaHistory {
                    policy: r.get(0)?,
                    progress: r.get(1)?,
                    revision: r.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|e| e.to_string())
    }
    pub fn claim_solana_history(
        &self,
        order: &Uuid,
        policy: &str,
        progress: &str,
    ) -> Result<(), String> {
        if policy.len() > 64 * 1024 || progress.len() > MAX_SOLANA_HISTORY_BYTES {
            return Err("historical journal byte capacity exhausted".into());
        }
        let c = self.conn.lock().map_err(|e| e.to_string())?;
        c.execute(
            "INSERT OR IGNORE INTO solana_history(order_id,policy,progress) VALUES (?1,?2,?3)",
            params![order.to_string(), policy, progress],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn update_solana_history(
        &self,
        order: &Uuid,
        expected: &SolanaHistory,
        progress: &str,
    ) -> Result<bool, String> {
        if progress.len() > MAX_SOLANA_HISTORY_BYTES {
            return Err("historical journal byte capacity exhausted".into());
        }
        let c = self.conn.lock().map_err(|e| e.to_string())?;
        Ok(c.execute("UPDATE solana_history SET progress=?1,revision=revision+1 WHERE order_id=?2 AND revision=?3 AND policy=?4",params![progress,order.to_string(),expected.revision,expected.policy]).map_err(|e|e.to_string())?==1)
    }
    /// Historical completion is one atomic transition; no journal-only success
    /// can race rollback and later be reused against a different immutable
    /// order.
    pub fn complete_solana_history(
        &self,
        order: &BridgeOrder,
        expected: &SolanaIntent,
        history: &SolanaHistory,
        index: u64,
        signature: &str,
        evidence: &str,
    ) -> Result<bool, String> {
        if order.order_type != OrderType::Mint
            || order.source_chain != Chain::Bth
            || order.dest_chain != Chain::Solana
            || order
                .source_tx
                .as_deref()
                .is_none_or(|s| s.trim().is_empty())
            || order.source_address.trim().is_empty()
            || order.amount <= order.fee
        {
            return Err("invalid historical mint source route".into());
        }
        let mut c = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let same: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM bridge_orders WHERE id=?1 AND amount=?2 AND fee=?3 AND source_tx IS ?4 AND source_chain=?5 AND dest_chain=?6 AND order_type=?7 AND source_address=?8 AND dest_address=?9 AND status='mint_pending')",params![order.id.to_string(),order.amount as i64,order.fee as i64,order.source_tx,order.source_chain.to_string(),order.dest_chain.to_string(),order.order_type.to_string(),order.source_address,order.dest_address],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !same || expected.order_id != order.id || expected.index.is_some_and(|i| i != index) {
            return Ok(false);
        }
        let index = i64::try_from(index).map_err(|e| e.to_string())?;
        let action = serde_json::to_string(&SolanaAction {
            kind: "completed".into(),
            signature: signature.into(),
            raw: vec![],
            last_valid_height: 0,
        })
        .map_err(|e| e.to_string())?;
        let n=tx.execute("UPDATE OR IGNORE solana_mint_intents SET transaction_index=?1,verified=1,action=?2,revision=revision+1 WHERE order_id=?3 AND revision=?4 AND binding=?5 AND EXISTS(SELECT 1 FROM bridge_orders WHERE id=?3 AND status='mint_pending') AND EXISTS(SELECT 1 FROM solana_history WHERE order_id=?3 AND revision=?6 AND policy=?7)",params![index,action,order.id.to_string(),expected.revision,expected.binding,history.revision,history.policy]).map_err(|e|e.to_string())?;
        if n != 1 {
            return Ok(false);
        }
        let mut progress: serde_json::Value =
            serde_json::from_str(&history.progress).map_err(|e| e.to_string())?;
        if !progress.is_object() {
            return Err("invalid historical progress object".into());
        }
        progress["completed_signature"] = serde_json::json!(signature);
        progress["diagnostic"] = serde_json::json!("historical mint completed");
        let progress = serde_json::to_string(&progress).map_err(|e| e.to_string())?;
        if progress.len() > MAX_SOLANA_HISTORY_BYTES {
            return Err("historical journal byte capacity exhausted".into());
        }
        if tx.execute("UPDATE solana_history SET progress=?1,revision=revision+1 WHERE order_id=?2 AND revision=?3 AND policy=?4",params![progress,order.id.to_string(),history.revision,history.policy]).map_err(|e|e.to_string())?!=1{return Err("historical evidence CAS failed".into());}
        let now = Utc::now().timestamp();
        if tx.execute("UPDATE mints SET dest_tx=?1,confirmed_at=?2 WHERE order_id=?3 AND confirmed_at IS NULL",params![signature,now,order.id.to_string()]).map_err(|e|e.to_string())?!=1{return Err("historical completion missing pending mint".into());}
        if tx.execute("UPDATE bridge_orders SET status='completed',dest_tx=?1,dest_confirmed_at=?2,updated_at=?2 WHERE id=?3 AND status='mint_pending'",params![signature,now,order.id.to_string()]).map_err(|e|e.to_string())?!=1{return Err("historical order CAS failed".into());}
        tx.execute("INSERT INTO audit_log(order_id,action,details,created_at) VALUES (?1,'solana_history_completed',?2,?3)",params![order.id.to_string(),evidence,now]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (Database, BridgeOrder, SolanaIntent, SolanaHistory) {
        let db = Database::open(":memory:").unwrap();
        db.migrate().unwrap();
        let mut order =
            BridgeOrder::new_mint(Chain::Solana, 10, 0, "source".into(), "recipient".into());
        order.source_tx = Some("confirmed-source".into());
        order.set_status(OrderStatus::MintPending);
        db.insert_order(&order).unwrap();
        db.record_mint_submitted(&order.id, "orderhash", Chain::Solana, "squads:handle")
            .unwrap();
        let row = db
            .claim_solana_intent(&order.id, "payload", "multisig")
            .unwrap();
        db.claim_solana_history(&order.id, "policy-v1", r#"{"cursor":0}"#)
            .unwrap();
        let h = db.solana_history(&order.id).unwrap().unwrap();
        (db, order, row, h)
    }
    #[test]
    fn completion_cas_is_atomic_and_policy_is_immutable() {
        let (db, o, row, h) = setup();
        db.claim_solana_history(&o.id, "replacement-policy", "cursor9")
            .unwrap();
        assert_eq!(
            db.solana_history(&o.id).unwrap().unwrap().policy,
            "policy-v1"
        );
        assert!(db
            .update_solana_history(&o.id, &h, r#"{"cursor":1}"#)
            .unwrap());
        assert!(!db
            .complete_solana_history(&o, &row, &h, 1, "execute", "proof")
            .unwrap());
        let h = db.solana_history(&o.id).unwrap().unwrap();
        let mut wrong = o.clone();
        wrong.amount += 1;
        assert!(!db
            .complete_solana_history(&wrong, &row, &h, 1, "execute", "proof")
            .unwrap());
        let mut newest = h.clone();
        newest.progress = r#"{"cursor":"last","receipts":["newest-finalized-receipt"]}"#.into();
        assert!(db
            .complete_solana_history(&o, &row, &newest, 1, "execute", "proof")
            .unwrap());
        let persisted: serde_json::Value =
            serde_json::from_str(&db.solana_history(&o.id).unwrap().unwrap().progress).unwrap();
        assert_eq!(persisted["cursor"], "last");
        assert_eq!(persisted["receipts"][0], "newest-finalized-receipt");
        assert_eq!(persisted["completed_signature"], "execute");
        assert!(!db
            .complete_solana_history(&o, &row, &h, 1, "execute", "proof")
            .unwrap());
        assert_eq!(
            db.get_order(&o.id).unwrap().unwrap().status,
            OrderStatus::Completed
        );
        assert_eq!(
            db.get_mint_by_order(&o.id).unwrap().unwrap().dest_tx,
            "execute"
        );
        let c = db.conn.lock().unwrap();
        let n: i64 = c
            .query_row(
                "SELECT count(*) FROM audit_log WHERE action='solana_history_completed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
    #[test]
    fn rollback_failure_and_missing_mint_cannot_leave_partial_import() {
        for race in ["rollback", "failed", "missing-mint", "bound-index"] {
            let (db, o, row, h) = setup();
            match race {
                "rollback" => db.rollback_mint(&o.id).unwrap(),
                "failed" => db
                    .update_order_status(
                        &o.id,
                        &OrderStatus::Failed {
                            reason: "race".into(),
                        },
                        None,
                    )
                    .unwrap(),
                "missing-mint" => {
                    db.conn
                        .lock()
                        .unwrap()
                        .execute("DELETE FROM mints", [])
                        .unwrap();
                }
                _ => {
                    db.update_solana_intent(&row, Some(2), true, None).unwrap();
                }
            }
            assert!(!db
                .complete_solana_history(&o, &row, &h, 1, "execute", "proof")
                .unwrap_or(false));
            assert!(db.solana_intent(&o.id).unwrap().unwrap().action.is_none());
            if let Some(m) = db.get_mint_by_order(&o.id).unwrap() {
                assert_eq!(m.dest_tx, "squads:handle");
                assert!(m.confirmed_at.is_none());
            }
        }
    }
}
