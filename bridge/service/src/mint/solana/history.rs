//! Positive historical completion only. No method in this module signs or
//! sends.
use super::{
    squads_backend::{bound_mint_event, invalid},
    *,
};
use crate::{
    db::SolanaIntent,
    mint::squads::{
        self,
        state::{checked, Reader},
        SquadsMintContext, SQUADS_V4_PROGRAM_ID,
    },
    solana_rpc::history::{HistoricalTransaction, HistorySignature},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Policy {
    version: u8,
    genesis: String,
    binding: String,
    multisig: Pubkey,
    proposer: Pubkey,
    members: Vec<Pubkey>,
    threshold: u32,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Scan {
    before: Option<String>,
    anchor: Option<String>,
    queue: Vec<HistorySignature>,
    done: bool,
    #[serde(default)]
    stop_at: Option<String>,
    #[serde(default)]
    reached_stop: bool,
    #[serde(default)]
    seen: Vec<String>,
    #[serde(default)]
    last_slot: Option<u64>,
}
#[derive(Default, Serialize, Deserialize)]
struct Progress {
    marker: Scan,
    multisig: Scan,
    transactions: Vec<HistoricalTransaction>,
    diagnostic: String,
    #[serde(default)]
    ticks: u64,
    #[serde(default)]
    refresh: bool,
}
fn disc(name: &str) -> [u8; 8] {
    Sha256::digest(format!("global:{name}"))[..8]
        .try_into()
        .unwrap()
}
fn matches(ix: &Instruction, expected: &Instruction) -> bool {
    ix.program_id == expected.program_id
        && ix.data == expected.data
        && ix.accounts.len() == expected.accounts.len()
        && ix.accounts.iter().zip(&expected.accounts).all(|(a, b)| {
            a.pubkey == b.pubkey
                && (!b.is_signer || a.is_signer)
                && (!b.is_writable || a.is_writable)
        })
}
fn all_instructions(t: &HistoricalTransaction) -> impl Iterator<Item = &Instruction> {
    t.instructions
        .iter()
        .chain(t.inner.iter().flat_map(|(_, v)| v.iter()))
}
fn deployment(ix: &Instruction, p: &Policy) -> Result<bool, String> {
    if ix.program_id != SQUADS_V4_PROGRAM_ID || !ix.data.starts_with(&disc("multisig_create_v2")) {
        return Ok(false);
    }
    if ix.accounts.len() != 6
        || ix.accounts[2].pubkey != p.multisig
        || !ix.accounts[3].is_signer
        || squads::derive_multisig_pda(&ix.accounts[3].pubkey).map_err(|e| e.to_string())?
            != p.multisig
    {
        return Ok(false);
    }
    let mut r = Reader::new(&ix.data[8..]);
    if r.byte()? != 0 {
        return Err("historical external config authority unsupported".into());
    }
    if u32::from(r.u16()?) != p.threshold {
        return Err("historical deployment threshold differs from pinned policy".into());
    }
    let n = r.count()?;
    let mut members = Vec::new();
    for _ in 0..n {
        let k = r.key()?;
        if r.byte()? != 7 {
            return Err("historical member permissions unsupported".into());
        }
        members.push(k);
    }
    members.sort();
    if members != p.members || members.windows(2).any(|w| w[0] == w[1]) {
        return Err("historical deployment members differ from pinned policy".into());
    }
    r.u32()?; // timelock is enforced by the real successful execute
    match r.byte()? {
        0 => {}
        1 => {
            r.key()?;
        }
        _ => return Err("invalid deployment collector".into()),
    }
    match r.byte()? {
        0 => {}
        1 => {
            let n = r.u32()? as usize;
            r.take(n)?;
        }
        _ => return Err("invalid deployment memo".into()),
    }
    if !r.is_empty() {
        return Err("trailing historical deployment data".into());
    }
    Ok(true)
}
struct Proof {
    index: u64,
    signature: String,
    evidence: String,
}
fn verify(
    p: &Policy,
    progress: &Progress,
    base: &SquadsMintContext,
    program: Pubkey,
    order_bytes: [u8; 32],
    amount: u64,
) -> Result<Option<Proof>, String> {
    if !progress.marker.done || !progress.multisig.done || progress.refresh {
        return Ok(None);
    }
    let mut origins = vec![];
    for t in &progress.transactions {
        for ix in &t.instructions {
            if deployment(ix, p)? {
                origins.push(t);
            }
        }
    }
    if origins.len() != 1 {
        return Err("historical deployment policy anchor missing/ambiguous".into());
    }
    let origin = origins[0];
    let mut proofs = vec![];
    for execution in &progress.transactions {
        // Current engine emits exactly one top-level execute. Reject compound
        // outer actions rather than attributing another invocation's event.
        if execution.instructions.len() != 1 {
            continue;
        }
        let ex = &execution.instructions[0];
        if ex.program_id != SQUADS_V4_PROGRAM_ID
            || ex.data != disc("vault_transaction_execute")
            || ex.accounts.len() < 4
            || ex.accounts[0].pubkey != p.multisig
        {
            continue;
        }
        if execution.slot < origin.slot {
            return Err("execution predates policy anchor".into());
        }
        let actor = ex.accounts[3].pubkey;
        if !p.members.contains(&actor) {
            continue;
        }
        let mut candidate = None;
        for t in &progress.transactions {
            if t.slot > execution.slot {
                continue;
            }
            for ix in &t.instructions {
                if ix.program_id != SQUADS_V4_PROGRAM_ID
                    || !ix.data.starts_with(&disc("proposal_create"))
                    || ix.data.len() != 17
                {
                    continue;
                }
                let index = u64::from_le_bytes(ix.data[8..16].try_into().unwrap());
                let mut ctx = SquadsMintContext::resolve(
                    p.multisig,
                    base.vault_index,
                    index,
                    p.proposer,
                    base.inner.clone(),
                )
                .map_err(|e| e.to_string())?;
                if ctx.transaction_pda != ex.accounts[2].pubkey
                    || ctx.proposal_pda != ex.accounts[1].pubkey
                    || !matches(ix, &ctx.build_proposal_create())
                {
                    continue;
                }
                let create = ctx.build_vault_transaction_create();
                let creates: Vec<_> = progress
                    .transactions
                    .iter()
                    .filter(|c| {
                        c.slot <= execution.slot
                            && c.instructions.iter().any(|i| matches(i, &create))
                    })
                    .collect();
                if creates.len() != 1 {
                    return Err("historical exact create lineage missing/ambiguous".into());
                }
                ctx.member = actor;
                if !matches(ex, &ctx.build_vault_transaction_execute())
                    || !ex
                        .accounts
                        .iter()
                        .zip(ctx.build_vault_transaction_execute().accounts)
                        .all(|(a, b)| {
                            a.is_signer == b.is_signer
                                && a.is_writable == (b.is_writable || b.pubkey == actor)
                        })
                    || execution.logs.first()
                        != Some(&format!(
                            "Program {} invoke [1]",
                            SQUADS_V4_PROGRAM_ID.to_base58()
                        ))
                    || execution.logs.last()
                        != Some(&format!(
                            "Program {} success",
                            SQUADS_V4_PROGRAM_ID.to_base58()
                        ))
                {
                    continue;
                }
                // The inner mint must belong to this outer execute and carry the
                // exact account order/data. CPI signer privileges are provided by
                // Squads and are not represented in the outer RPC header.
                let inner = execution
                    .inner
                    .iter()
                    .find(|g| g.0 == 0)
                    .ok_or("execute inner instructions unavailable")?;
                let mint: Vec<_> = inner.1.iter().filter(|i| i.program_id == program).collect();
                if mint.len() != 1
                    || mint[0].data != ctx.inner.data
                    || mint[0]
                        .accounts
                        .iter()
                        .map(|a| a.pubkey)
                        .collect::<Vec<_>>()
                        != ctx
                            .inner
                            .accounts
                            .iter()
                            .map(|a| a.pubkey)
                            .collect::<Vec<_>>()
                {
                    continue;
                }
                if !bound_mint_event(
                    &execution.logs,
                    program,
                    ctx.inner.accounts[4].pubkey,
                    amount,
                    order_bytes,
                ) {
                    continue;
                }
                candidate = Some((ctx, creates[0]));
            }
        }
        let Some((ctx, creation)) = candidate else {
            continue;
        };
        let mut votes = std::collections::BTreeSet::new();
        // This first verifier supports the deployment authorization epoch only.
        // Complete history through the anchor proves that no governance mutation
        // or vote revocation has been silently omitted. Same-slot uncertainty
        // is conservative: an unsupported mutation holds even if ordered later.
        for t in &progress.transactions {
            if t.slot < origin.slot || t.slot > execution.slot {
                continue;
            }
            for ix in all_instructions(t).filter(|i| {
                i.program_id == SQUADS_V4_PROGRAM_ID
                    && i.accounts.iter().any(|a| a.pubkey == p.multisig)
            }) {
                let known = [
                    "multisig_create_v2",
                    "vault_transaction_create",
                    "proposal_create",
                    "proposal_approve",
                    "vault_transaction_execute",
                    "vault_transaction_accounts_close",
                    "config_transaction_accounts_close",
                ];
                if !known.iter().any(|n| ix.data.starts_with(&disc(n))) {
                    return Err(
                        "unsupported historical governance/vote-revocation epoch; retain backing"
                            .into(),
                    );
                }
                if ix.data.starts_with(&disc("proposal_approve"))
                    && ix.accounts.len() == 3
                    && ix.accounts[2].pubkey == ctx.proposal_pda
                {
                    if t.slot < creation.slot || t.slot > execution.slot {
                        return Err("vote outside proposal lineage".into());
                    }
                    let member = ix.accounts[1].pubkey;
                    let mut vote_ctx = ctx.clone();
                    vote_ctx.member = member;
                    if p.members.contains(&member)
                        && matches(ix, &vote_ctx.build_proposal_approve())
                    {
                        votes.insert(member);
                    }
                }
            }
        }
        if votes.len() < p.threshold as usize {
            return Err("historical threshold approvals unavailable".into());
        }
        proofs.push(Proof{index:ctx.transaction_index,signature:execution.signature.clone(),evidence:serde_json::json!({"version":1,"genesis":p.genesis,"execute":execution.signature,"deployment":origin.signature,"create":creation.signature,"transaction_index":ctx.transaction_index,"evidence_hashes":progress.transactions.iter().map(|t|(&t.signature,&t.evidence_hash)).collect::<Vec<_>>()}).to_string()});
    }
    if proofs.len() > 1 {
        return Err("ambiguous historical successful execution".into());
    }
    Ok(proofs.pop())
}

impl SolMinter {
    pub(super) async fn pin_history_policy(
        &self,
        order: &BridgeOrder,
        ctx: &SquadsMintContext,
    ) -> Result<(), MintError> {
        if self
            .store()?
            .solana_history(&order.id)
            .map_err(invalid)?
            .is_some()
        {
            return Ok(());
        }
        let (_, proposer, _) = self.identity()?;
        let mut members = self
            .config
            .mint_signers
            .iter()
            .map(|s| {
                let b = hex::decode(s).map_err(invalid)?;
                Ok(Pubkey(
                    b.try_into()
                        .map_err(|_| invalid("invalid historical member"))?,
                ))
            })
            .collect::<Result<Vec<_>, MintError>>()?;
        members.sort();
        let policy = Policy {
            version: 1,
            genesis: self.rpc.genesis_hash().await.map_err(MintError::Rpc)?,
            binding: self.binding(ctx),
            multisig: ctx.multisig,
            proposer,
            members,
            threshold: self.config.mint_threshold,
        };
        self.store()?
            .claim_solana_history(
                &order.id,
                &serde_json::to_string(&policy).map_err(invalid)?,
                &serde_json::to_string(&Progress::default()).map_err(invalid)?,
            )
            .map_err(invalid)
    }
    /// A bounded read-only chain reconciliation tick, also used by the CLI.
    pub async fn reconcile_squads_history(
        &self,
        order: &BridgeOrder,
    ) -> Result<ConfirmationStatus, MintError> {
        let row = self
            .store()?
            .solana_intent(&order.id)
            .map_err(invalid)?
            .ok_or_else(|| invalid("historical recovery requires existing immutable intent"))?;
        let ctx = self.context(order, row.index.unwrap_or(0)).await?;
        self.recover_history(order, &row, &ctx).await
    }
    pub(super) async fn recover_history(
        &self,
        order: &BridgeOrder,
        row: &SolanaIntent,
        ctx: &SquadsMintContext,
    ) -> Result<ConfirmationStatus, MintError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.recover_history_inner(order, row, ctx),
        )
        .await
        .map_err(|_| {
            invalid("historical recovery tick exceeded 15 seconds; durable cursor retained")
        })?
    }
    async fn recover_history_inner(
        &self,
        order: &BridgeOrder,
        row: &SolanaIntent,
        ctx: &SquadsMintContext,
    ) -> Result<ConfirmationStatus, MintError> {
        let db = self.store()?;
        if db
            .get_order(&order.id)
            .map_err(invalid)?
            .is_some_and(|o| o.status == bth_bridge_core::OrderStatus::Completed)
        {
            return Ok(ConfirmationStatus::Confirmed);
        }
        let saved = db
            .solana_history(&order.id)
            .map_err(invalid)?
            .ok_or_else(|| {
                invalid("legacy intent lacks pinned historical policy; explicit migration required")
            })?;
        let policy: Policy = serde_json::from_str(&saved.policy).map_err(invalid)?;
        if policy.version != 1
            || policy.threshold < 2
            || policy.binding != row.binding
            || row.binding != self.binding(ctx)
            || policy.multisig != ctx.multisig
            || policy.genesis != self.rpc.genesis_hash().await.map_err(MintError::Rpc)?
        {
            return Err(invalid("historical policy/network/binding mismatch"));
        }
        let marker = self
            .account(ctx.inner.accounts[1].pubkey, "finalized")
            .await?
            .ok_or_else(|| invalid("finalized order marker absent; history inconclusive"))?;
        if checked(&marker, self.program_id, "OrderMarker").map_err(invalid)?
            != order.order_id_bytes()
        {
            return Err(invalid("historical marker payload mismatch"));
        }
        let mut progress: Progress = serde_json::from_str(&saved.progress).map_err(invalid)?;
        let config = self.config.squads.as_ref().unwrap();
        if !(1..=100).contains(&config.history_page_size) || config.history_capacity == 0 {
            return Err(invalid("invalid historical recovery bounds"));
        }
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.scan_history(
                &mut progress,
                ctx,
                config.history_page_size,
                config.history_capacity,
            ),
        )
        .await
        .unwrap_or_else(|_| {
            Err(invalid(
                "historical scan time budget exhausted; cursor retained",
            ))
        });
        if let Err(e) = outcome {
            progress.diagnostic = e.to_string();
            db.update_solana_history(
                &order.id,
                &saved,
                &serde_json::to_string(&progress).map_err(invalid)?,
            )
            .map_err(invalid)?;
            return Err(e);
        }
        let proof = verify(
            &policy,
            &progress,
            ctx,
            self.program_id,
            order.order_id_bytes(),
            order.net_amount(),
        );
        match proof {
            Ok(Some(proof)) => {
                // Reject contradictions in any still-present accounts. Closed
                // accounts are allowed only after complete historical proof.
                if let Some(a) = self
                    .account(
                        squads::derive_transaction_pda(&ctx.multisig, proof.index)?,
                        "finalized",
                    )
                    .await?
                {
                    let t = squads::state::VaultState::parse(&a).map_err(invalid)?;
                    if t.multisig != policy.multisig
                        || t.index != proof.index
                        || t.creator != policy.proposer
                        || t.vault_index != ctx.vault_index
                        || t.message != ctx.transaction_message()
                    {
                        return Err(invalid("live transaction contradicts historical proof"));
                    }
                }
                if let Some(a) = self
                    .account(
                        squads::derive_proposal_pda(&ctx.multisig, proof.index)?,
                        "finalized",
                    )
                    .await?
                {
                    let p = squads::state::ProposalState::parse(&a).map_err(invalid)?;
                    if p.multisig != policy.multisig || p.index != proof.index || p.status != 5 {
                        return Err(invalid("live proposal contradicts historical proof"));
                    }
                }
                if db
                    .complete_solana_history(
                        order,
                        row,
                        &saved,
                        proof.index,
                        &proof.signature,
                        &proof.evidence,
                    )
                    .map_err(invalid)?
                {
                    return Ok(ConfirmationStatus::Confirmed);
                }
            }
            Ok(None) => {
                progress.diagnostic = "bounded historical scan incomplete; backing retained".into();
                if progress.marker.done && progress.multisig.done {
                    progress.refresh = true;
                }
            }
            Err(e) => {
                if progress.diagnostic != e {
                    tracing::warn!(order_id=%order.id, reason=%e, "Historical Solana recovery remains pending");
                }
                if e == "historical deployment policy anchor missing/ambiguous" {
                    // A provider may expose older history after archive repair.
                    // Retry the retained oldest boundary; an empty page never
                    // freezes a missing deployment anchor as permanent absence.
                    progress.multisig.done = false;
                }
                progress.diagnostic = e;
            }
        }
        db.update_solana_history(
            &order.id,
            &saved,
            &serde_json::to_string(&progress).map_err(invalid)?,
        )
        .map_err(invalid)?;
        Ok(ConfirmationStatus::Pending { confirmations: 0 })
    }
    async fn scan_history(
        &self,
        p: &mut Progress,
        ctx: &SquadsMintContext,
        limit: usize,
        capacity: usize,
    ) -> Result<(), MintError> {
        p.ticks = p.ticks.wrapping_add(1);
        if p.ticks % 16 == 0 {
            if let Some(anchor) = &p.multisig.anchor {
                let newest = self
                    .rpc
                    .history_page(&ctx.multisig.to_base58(), None, None, 1)
                    .await
                    .map_err(MintError::Rpc)?;
                if newest.first().is_some_and(|r| &r.signature != anchor) {
                    p.refresh = true;
                }
            }
        }
        if p.marker.done && p.multisig.done {
            if !p.refresh {
                return Ok(());
            }
            p.marker = Scan {
                stop_at: p.marker.anchor.clone(),
                ..Scan::default()
            };
            p.multisig = Scan {
                stop_at: p.multisig.anchor.clone(),
                ..Scan::default()
            };
            p.refresh = false;
        }
        let address = if p.marker.done {
            ctx.multisig
        } else {
            ctx.inner.accounts[1].pubkey
        };
        let scan = if p.marker.done {
            &mut p.multisig
        } else {
            &mut p.marker
        };
        if scan.queue.is_empty() {
            if scan.reached_stop {
                scan.done = true;
                return Ok(());
            }
            let page = self
                .rpc
                .history_page(&address.to_base58(), scan.before.as_deref(), None, limit)
                .await
                .map_err(MintError::Rpc)?;
            queue_page(scan, page, limit, capacity).map_err(invalid)?;
            if scan.queue.is_empty() {
                return Ok(());
            }
        }
        let item = &scan.queue[0];
        if let Some(t) = p
            .transactions
            .iter()
            .find(|t| t.signature == item.signature)
        {
            if item.failed || t.slot != item.slot {
                return Err(invalid(
                    "historical page contradicts cached successful evidence",
                ));
            }
        }
        if !item.failed && !p.transactions.iter().any(|t| t.signature == item.signature) {
            if p.transactions.len() >= capacity {
                return Err(invalid(
                    "historical evidence capacity exhausted; raise archive bound explicitly",
                ));
            }
            let tx = self
                .rpc
                .historical_transaction(&item.signature)
                .await
                .map_err(MintError::Rpc)?
                .ok_or_else(|| {
                    invalid("historical transaction unavailable; retry retained cursor")
                })?;
            if tx.slot != item.slot {
                return Err(invalid("historical page/transaction slot mismatch"));
            }
            p.transactions.push(tx);
        }
        scan.queue.remove(0);
        Ok(())
    }
}

