//! Durable Squads binding/action journal, independent of the generic mint row.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolanaAction {
    pub kind: String,
    pub signature: String,
    pub raw: Vec<u8>,
    pub last_valid_height: u64,
}

#[derive(Clone, Debug)]
pub struct SolanaIntent {
    pub order_id: Uuid,
    pub binding: String,
    pub multisig: String,
    pub index: Option<u64>,
    pub revision: i64,
    pub verified: bool,
    pub action: Option<SolanaAction>,
}

impl Database {
    pub fn solana_intent(&self, order: &Uuid) -> Result<Option<SolanaIntent>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        Self::read_solana_intent(&conn, order)
    }
    pub(super) fn read_solana_intent(
        conn: &Connection,
        order: &Uuid,
    ) -> Result<Option<SolanaIntent>, String> {
        let row = conn.query_row("SELECT binding,multisig,transaction_index,revision,action,verified FROM solana_mint_intents WHERE order_id=?1",
            [order.to_string()], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,i64>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,bool>(5)?)))
            .optional().map_err(|e|e.to_string())?;
        row.map(|(binding, multisig, index, revision, action, verified)| {
            Ok(SolanaIntent {
                order_id: *order,
                binding,
                multisig,
                index: index
                    .map(|n| u64::try_from(n).map_err(|e| e.to_string()))
                    .transpose()?,
                revision,
                verified,
                action: action
                    .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
                    .transpose()?,
            })
        })
        .transpose()
    }
    /// Reserve immutable order semantics before signing or broadcasting
    /// anything.
    pub fn claim_solana_intent(
        &self,
        order: &Uuid,
        binding: &str,
        multisig: &str,
    ) -> Result<SolanaIntent, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let inserted = tx.execute("INSERT OR IGNORE INTO solana_mint_intents(order_id,binding,multisig,revision) VALUES (?1,?2,?3,0)",params![order.to_string(),binding,multisig]).map_err(|e|e.to_string())?;
        if inserted == 1 {
            tx.execute(
                "INSERT INTO solana_budget_origins(order_id,legacy) VALUES(?1,0)",
                [order.to_string()],
            )
            .map_err(|e| e.to_string())?;
        }
        let intent = Self::read_solana_intent(&tx, order)?.ok_or("missing Squads intent")?;
        if intent.binding != binding || intent.multisig != multisig {
            return Err("Squads order binding changed".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(intent)
    }
    /// CAS guards workers sharing this database; the unique index guards
    /// different orders racing the shared multisig counter. A losing writer
    /// never broadcasts.
    pub fn update_solana_intent(
        &self,
        expected: &SolanaIntent,
        index: Option<u64>,
        verified: bool,
        action: Option<&SolanaAction>,
    ) -> Result<bool, String> {
        let index = index
            .map(|i| i64::try_from(i).map_err(|e| e.to_string()))
            .transpose()?;
        let action = action
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| e.to_string())?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let n=conn.execute("UPDATE OR IGNORE solana_mint_intents SET transaction_index=?1,action=?2,verified=?5,revision=revision+1 WHERE order_id=?3 AND revision=?4",params![index,action,expected.order_id.to_string(),expected.revision,verified]).map_err(|e|e.to_string())?;
        Ok(n == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_binding_and_cas_survive_reopen_and_reject_competing_order() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("intents.db");
        let path = path.to_str().unwrap();
        let a = Database::open(path).unwrap();
        a.migrate().unwrap();
        let b = Database::open(path).unwrap();
        let id = Uuid::new_v4();
        let x = a.claim_solana_intent(&id, "payload", "multisig").unwrap();
        let y = b.claim_solana_intent(&id, "payload", "multisig").unwrap();
        let action = SolanaAction {
            kind: "create".into(),
            signature: "signed".into(),
            raw: vec![1, 2],
            last_valid_height: 3,
        };
        assert!(a
            .update_solana_intent(&x, Some(1), false, Some(&action))
            .unwrap());
        assert!(!b.update_solana_intent(&y, Some(2), false, None).unwrap());
        assert!(a.claim_solana_intent(&id, "changed", "multisig").is_err());
        let z = b
            .claim_solana_intent(&Uuid::new_v4(), "other", "multisig")
            .unwrap();
        assert!(!b.update_solana_intent(&z, Some(1), false, None).unwrap());
        drop(a);
        drop(b);
        let c = Database::open(path).unwrap();
        c.migrate().unwrap();
        let saved = c.solana_intent(&id).unwrap().unwrap();
        assert_eq!(saved.index, Some(1));
        assert_eq!(saved.action, Some(action));
    }
    #[test]
    fn claim_in_child_process() {
        let Ok(path) = std::env::var("SQUADS_TEST_RACE_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(path);
        let worker = std::env::var("SQUADS_TEST_RACE_WORKER").unwrap();
        let order = Uuid::parse_str(&std::env::var("SQUADS_TEST_RACE_ORDER").unwrap()).unwrap();
        let db = Database::open(directory.join("race.db").to_str().unwrap()).unwrap();
        let row = db
            .claim_solana_intent(&order, "immutable", "multisig")
            .unwrap();
        std::fs::write(directory.join(format!("ready{worker}")), b"ready").unwrap();
        for _ in 0..500 {
            if directory.join("go").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(directory.join("go").exists());
        let action = SolanaAction {
            kind: "create".into(),
            signature: worker.clone(),
            raw: vec![1],
            last_valid_height: 10,
        };
        let won = db
            .update_solana_intent(&row, Some(1), false, Some(&action))
            .unwrap();
        std::fs::write(
            directory.join(format!("result{worker}")),
            if won { "won" } else { "lost" },
        )
        .unwrap();
    }
    #[test]
    fn independent_processes_have_one_action_winner() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(directory.path().join("race.db").to_str().unwrap()).unwrap();
        db.migrate().unwrap();
        let order = Uuid::new_v4();
        let mut children = vec![];
        for worker in ["a", "b"] {
            children.push(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "db::solana_intents::tests::claim_in_child_process",
                    ])
                    .env("SQUADS_TEST_RACE_DIR", directory.path())
                    .env("SQUADS_TEST_RACE_WORKER", worker)
                    .env("SQUADS_TEST_RACE_ORDER", order.to_string())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            );
        }
        for _ in 0..500 {
            if directory.path().join("readya").exists() && directory.path().join("readyb").exists()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            directory.path().join("readya").exists() && directory.path().join("readyb").exists()
        );
        std::fs::write(directory.path().join("go"), b"go").unwrap();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let wins = ["a", "b"]
            .iter()
            .filter(|w| {
                std::fs::read_to_string(directory.path().join(format!("result{w}"))).unwrap()
                    == "won"
            })
            .count();
        assert_eq!(wins, 1);
        assert_eq!(db.solana_intent(&order).unwrap().unwrap().revision, 1);
    }
    #[test]
    fn rollback_retains_binding_and_completion_cas_does_not_touch_failed_order() {
        use bth_bridge_core::{BridgeOrder, Chain, OrderStatus};
        let db = Database::open(":memory:").unwrap();
        db.migrate().unwrap();
        let mut order =
            BridgeOrder::new_mint(Chain::Solana, 10, 0, "reserve".into(), "recipient".into());
        order.set_status(OrderStatus::MintPending);
        db.insert_order(&order).unwrap();
        db.record_mint_submitted(&order.id, "orderhash", Chain::Solana, "squads:handle")
            .unwrap();
        let row = db
            .claim_solana_intent(&order.id, "payload", "multisig")
            .unwrap();
        db.update_solana_intent(&row, Some(1), true, None).unwrap();
        db.rollback_mint(&order.id).unwrap();
        assert!(db.solana_intent(&order.id).unwrap().unwrap().verified);
        db.record_mint_submitted(&order.id, "orderhash", Chain::Solana, "squads:handle")
            .unwrap();
        db.update_order_status(
            &order.id,
            &OrderStatus::Failed {
                reason: "concurrent transition".into(),
            },
            None,
        )
        .unwrap();
        let row = db.solana_intent(&order.id).unwrap().unwrap();
        let action = SolanaAction {
            kind: "completed".into(),
            signature: "actual-execution".into(),
            raw: vec![],
            last_valid_height: 0,
        };
        db.update_solana_intent(&row, row.index, true, Some(&action))
            .unwrap();
        assert!(db.mark_mint_confirmed(&order.id).is_err());
        let mint = db.get_mint_by_order(&order.id).unwrap().unwrap();
        assert_eq!(mint.dest_tx, "squads:handle");
        assert!(mint.confirmed_at.is_none());
    }
}
