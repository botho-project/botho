//! Lifetime signing/fee reservations. Transient intent clearing never deletes
//! these.
use super::*;
use bth_bridge_core::SquadsRetryPolicy;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolanaAttempt {
    pub action: SolanaAction,
    pub quoted_fee: u64,
    pub broadcasts: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SolanaBudget {
    pub version: u32,
    pub policy: SquadsRetryPolicy,
    pub attempts: Vec<SolanaAttempt>,
    pub next_send_at_ms: i64,
    pub legacy_prior_exposure_unknown: bool,
    pub revision: i64,
}
impl SolanaBudget {
    pub fn reserved_fees(&self) -> Result<u64, String> {
        self.attempts.iter().try_fold(0u64, |n, a| {
            n.checked_add(a.quoted_fee).ok_or("fee sum overflow".into())
        })
    }
    fn validate(&self) -> Result<(), String> {
        self.policy.validate()?;
        if self.version != 1 || self.attempts.len() > 128 || self.next_send_at_ms < 0 {
            return Err("invalid Squads budget ledger".into());
        }
        self.reserved_fees()?;
        Ok(())
    }
}
impl Database {
    fn read_solana_budget(
        conn: &Connection,
        order: &Uuid,
        member: &str,
    ) -> Result<Option<SolanaBudget>, String> {
        let row = conn
            .query_row(
                "SELECT revision,payload FROM solana_budgets WHERE order_id=?1 AND member=?2",
                params![order.to_string(), member],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        row.map(|(revision, json)| {
            if json.len() > 1024 * 1024 {
                return Err("Squads budget ledger too large".into());
            }
            let mut b: SolanaBudget = serde_json::from_str(&json).map_err(|e| e.to_string())?;
            b.revision = revision;
            b.validate()?;
            Ok(b)
        })
        .transpose()
    }
    pub fn solana_budget(
        &self,
        order: &Uuid,
        member: &str,
    ) -> Result<Option<SolanaBudget>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        Self::read_solana_budget(&conn, order, member)
    }
    fn write_budget(
        conn: &Connection,
        order: &Uuid,
        member: &str,
        b: &SolanaBudget,
    ) -> Result<(), String> {
        b.validate()?;
        let payload = serde_json::to_string(b).map_err(|e| e.to_string())?;
        if payload.len() > 1024 * 1024 {
            return Err("Squads budget ledger too large".into());
        }
        let n=conn.execute("UPDATE solana_budgets SET payload=?1,revision=revision+1 WHERE order_id=?2 AND member=?3 AND revision=?4",params![payload,order.to_string(),member,b.revision]).map_err(|e|e.to_string())?;
        if n != 1 {
            return Err("Squads budget revision changed".into());
        }
        Ok(())
    }
    fn new_budget(policy: &SquadsRetryPolicy, legacy: bool) -> Result<SolanaBudget, String> {
        policy.validate()?;
        Ok(SolanaBudget {
            version: 1,
            policy: policy.clone(),
            attempts: vec![],
            next_send_at_ms: 0,
            legacy_prior_exposure_unknown: legacy,
            revision: 0,
        })
    }
    /// Advisory pre-signing check; the atomic reservation repeats every limit.
    pub fn solana_attempt_available(
        &self,
        order: &Uuid,
        member: &str,
        fee: u64,
        now: i64,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        if let Some(b) = Self::read_solana_budget(&conn, order, member)? {
            if b.attempts.len() >= b.policy.max_attempts as usize
                || b.reserved_fees()?
                    .checked_add(fee)
                    .ok_or("fee sum overflow")?
                    > b.policy.max_fee_lamports
            {
                return Err("Squads transaction-fee/signature budget exhausted (rent/transfers excluded); read-only recovery remains available".into());
            }
            return Ok(now >= 0 && now >= b.next_send_at_ms);
        }
        let legacy: bool = conn
            .query_row(
                "SELECT legacy FROM solana_budget_origins WHERE order_id=?1",
                [order.to_string()],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if legacy {
            return Err("legacy Squads exposure unknown; explicit bounded import required".into());
        }
        Ok(now >= 0)
    }
    /// Atomically reserve signature, full quoted fee and first broadcast before
    /// publishing bytes. Losing workers cannot broadcast. Fees are never
    /// refunded.
    pub fn reserve_solana_attempt(
        &self,
        row: &SolanaIntent,
        member: &str,
        policy: &SquadsRetryPolicy,
        action: &SolanaAction,
        fee: u64,
        now: i64,
    ) -> Result<bool, String> {
        policy.validate()?;
        if now < 0
            || fee == 0
            || action.raw.is_empty()
            || action.raw.len() > 1232
            || action.kind == "completed"
        {
            return Err("invalid Squads attempt reservation".into());
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM bridge_orders WHERE id=?1 AND status='mint_pending' AND order_type='mint' AND dest_chain='solana') AND EXISTS(SELECT 1 FROM bridge_state WHERE id=1 AND paused=0)",[row.order_id.to_string()],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !active {
            return Ok(false);
        }
        if Self::read_solana_intent(&tx, &row.order_id)?
            .is_none_or(|r| r.revision != row.revision || r.binding != row.binding)
        {
            return Ok(false);
        }
        let mut budget = match Self::read_solana_budget(&tx, &row.order_id, member)? {
            Some(b) => b,
            None => {
                let legacy: bool = tx
                    .query_row(
                        "SELECT legacy FROM solana_budget_origins WHERE order_id=?1",
                        [row.order_id.to_string()],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if legacy {
                    return Err("legacy Squads fee exposure unknown; explicit bounded budget extension required".into());
                }
                let b = Self::new_budget(policy, false)?;
                tx.execute("INSERT INTO solana_budgets(order_id,member,revision,payload) VALUES(?1,?2,0,?3)",params![row.order_id.to_string(),member,serde_json::to_string(&b).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
                b
            }
        };
        if now < budget.next_send_at_ms {
            return Ok(false);
        }
        if budget
            .attempts
            .iter()
            .any(|a| a.action.signature == action.signature)
        {
            return Err("signature already allocated; reconcile original attempt".into());
        }
        if budget.attempts.len() >= budget.policy.max_attempts as usize
            || budget
                .reserved_fees()?
                .checked_add(fee)
                .ok_or("fee reservation overflow")?
                > budget.policy.max_fee_lamports
        {
            return Err("Squads transaction-fee/signature budget exhausted (rent/transfers excluded); backing retained".into());
        }
        budget.next_send_at_ms = now
            .checked_add(i64::from(budget.policy.send_interval_seconds) * 1000)
            .ok_or("retry clock overflow")?;
        budget.attempts.push(SolanaAttempt {
            action: action.clone(),
            quoted_fee: fee,
            broadcasts: 1,
        });
        let n=tx.execute("UPDATE solana_mint_intents SET action=?1,revision=revision+1 WHERE order_id=?2 AND revision=?3",params![serde_json::to_string(action).map_err(|e|e.to_string())?,row.order_id.to_string(),row.revision]).map_err(|e|e.to_string())?;
        if n != 1 {
            return Ok(false);
        }
        Self::write_budget(&tx, &row.order_id, member, &budget)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }
    /// Same bytes have no new fee allocation, but each broadcast gets a durable
    /// count/time reservation. Clock rollback delays sends; it never resets
    /// state.
    pub fn reserve_solana_rebroadcast(
        &self,
        row: &SolanaIntent,
        member: &str,
        action: &SolanaAction,
        now: i64,
    ) -> Result<bool, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let active:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM bridge_orders WHERE id=?1 AND status='mint_pending' AND order_type='mint' AND dest_chain='solana') AND EXISTS(SELECT 1 FROM bridge_state WHERE id=1 AND paused=0)",[row.order_id.to_string()],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !active {
            return Ok(false);
        }
        if Self::read_solana_intent(&tx, &row.order_id)?
            .is_none_or(|r| r.revision != row.revision || r.action.as_ref() != Some(action))
        {
            return Ok(false);
        }
        let mut b = Self::read_solana_budget(&tx, &row.order_id, member)?
            .ok_or("legacy/unallocated Squads action; read-only reconciliation required")?;
        if now < 0 || now < b.next_send_at_ms {
            return Ok(false);
        }
        let attempt = b
            .attempts
            .iter_mut()
            .find(|a| &a.action == action)
            .ok_or("action does not match fee reservation")?;
        if attempt.broadcasts >= b.policy.max_broadcasts_per_signature {
            return Err("Squads signature broadcast allowance exhausted; read-only reconciliation continues".into());
        }
        attempt.broadcasts = attempt
            .broadcasts
            .checked_add(1)
            .ok_or("broadcast overflow")?;
        b.next_send_at_ms = now
            .checked_add(i64::from(b.policy.send_interval_seconds) * 1000)
            .ok_or("retry clock overflow")?;
        Self::write_budget(&tx, &row.order_id, member, &b)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }
    /// Operator-only finite extension. No network access, consumed totals
    /// unchanged. Revision -1 denotes an explicitly acknowledged legacy
    /// ledger import.
    pub fn extend_solana_budget(
        &self,
        order: &Uuid,
        member: &str,
        revision: i64,
        attempts: u32,
        fees: u64,
        reason: &str,
    ) -> Result<(), String> {
        if attempts == 0 || fees == 0 || reason.trim().is_empty() || reason.len() > 1024 {
            return Err("extension needs positive finite allowances and a reason".into());
        }
        crate::solana_rpc::Pubkey::from_base58(member)?;
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let intent = Self::read_solana_intent(&tx, order)?.ok_or("unknown Squads intent")?;
        if intent
            .action
            .as_ref()
            .is_some_and(|a| a.kind == "completed")
        {
            return Err("completed intent cannot extend budget".into());
        }
        let b = match Self::read_solana_budget(&tx, order, member)? {
            Some(mut b) => {
                if b.revision != revision {
                    return Err("Squads budget revision changed".into());
                }
                b.policy.max_attempts = b
                    .policy
                    .max_attempts
                    .checked_add(attempts)
                    .ok_or("attempt extension overflow")?;
                b.policy.max_fee_lamports = b
                    .policy
                    .max_fee_lamports
                    .checked_add(fees)
                    .ok_or("fee extension overflow")?;
                b
            }
            None => {
                let legacy: bool = tx
                    .query_row(
                        "SELECT legacy FROM solana_budget_origins WHERE order_id=?1",
                        [order.to_string()],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if revision != -1 || !legacy {
                    return Err("missing ledger is not an acknowledged legacy import".into());
                }
                let b = Self::new_budget(
                    &SquadsRetryPolicy {
                        max_attempts: attempts,
                        max_fee_lamports: fees,
                        ..Default::default()
                    },
                    true,
                )?;
                tx.execute("INSERT INTO solana_budgets(order_id,member,revision,payload) VALUES(?1,?2,0,?3)",params![order.to_string(),member,serde_json::to_string(&b).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
                b
            }
        };
        b.validate()?;
        Self::write_budget(&tx, order, member, &b)?;
        let details = serde_json::json!({"member":member,"expected_revision":revision,"added_attempts":attempts,"added_transaction_fee_lamports":fees,"reason":reason,"legacy_prior_exposure_unknown":b.legacy_prior_exposure_unknown});
        tx.execute("INSERT INTO audit_log(order_id,action,details,created_at) VALUES(?1,'solana_budget_extension',?2,?3)",params![order.to_string(),details.to_string(),chrono::Utc::now().timestamp()]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn action(n: u8) -> SolanaAction {
        SolanaAction {
            kind: "execute".into(),
            signature: format!("sig{n}"),
            raw: vec![n],
            last_valid_height: 10,
        }
    }
    fn setup() -> (Database, Uuid) {
        let db = Database::open(":memory:").unwrap();
        db.migrate().unwrap();
        let mut order = bth_bridge_core::BridgeOrder::new_mint(
            bth_bridge_core::Chain::Solana,
            10,
            0,
            "source".into(),
            "recipient".into(),
        );
        order.set_status(bth_bridge_core::OrderStatus::MintPending);
        let id = order.id;
        db.insert_order(&order).unwrap();
        db.claim_solana_intent(&id, "immutable", "multisig")
            .unwrap();
        (db, id)
    }
    #[test]
    fn lifetime_budget_cas_backoff_and_member_isolation() {
        let (db, id) = setup();
        let policy = SquadsRetryPolicy {
            max_attempts: 2,
            max_fee_lamports: 10_000,
            ..Default::default()
        };
        let row = db.solana_intent(&id).unwrap().unwrap();
        assert!(db
            .reserve_solana_attempt(&row, "one", &policy, &action(1), 5000, 100000)
            .unwrap());
        assert!(!db
            .reserve_solana_attempt(&row, "one", &policy, &action(2), 5000, 100000)
            .unwrap());
        let row = db.solana_intent(&id).unwrap().unwrap();
        assert!(!db
            .reserve_solana_rebroadcast(&row, "one", &action(1), 99000)
            .unwrap());
        assert!(db
            .reserve_solana_rebroadcast(&row, "one", &action(1), 110000)
            .unwrap());
        assert!(!db
            .reserve_solana_rebroadcast(&row, "one", &action(1), 110000)
            .unwrap());
        for now in [120000, 130000, 140000, 150000] {
            assert!(db
                .reserve_solana_rebroadcast(&row, "one", &action(1), now)
                .unwrap());
        }
        assert!(db
            .reserve_solana_rebroadcast(&row, "one", &action(1), 160000)
            .is_err());
        assert!(db.update_solana_intent(&row, None, false, None).unwrap());
        let row = db.solana_intent(&id).unwrap().unwrap();
        assert!(db
            .reserve_solana_attempt(&row, "one", &policy, &action(2), 5000, 160000)
            .unwrap());
        let row = db.solana_intent(&id).unwrap().unwrap();
        assert!(db
            .reserve_solana_attempt(
                &row,
                "one",
                &SquadsRetryPolicy::default(),
                &action(3),
                5000,
                170000
            )
            .is_err());
        assert_eq!(
            db.solana_budget(&id, "one")
                .unwrap()
                .unwrap()
                .reserved_fees()
                .unwrap(),
            10_000
        );
        assert!(db
            .reserve_solana_attempt(&row, "two", &policy, &action(3), 5000, 170000)
            .unwrap());
        assert_eq!(
            db.solana_budget(&id, "two")
                .unwrap()
                .unwrap()
                .attempts
                .len(),
            1
        );
    }
    #[test]
    fn budget_survives_restart_rollback_and_extension_is_cas_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("budget.db");
        let path = path.to_str().unwrap();
        let db = Database::open(path).unwrap();
        db.migrate().unwrap();
        let mut order = bth_bridge_core::BridgeOrder::new_mint(
            bth_bridge_core::Chain::Solana,
            10,
            0,
            "reserve".into(),
            "recipient".into(),
        );
        order.set_status(bth_bridge_core::OrderStatus::MintPending);
        db.insert_order(&order).unwrap();
        let id = order.id;
        let member = crate::solana_rpc::Pubkey([1; 32]).to_base58();
        let row = db
            .claim_solana_intent(&id, "immutable", "multisig")
            .unwrap();
        db.reserve_solana_attempt(
            &row,
            &member,
            &SquadsRetryPolicy::default(),
            &action(1),
            5000,
            100000,
        )
        .unwrap();
        db.rollback_mint(&id).unwrap();
        let stale = db.solana_intent(&id).unwrap().unwrap();
        assert!(!db
            .reserve_solana_attempt(
                &stale,
                &member,
                &SquadsRetryPolicy::default(),
                &action(2),
                5000,
                120000
            )
            .unwrap());
        assert!(!db
            .reserve_solana_rebroadcast(&stale, &member, &action(1), 120000)
            .unwrap());
        drop(db);
        let db = Database::open(path).unwrap();
        db.migrate().unwrap();
        let before = db.solana_budget(&id, &member).unwrap().unwrap();
        assert_eq!(before.attempts.len(), 1);
        assert!(db
            .extend_solana_budget(&id, &member, before.revision + 1, 1, 5000, "reviewed")
            .is_err());
        assert!(db
            .extend_solana_budget(&id, &member, before.revision, u32::MAX, 5000, "overflow")
            .is_err());
        db.extend_solana_budget(
            &id,
            &member,
            before.revision,
            1,
            5000,
            "reviewed bounded retry",
        )
        .unwrap();
        let after = db.solana_budget(&id, &member).unwrap().unwrap();
        assert_eq!(after.reserved_fees().unwrap(), 5000);
        assert_eq!(after.attempts.len(), 1);
        assert_eq!(after.policy.max_attempts, 13);
        let details:String=db.conn.lock().unwrap().query_row("SELECT details FROM audit_log WHERE order_id=?1 AND action='solana_budget_extension'",[id.to_string()],|r|r.get(0)).unwrap();
        let details: serde_json::Value = serde_json::from_str(&details).unwrap();
        assert_eq!(details["member"], member);
        assert_eq!(details["expected_revision"], before.revision);
        assert_eq!(details["added_attempts"], 1);
        assert_eq!(details["added_transaction_fee_lamports"], 5000);
        assert_eq!(details["reason"], "reviewed bounded retry");
    }
    #[test]
    fn legacy_intent_requires_explicit_import_and_old_bytes_cannot_rebroadcast() {
        let (db, id) = setup();
        db.conn
            .lock()
            .unwrap()
            .execute("UPDATE solana_budget_origins SET legacy=1", [])
            .unwrap();
        let member = crate::solana_rpc::Pubkey([1; 32]).to_base58();
        let row = db.solana_intent(&id).unwrap().unwrap();
        assert!(db
            .reserve_solana_attempt(
                &row,
                &member,
                &SquadsRetryPolicy::default(),
                &action(1),
                5000,
                100000
            )
            .is_err());
        db.extend_solana_budget(
            &id,
            &member,
            -1,
            2,
            10_000,
            "legacy allowance from migration forward only",
        )
        .unwrap();
        assert!(
            db.solana_budget(&id, &member)
                .unwrap()
                .unwrap()
                .legacy_prior_exposure_unknown
        );
        assert!(db
            .reserve_solana_attempt(
                &row,
                &member,
                &SquadsRetryPolicy::default(),
                &action(2),
                5000,
                100000
            )
            .unwrap());
        let row = db.solana_intent(&id).unwrap().unwrap();
        assert!(!db
            .reserve_solana_rebroadcast(&row, &member, &action(1), 120000)
            .unwrap());
    }
    #[test]
    fn independent_connections_reserve_one_signature_and_one_fee() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("race.db");
        let db = Database::open(path.to_str().unwrap()).unwrap();
        db.migrate().unwrap();
        let mut order = bth_bridge_core::BridgeOrder::new_mint(
            bth_bridge_core::Chain::Solana,
            10,
            0,
            "reserve".into(),
            "recipient".into(),
        );
        order.set_status(bth_bridge_core::OrderStatus::MintPending);
        db.insert_order(&order).unwrap();
        let row = db
            .claim_solana_intent(&order.id, "immutable", "multisig")
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (1..=2)
            .map(|n| {
                let path = path.clone();
                let barrier = barrier.clone();
                let row = row.clone();
                std::thread::spawn(move || {
                    let db = Database::open(path.to_str().unwrap()).unwrap();
                    barrier.wait();
                    db.reserve_solana_attempt(
                        &row,
                        "member",
                        &SquadsRetryPolicy::default(),
                        &action(n),
                        5000,
                        100_000,
                    )
                    .unwrap()
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .map(|h| usize::from(h.join().unwrap()))
                .sum::<usize>(),
            1
        );
        let b = db.solana_budget(&order.id, "member").unwrap().unwrap();
        assert_eq!(b.attempts.len(), 1);
        assert_eq!(b.reserved_fees().unwrap(), 5000);
        let row = db.solana_intent(&order.id).unwrap().unwrap();
        db.update_solana_intent(&row, Some(9), false, None).unwrap();
        let b = db.solana_budget(&order.id, "member").unwrap().unwrap();
        assert_eq!(b.attempts.len(), 1);
        assert_eq!(b.reserved_fees().unwrap(), 5000);
    }
    #[test]
    fn policy_hard_caps_and_exact_millisecond_boundary() {
        for p in [
            SquadsRetryPolicy {
                max_attempts: 129,
                ..Default::default()
            },
            SquadsRetryPolicy {
                max_fee_lamports: 10_000_001,
                ..Default::default()
            },
            SquadsRetryPolicy {
                send_interval_seconds: 0,
                ..Default::default()
            },
            SquadsRetryPolicy {
                max_broadcasts_per_signature: 33,
                ..Default::default()
            },
        ] {
            assert!(p.validate().is_err());
        }
        let (db, id) = setup();
        let row = db.solana_intent(&id).unwrap().unwrap();
        db.reserve_solana_attempt(
            &row,
            "member",
            &SquadsRetryPolicy::default(),
            &action(1),
            5000,
            100_999,
        )
        .unwrap();
        let row = db.solana_intent(&id).unwrap().unwrap();
        assert!(!db
            .reserve_solana_rebroadcast(&row, "member", &action(1), 110_998)
            .unwrap());
        assert!(db
            .reserve_solana_rebroadcast(&row, "member", &action(1), 110_999)
            .unwrap());
    }
}