/// Validate a whole page before advancing the durable cursor. In particular,
/// raising an exhausted capacity later must retry the same page, not skip it.
fn queue_page(
    scan: &mut Scan,
    mut page: Vec<HistorySignature>,
    limit: usize,
    capacity: usize,
) -> Result<(), String> {
    if page.is_empty() {
        if scan.stop_at.is_some() {
            return Err("historical refresh lost its anchor; retention gap".into());
        }
        scan.done = true;
        return Ok(());
    }
    if page.len() > limit
        || page.windows(2).any(|w| w[0].slot < w[1].slot)
        || scan
            .last_slot
            .is_some_and(|s| page.first().is_some_and(|r| r.slot > s))
        || page
            .last()
            .is_some_and(|r| scan.seen.contains(&r.signature))
    {
        return Err("non-advancing historical page".into());
    }
    let mut next = scan.clone();
    if next.anchor.is_none() {
        next.anchor = Some(page[0].signature.clone());
    }
    next.before = page.last().map(|r| r.signature.clone());
    next.last_slot = page.last().map(|r| r.slot);
    if let Some(i) = page
        .iter()
        .position(|r| Some(&r.signature) == next.stop_at.as_ref())
    {
        page.truncate(i);
        next.reached_stop = true;
    }
    page.retain(|r| !next.seen.contains(&r.signature));
    if next.seen.len() + page.len() > capacity {
        return Err("historical signature capacity exhausted".into());
    }
    for r in &page {
        if !next.seen.contains(&r.signature) {
            next.seen.push(r.signature.clone());
        }
    }
    let mut unique = std::collections::HashSet::new();
    page.retain(|r| unique.insert(r.signature.clone()));
    next.queue = page;
    if next.queue.is_empty() {
        next.done = next.reached_stop;
    }
    *scan = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(s: &str, slot: u64) -> HistorySignature {
        HistorySignature {
            signature: s.into(),
            slot,
            failed: false,
        }
    }
    #[test]
    fn paging_preserves_same_slot_order_overlap_and_capacity_retry() {
        let mut scan = Scan::default();
        assert!(queue_page(&mut scan, vec![sig("a", 9), sig("b", 9)], 2, 1).is_err());
        assert!(scan.before.is_none() && scan.queue.is_empty());
        queue_page(&mut scan, vec![sig("a", 9), sig("b", 9)], 2, 4).unwrap();
        scan.queue.clear();
        queue_page(&mut scan, vec![sig("b", 9), sig("c", 8)], 2, 4).unwrap();
        assert_eq!(
            scan.queue
                .iter()
                .map(|s| s.signature.as_str())
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        let saved = serde_json::to_string(&scan).unwrap();
        assert!(queue_page(&mut scan, vec![sig("a", 9), sig("b", 9)], 2, 4).is_err());
        assert_eq!(serde_json::to_string(&scan).unwrap(), saved);
        let mut resumed: Scan = serde_json::from_str(&saved).unwrap();
        assert_eq!(resumed.queue[0].signature, "c");
        resumed.queue.clear();
        queue_page(&mut resumed, vec![], 2, 4).unwrap();
        assert!(resumed.done);
    }
    #[test]
    fn new_tip_refresh_requires_overlap_with_prior_anchor() {
        let mut refresh = Scan {
            stop_at: Some("old-anchor".into()),
            ..Scan::default()
        };
        queue_page(
            &mut refresh,
            vec![sig("new-execution", 11), sig("new-governance", 10)],
            2,
            10,
        )
        .unwrap();
        refresh.queue.clear();
        let before = serde_json::to_string(&refresh).unwrap();
        assert!(queue_page(&mut refresh, vec![], 2, 10).is_err());
        assert_eq!(serde_json::to_string(&refresh).unwrap(), before);
        queue_page(
            &mut refresh,
            vec![sig("old-anchor", 9), sig("older", 8)],
            2,
            10,
        )
        .unwrap();
        assert!(refresh.done && refresh.reached_stop);
        assert_eq!(refresh.anchor.as_deref(), Some("new-execution"));
    }
    fn sample() -> (Policy, Progress, SquadsMintContext, Pubkey, [u8; 32], u64) {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/squads-history-transactions.json"
        ))
        .unwrap();
        let transactions: Vec<HistoricalTransaction> =
            serde_json::from_value(fixture["transactions"].clone()).unwrap();
        let execution = transactions
            .iter()
            .find(|t| t.signature == fixture["execution"].as_str().unwrap())
            .unwrap();
        let mint = &execution.inner[0].1[0];
        let a = &mint.accounts;
        let amount = u64::from_le_bytes(mint.data[8..16].try_into().unwrap());
        let order: [u8; 32] = mint.data[16..48].try_into().unwrap();
        let program = mint.program_id;
        let inner = build_bridge_mint_instruction(
            program,
            a[0].pubkey,
            a[1].pubkey,
            a[2].pubkey,
            a[3].pubkey,
            a[4].pubkey,
            a[5].pubkey,
            amount,
            order,
        );
        let proposer = Pubkey(SigningKey::from_bytes(&[1; 32]).verifying_key().to_bytes());
        let ctx = SquadsMintContext::resolve(
            execution.instructions[0].accounts[0].pubkey,
            0,
            1,
            proposer,
            inner,
        )
        .unwrap();
        let mut members = (1..=3)
            .map(|n| Pubkey(SigningKey::from_bytes(&[n; 32]).verifying_key().to_bytes()))
            .collect::<Vec<_>>();
        members.sort();
        let p = Policy {
            version: 1,
            genesis: fixture["genesis"].as_str().unwrap().into(),
            binding: "test".into(),
            multisig: ctx.multisig,
            proposer,
            members,
            threshold: 2,
        };
        let progress = Progress {
            marker: Scan {
                done: true,
                ..Scan::default()
            },
            multisig: Scan {
                done: true,
                ..Scan::default()
            },
            transactions,
            ..Progress::default()
        };
        (p, progress, ctx, program, order, amount)
    }
    fn execution(p: &mut Progress) -> &mut HistoricalTransaction {
        p.transactions
            .iter_mut()
            .find(|t| {
                t.instructions.len() == 1
                    && t.instructions[0].data == disc("vault_transaction_execute")
            })
            .unwrap()
    }
    #[test]
    fn real_receipt_replay_rejects_payload_and_attribution_mutations() {
        let (p, progress, ctx, program, order, amount) = sample();
        assert!(verify(&p, &progress, &ctx, program, order, amount)
            .unwrap()
            .is_some());
        for mutation in 0..10 {
            let (_, mut progress, mut ctx, _, _, _) = sample();
            match mutation {
                0 => ctx.inner.data[8] ^= 1,
                1 => ctx.inner.accounts[4].pubkey = Pubkey([99; 32]),
                2 => execution(&mut progress).instructions[0].accounts[2].pubkey = Pubkey([99; 32]),
                3 => {
                    let e = execution(&mut progress);
                    e.instructions.push(e.instructions[0].clone());
                }
                4 => execution(&mut progress).inner.clear(),
                5 => execution(&mut progress).inner[0].1[0].data[8] ^= 1,
                6 => {
                    let e = execution(&mut progress);
                    e.logs[0] = format!("Program {} invoke [1]", program.to_base58());
                }
                7 => {
                    let e = execution(&mut progress);
                    let last = e.logs.len() - 1;
                    e.logs[last] = format!(
                        "Program {} failed: caught error",
                        squads::SQUADS_V4_PROGRAM_ID.to_base58()
                    );
                }
                8 => {
                    let e = execution(&mut progress);
                    e.instructions[0].accounts[3].pubkey = Pubkey([99; 32]);
                }
                _ => ctx
                    .inner
                    .accounts
                    .push(AccountMeta::readonly(Pubkey([99; 32]))),
            }
            assert!(
                verify(&p, &progress, &ctx, program, order, amount)
                    .map(|v| v.is_none())
                    .unwrap_or(true),
                "mutation {mutation}"
            );
        }
    }
    #[test]
    fn history_requires_complete_epoch_and_nonrevoked_threshold() {
        let (p, mut progress, ctx, program, order, amount) = sample();
        progress.marker.done = false;
        assert!(verify(&p, &progress, &ctx, program, order, amount)
            .unwrap()
            .is_none());
        progress.marker.done = true;
        for mutation in [
            "config_transaction_execute",
            "multisig_remove_member",
            "proposal_reject",
            "proposal_cancel",
            "proposal_activate",
        ] {
            let (_, mut q, _, _, _, _) = sample();
            let slot = execution(&mut q).slot;
            q.transactions.push(HistoricalTransaction {
                signature: mutation.into(),
                slot: slot - 1,
                instructions: vec![Instruction {
                    program_id: SQUADS_V4_PROGRAM_ID,
                    accounts: vec![
                        AccountMeta::readonly(ctx.multisig),
                        AccountMeta::writable(ctx.proposal_pda),
                    ],
                    data: disc(mutation).to_vec(),
                }],
                inner: vec![],
                logs: vec![],
                evidence_hash: "mutation".into(),
            });
            assert!(
                verify(&p, &q, &ctx, program, order, amount).is_err(),
                "{mutation}"
            );
            q.transactions.last_mut().unwrap().slot = slot;
            assert!(verify(&p, &q, &ctx, program, order, amount).is_err());
            q.transactions.last_mut().unwrap().slot = slot + 1;
            assert!(
                verify(&p, &q, &ctx, program, order, amount)
                    .unwrap()
                    .is_some(),
                "post-execution drift {mutation}"
            );
        }
        let (_, mut q, _, _, _, _) = sample();
        for t in &mut q.transactions {
            t.instructions
                .retain(|i| !i.data.starts_with(&disc("proposal_approve")));
        }
        assert!(verify(&p, &q, &ctx, program, order, amount).is_err());
        let mut wrong = p.clone();
        wrong.threshold = 1;
        assert!(verify(&wrong, &progress, &ctx, program, order, amount).is_err());
        wrong = p;
        wrong.proposer = Pubkey([99; 32]);
        assert!(verify(&wrong, &progress, &ctx, program, order, amount)
            .unwrap()
            .is_none());
    }
}
