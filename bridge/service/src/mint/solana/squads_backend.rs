//! State-advancing Squads custody. An intent handle is not a mint signature:
//! only a bound, successful wbth event can complete it.
use super::*;
use crate::{
    db::{Database, SolanaAction, SolanaIntent},
    mint::squads::{
        self,
        state::{checked, discriminator, MultisigState, ProposalState, VaultState},
        SquadsMintContext, SQUADS_V4_PROGRAM_ID,
    },
    solana_rpc::SolanaAccount,
};

fn invalid(e: impl ToString) -> MintError {
    MintError::Config(e.to_string())
}
fn next_index(index: u64) -> Result<u64, MintError> {
    index
        .checked_add(1)
        .ok_or_else(|| invalid("Squads index exhausted"))
}
fn unlock_at(timestamp: i64, time_lock: u32) -> Result<i64, MintError> {
    timestamp
        .checked_add(i64::from(time_lock))
        .ok_or_else(|| invalid("Squads timelock timestamp overflow"))
}
fn pending() -> ConfirmationStatus {
    ConfirmationStatus::Pending { confirmations: 0 }
}

impl SolMinter {
    fn store(&self) -> Result<&Database, MintError> {
        self.store
            .as_ref()
            .ok_or_else(|| invalid("Squads minting requires a durable database"))
    }
    fn identity(&self) -> Result<(Pubkey, Pubkey, Pubkey), MintError> {
        let c = self
            .config
            .squads
            .as_ref()
            .ok_or_else(|| invalid("Squads configuration absent"))?;
        let multisig = Pubkey::from_base58(&c.multisig).map_err(invalid)?;
        let proposer = Pubkey::from_base58(&c.proposer).map_err(invalid)?;
        let member = self
            .signer
            .as_ref()
            .ok_or_else(|| invalid("Squads member signing key absent"))?
            .1;
        Ok((multisig, proposer, member))
    }
    async fn account(
        &self,
        key: Pubkey,
        commitment: &str,
    ) -> Result<Option<SolanaAccount>, MintError> {
        self.rpc
            .get_account_info(&key.to_base58(), commitment)
            .await
            .map_err(MintError::Rpc)
    }
    fn commitment(&self) -> &'static str {
        match self.config.commitment {
            SolanaCommitment::Processed => "processed",
            SolanaCommitment::Confirmed => "confirmed",
            SolanaCommitment::Finalized => "finalized",
        }
    }
    pub(super) async fn validate_squads_custody(&self) -> Result<MultisigState, MintError> {
        let (multisig, proposer, member) = self.identity()?;
        if self.config.mint_threshold < 2 {
            return Err(invalid("Squads threshold must be at least two"));
        }
        for id in [self.program_id, SQUADS_V4_PROGRAM_ID] {
            let a = self
                .account(id, "finalized")
                .await?
                .ok_or_else(|| invalid("custody program missing"))?;
            let loaders = [
                "BPFLoaderUpgradeab1e11111111111111111111111",
                "BPFLoader2111111111111111111111111111111111",
            ];
            if !a.executable || !loaders.iter().any(|id| a.owner.to_base58() == *id) {
                return Err(invalid("custody program executable/loader owner mismatch"));
            }
        }
        let a = self
            .account(multisig, "finalized")
            .await?
            .ok_or_else(|| invalid("Squads multisig missing"))?;
        let state = MultisigState::parse(&a).map_err(invalid)?;
        let mut expected = self
            .config
            .mint_signers
            .iter()
            .map(|s| {
                let bytes = hex::decode(s).map_err(invalid)?;
                Ok(Pubkey(
                    bytes
                        .try_into()
                        .map_err(|_| invalid("member key must be 32 bytes"))?,
                ))
            })
            .collect::<Result<Vec<_>, MintError>>()?;
        expected.sort();
        if expected.windows(2).any(|w| w[0] == w[1])
            || state.members.iter().map(|m| m.0).collect::<Vec<_>>() != expected
            || state.members.iter().any(|m| m.1 != 7)
            || !expected.contains(&member)
            || !expected.contains(&proposer)
            || u32::from(state.threshold) != self.config.mint_threshold
            || state.config_authority != SYSTEM_PROGRAM_ID
            || squads::derive_multisig_pda(&state.create_key)? != multisig
        {
            return Err(invalid(
                "Squads custody membership/threshold/permissions/config authority mismatch",
            ));
        }
        let vault =
            squads::derive_vault_pda(&multisig, self.config.squads.as_ref().unwrap().vault_index)?;
        let vault_account = self
            .account(vault, "finalized")
            .await?
            .ok_or_else(|| invalid("vault rent account is not funded"))?;
        if vault_account.owner != SYSTEM_PROGRAM_ID
            || vault_account.executable
            || vault_account.lamports == 0
        {
            return Err(invalid("invalid/unfunded vault rent account"));
        }
        let bridge = self
            .account(self.bridge_pda, "finalized")
            .await?
            .ok_or_else(|| invalid("wbth bridge missing"))?;
        checked(&bridge, self.program_id, "Bridge").map_err(invalid)?;
        if parse_bridge_mint_authority(&bridge.data)? != vault {
            return Err(invalid("wbth authority is not configured Squads vault"));
        }
        let mint = parse_bridge_mint(&bridge.data)?;
        let a = self
            .account(mint, "finalized")
            .await?
            .ok_or_else(|| invalid("SPL mint missing"))?;
        if a.owner != TOKEN_PROGRAM_ID
            || a.executable
            || a.data.len() != 82
            || a.data[0..4] != 1u32.to_le_bytes()
            || a.data[4..36] != self.bridge_pda.0
            || a.data[44] != 12
            || a.data[45] != 1
        {
            return Err(invalid("invalid wbth SPL mint authority/decimals"));
        }
        Ok(state)
    }
    async fn context(
        &self,
        order: &BridgeOrder,
        index: u64,
    ) -> Result<SquadsMintContext, MintError> {
        let (multisig, _, member) = self.identity()?;
        let c = self.config.squads.as_ref().unwrap();
        let vault = squads::derive_vault_pda(&multisig, c.vault_index)?;
        let bridge = self
            .account(self.bridge_pda, self.commitment())
            .await?
            .ok_or_else(|| invalid("wbth bridge missing"))?;
        checked(&bridge, self.program_id, "Bridge").map_err(invalid)?;
        let mint = parse_bridge_mint(&bridge.data)?;
        let recipient = Pubkey::from_base58(&order.dest_address).map_err(invalid)?;
        let ata = derive_associated_token_account(&recipient, &mint)?;
        let marker = Pubkey::find_program_address(
            &[ORDER_MARKER_SEED, &order.order_id_bytes()],
            &self.program_id,
        )
        .ok_or_else(|| invalid("marker derivation"))?
        .0;
        let inner = build_bridge_mint_instruction(
            self.program_id,
            self.bridge_pda,
            marker,
            mint,
            ata,
            recipient,
            vault,
            order.net_amount(),
            order.order_id_bytes(),
        );
        SquadsMintContext::resolve(multisig, c.vault_index, index, member, inner)
    }
    fn binding(&self, ctx: &SquadsMintContext) -> String {
        format!(
            "{}:{}:{}",
            self.program_id.to_base58(),
            ctx.multisig.to_base58(),
            hex::encode(ctx.transaction_message().serialize())
        )
    }
    pub(super) async fn prepare_squads(
        &self,
        order: &BridgeOrder,
    ) -> Result<PreparedMint, MintError> {
        self.validate_squads_custody().await?;
        let ctx = self.context(order, 0).await?;
        self.store()?
            .claim_solana_intent(&order.id, &self.binding(&ctx), &ctx.multisig.to_base58())
            .map_err(invalid)?;
        // Explicit durable asynchronous operation handle, never treated as chain
        // evidence.
        Ok(PreparedMint {
            tx_id: format!("squads:{}", order.id),
            raw: vec![],
        })
    }
    fn valid_vault(
        &self,
        address: Pubkey,
        a: &SolanaAccount,
        ctx: &SquadsMintContext,
    ) -> Result<bool, MintError> {
        // Another legitimate Squads configuration transaction can consume the
        // shared index. Positive finalized ownership + account identity permit
        // abandoning this unverified candidate; malformed metadata never does.
        if a.data.starts_with(&discriminator("ConfigTransaction")) {
            let bytes = checked(a, SQUADS_V4_PROGRAM_ID, "ConfigTransaction").map_err(invalid)?;
            if bytes.len() < 72
                || bytes[..32] != ctx.multisig.0
                || u64::from_le_bytes(bytes[64..72].try_into().unwrap()) != ctx.transaction_index
            {
                return Err(invalid("competing config transaction binding mismatch"));
            }
            return Ok(false);
        }
        let tx = VaultState::parse(a).map_err(invalid)?;
        let (_, proposer, _) = self.identity()?;
        Ok(tx.multisig == ctx.multisig
            && tx.creator == proposer
            && tx.vault_index == ctx.vault_index
            && squads::derive_transaction_pda(&ctx.multisig, tx.index)? == address
            && tx.message == ctx.transaction_message())
    }
    async fn proposal(
        &self,
        ctx: &SquadsMintContext,
        commitment: &str,
    ) -> Result<Option<ProposalState>, MintError> {
        self.account(ctx.proposal_pda, commitment)
            .await?
            .map(|a| {
                let p = ProposalState::parse(&a).map_err(invalid)?;
                if p.multisig != ctx.multisig || p.index != ctx.transaction_index {
                    return Err(invalid("proposal binding mismatch"));
                }
                Ok(p)
            })
            .transpose()
    }
    /// Event attribution follows the invoke/success stack, so another program
    /// cannot spoof an identically encoded Program data line.
    async fn completed(
        &self,
        order: &BridgeOrder,
        row: &SolanaIntent,
        ctx: &SquadsMintContext,
    ) -> Result<Option<String>, MintError> {
        let Some(marker) = self
            .account(ctx.inner.accounts[1].pubkey, self.commitment())
            .await?
        else {
            return Ok(None);
        };
        let bytes = checked(&marker, self.program_id, "OrderMarker").map_err(invalid)?;
        if bytes != order.order_id_bytes() {
            return Err(invalid("order marker payload mismatch"));
        }
        if !row.verified {
            return Err(invalid(
                "mint marker exists without a verified durable proposal binding; reconcile history",
            ));
        }
        let proposal = self
            .proposal(ctx, self.commitment())
            .await?
            .ok_or_else(|| {
                invalid("executed proposal missing/closed; historical reconciliation required")
            })?;
        if proposal.status != 5 {
            return Ok(None);
        }
        for (signature, _) in self
            .rpc
            .get_signatures_for_address(
                &ctx.inner.accounts[1].pubkey.to_base58(),
                None,
                self.commitment(),
            )
            .await
            .map_err(MintError::Rpc)?
        {
            if let Some((logs, _)) = self
                .rpc
                .get_transaction_logs(&signature, self.commitment())
                .await
                .map_err(MintError::Rpc)?
            {
                if bound_mint_event(
                    &logs,
                    self.program_id,
                    ctx.inner.accounts[4].pubkey,
                    order.net_amount(),
                    order.order_id_bytes(),
                ) {
                    return Ok(Some(signature));
                }
            }
        }
        // RPC history is bounded; absence in one page is NOT proof of no mint.
        Err(invalid(
            "mint marker exists but matching execution evidence unavailable; retain backing",
        ))
    }
    pub(super) async fn advance_squads(
        &self,
        order: &BridgeOrder,
    ) -> Result<ConfirmationStatus, MintError> {
        let db = self.store()?;
        let row = db
            .solana_intent(&order.id)
            .map_err(invalid)?
            .ok_or_else(|| invalid("durable Squads intent missing"))?;
        let ctx = self.context(order, row.index.unwrap_or(0)).await?;
        if row.binding != self.binding(&ctx) {
            return Err(invalid("immutable Squads mint payload changed"));
        }
        // Read-only reconciliation still works after pause or custody-config drift.
        if row.index.is_some() {
            if let Some(signature) = self.completed(order, &row, &ctx).await? {
                let completed = SolanaAction {
                    kind: "completed".into(),
                    signature,
                    raw: vec![],
                    last_valid_height: 0,
                };
                if row.action.as_ref() != Some(&completed)
                    && !db
                        .update_solana_intent(&row, row.index, true, Some(&completed))
                        .map_err(invalid)?
                {
                    return Ok(pending());
                }
                return Ok(ConfirmationStatus::Confirmed);
            }
        }
        if row.index.is_none()
            && self
                .account(ctx.inner.accounts[1].pubkey, self.commitment())
                .await?
                .is_some()
        {
            return Err(invalid("mint marker predates local verified binding; historical reconciliation required, backing retained"));
        }
        let state = self.validate_squads_custody().await?;
        let (_, proposer, member) = self.identity()?;
        if row.index.is_none() {
            let accounts = self
                .rpc
                .get_program_accounts(
                    &SQUADS_V4_PROGRAM_ID.to_base58(),
                    &ctx.multisig.to_base58(),
                    "finalized",
                )
                .await
                .map_err(MintError::Rpc)?;
            let mut matches = vec![];
            for (address, a) in accounts {
                if !a.data.starts_with(&discriminator("VaultTransaction")) {
                    continue;
                }
                // Empty executed accounts and unrelated payloads are not candidates.
                let Ok(tx) = VaultState::parse(&a) else {
                    continue;
                };
                if self.valid_vault(address, &a, &ctx)? {
                    matches.push(tx.index)
                }
            }
            if matches.len() > 1 {
                return Err(invalid(
                    "multiple matching Squads proposals; refuse split approvals",
                ));
            }
            if let Some(index) = matches.first() {
                db.update_solana_intent(&row, Some(*index), true, None)
                    .map_err(invalid)?;
                return Ok(pending());
            }
            if member != proposer || db.is_paused().map_err(invalid)?.is_some() {
                return Ok(pending());
            }
            // Counter read and create-account reservation are durable. Different local
            // orders cannot both win the same candidate even across DB connections.
            let index = next_index(state.index)?;
            db.update_solana_intent(&row, Some(index), false, None)
                .map_err(invalid)?;
            return Ok(pending());
        }
        let proposal = self.proposal(&ctx, "finalized").await?;
        if let Some(p) = &proposal {
            // Approved stale vault transactions remain executable in Squads v4.
            // No stale-index/config change can release reserve backing.
            if p.status == 2 || p.status == 6 {
                return Err(invalid("bound Squads proposal rejected/cancelled; backing retained pending explicit recovery"));
            }
            if p.status == 5 {
                return Ok(pending());
            } // wait for required marker/event visibility
        }
        let tx = self.account(ctx.transaction_pda, "finalized").await?;
        if let Some(a) = tx.as_ref() {
            if !self.valid_vault(ctx.transaction_pda, a, &ctx)? {
                if row.verified {
                    return Err(invalid("bound Squads proposal payload changed"));
                }
                // The candidate was consumed by an unrelated order. A finalized account
                // is positive evidence; an RPC timeout/Unknown is never enough.
                if let Some(action) = &row.action {
                    if self.rpc.get_block_height().await.map_err(MintError::Rpc)?
                        <= action.last_valid_height
                    {
                        return Ok(pending());
                    }
                }
                db.update_solana_intent(&row, None, false, None)
                    .map_err(invalid)?;
                return Ok(pending());
            }
            if !row.verified {
                db.update_solana_intent(&row, row.index, true, None)
                    .map_err(invalid)?;
                return Ok(pending());
            }
        } else if row.verified {
            return Err(invalid(
                "previously verified proposal transaction missing; retain binding/backing",
            ));
        }
        if db.is_paused().map_err(invalid)?.is_some() {
            return Ok(pending());
        }
        let (kind, instructions) = if tx.is_none() {
            if member != proposer {
                return Ok(pending());
            }
            if ctx.transaction_index != next_index(state.index)? {
                return Err(invalid("candidate no longer next; reconcile before retry"));
            }
            let inner = &ctx.inner;
            let ata = Instruction {
                program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
                accounts: vec![
                    AccountMeta::writable_signer(member),
                    inner.accounts[3],
                    inner.accounts[4],
                    AccountMeta::readonly(inner.accounts[2].pubkey),
                    AccountMeta::readonly(SYSTEM_PROGRAM_ID),
                    AccountMeta::readonly(TOKEN_PROGRAM_ID),
                ],
                data: vec![1],
            };
            (
                "create",
                vec![
                    ata,
                    ctx.build_vault_transaction_create(),
                    ctx.build_proposal_create(),
                    ctx.build_proposal_approve(),
                ],
            )
        } else if proposal.is_none() {
            (
                "propose",
                vec![ctx.build_proposal_create(), ctx.build_proposal_approve()],
            )
        } else {
            let p = proposal.as_ref().unwrap();
            match p.status {
                1 if !p.approved.contains(&member) => {
                    ("approve", vec![ctx.build_proposal_approve()])
                }
                3 => {
                    if p.approved.len() < usize::from(state.threshold)
                        || p.approved
                            .iter()
                            .any(|k| !state.members.iter().any(|m| m.0 == *k))
                    {
                        return Err(invalid("proposal approval set mismatch"));
                    }
                    if chrono::Utc::now().timestamp() < unlock_at(p.timestamp, state.time_lock)? {
                        return Ok(pending());
                    }
                    ("execute", vec![ctx.build_vault_transaction_execute()])
                }
                _ => return Ok(pending()),
            }
        };
        self.advance_action(&row, kind, instructions).await?;
        Ok(pending())
    }
    async fn advance_action(
        &self,
        row: &SolanaIntent,
        kind: &str,
        instructions: Vec<Instruction>,
    ) -> Result<(), MintError> {
        let db = self.store()?;
        if db.is_paused().map_err(invalid)?.is_some() {
            return Ok(());
        }
        if let Some(action) = &row.action {
            if action.kind == kind {
                let state = self
                    .rpc
                    .get_signature_status(&action.signature)
                    .await
                    .map_err(MintError::Rpc)?;
                let expired = self.rpc.get_block_height().await.map_err(MintError::Rpc)?
                    > action.last_valid_height;
                match state {
                    SignatureState::Landed { err: None, .. } => return Ok(()),
                    SignatureState::Landed { err: Some(_), .. } => {
                        db.update_solana_intent(row, row.index, row.verified, None)
                            .map_err(invalid)?;
                        return Ok(());
                    }
                    SignatureState::Unknown if expired => {
                        db.update_solana_intent(row, row.index, row.verified, None)
                            .map_err(invalid)?;
                        return Ok(());
                    }
                    SignatureState::Unknown => {}
                }
                if db.is_paused().map_err(invalid)?.is_none() {
                    self.rpc
                        .send_transaction(&action.raw)
                        .await
                        .or_else(|e| {
                            if e == ALREADY_PROCESSED_MARKER {
                                Ok(action.signature.clone())
                            } else {
                                Err(e)
                            }
                        })
                        .map_err(MintError::Rpc)?;
                }
                return Ok(());
            }
        }
        let (hash, height) = self
            .rpc
            .get_latest_blockhash()
            .await
            .map_err(MintError::Rpc)?;
        if db.is_paused().map_err(invalid)?.is_some() {
            return Ok(());
        }
        let (sk, member) = self
            .signer
            .as_ref()
            .ok_or_else(|| invalid("member signing key missing"))?;
        let message = LegacyMessage::compile(*member, &instructions, hash);
        let transaction = Transaction {
            signatures: vec![sk.sign(&message.serialize()).to_bytes()],
            message,
        };
        let action = SolanaAction {
            kind: kind.into(),
            signature: transaction
                .signature_base58()
                .ok_or_else(|| invalid("missing transaction signature"))?,
            raw: transaction.serialize(),
            last_valid_height: height,
        };
        if db
            .update_solana_intent(row, row.index, row.verified, Some(&action))
            .map_err(invalid)?
            && db.is_paused().map_err(invalid)?.is_none()
        {
            self.rpc
                .send_transaction(&action.raw)
                .await
                .or_else(|e| {
                    if e == ALREADY_PROCESSED_MARKER {
                        Ok(action.signature.clone())
                    } else {
                        Err(e)
                    }
                })
                .map_err(MintError::Rpc)?;
        }
        Ok(())
    }
}

fn bound_mint_event(
    logs: &[String],
    program: Pubkey,
    user: Pubkey,
    amount: u64,
    order: [u8; 32],
) -> bool {
    // Propagate an event only when its invocation and every ancestor succeed.
    // A caller may catch a failed CPI; its rolled-back logs are not mint proof.
    let mut stack: Vec<(String, bool)> = vec![];
    let mut committed = false;
    let program = program.to_base58();
    let mut expected = Sha256::digest(b"event:BridgeMintEvent")[..8].to_vec();
    expected.extend_from_slice(&user.0);
    expected.extend_from_slice(&amount.to_le_bytes());
    expected.extend_from_slice(&order);
    for line in logs {
        if let Some(rest) = line.strip_prefix("Program ") {
            if let Some((id, _)) = rest.split_once(" invoke [") {
                stack.push((id.to_string(), false));
                continue;
            }
            if let Some(id) = rest.strip_suffix(" success") {
                let Some((active, matched)) = stack.pop() else {
                    return false;
                };
                if active != id {
                    return false;
                }
                if matched {
                    if let Some(parent) = stack.last_mut() {
                        parent.1 = true
                    } else {
                        committed = true
                    }
                }
                continue;
            }
            if let Some((id, _)) = rest.split_once(" failed:") {
                if stack.pop().is_none_or(|frame| frame.0 != id) {
                    return false;
                }
                continue;
            }
        }
        if let Some((active, matched)) = stack.last_mut() {
            if *active == program {
                if let Some(data) = line.strip_prefix("Program data: ") {
                    if let Ok(bytes) = crate::solana_rpc::base64_decode(data) {
                        if bytes.len() == 88 && bytes.starts_with(&expected) {
                            *matched = true
                        }
                    }
                }
            }
        }
    }
    committed && stack.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_counter_and_timelock_bounds_fail_closed() {
        assert!(next_index(u64::MAX).is_err());
        assert_eq!(next_index(u64::MAX - 1).unwrap(), u64::MAX);
        assert!(unlock_at(i64::MAX, 1).is_err());
        assert!(unlock_at(i64::MAX - 1, u32::MAX).is_err());
        assert_eq!(unlock_at(i64::MAX - 1, 1).unwrap(), i64::MAX);
    }
    #[test]
    fn mint_event_requires_real_program_attribution_and_exact_payload() {
        let program = Pubkey([1; 32]);
        let other = Pubkey([2; 32]);
        let user = Pubkey([3; 32]);
        let order = [4; 32];
        let mut bytes = Sha256::digest(b"event:BridgeMintEvent")[..8].to_vec();
        bytes.extend(user.0);
        bytes.extend(5u64.to_le_bytes());
        bytes.extend(order);
        bytes.extend(9u64.to_le_bytes());
        let data = format!("Program data: {}", crate::solana_rpc::base64_encode(&bytes));
        let logs = vec![
            format!("Program {} invoke [1]", other.to_base58()),
            format!("Program {} invoke [2]", program.to_base58()),
            data.clone(),
            format!("Program {} success", program.to_base58()),
            format!("Program {} success", other.to_base58()),
        ];
        assert!(bound_mint_event(&logs, program, user, 5, order));
        let mut caught_failure = logs.clone();
        caught_failure[4] = format!(
            "Program {} failed: custom program error: 0x1",
            other.to_base58()
        );
        assert!(!bound_mint_event(&caught_failure, program, user, 5, order));
        assert!(!bound_mint_event(&logs[..3], program, user, 5, order));

        assert!(!bound_mint_event(&logs, program, user, 6, order));
        assert!(!bound_mint_event(&logs, program, other, 5, order));
        assert!(!bound_mint_event(&logs, program, user, 5, [0; 32]));
        let spoof = vec![
            format!("Program {} invoke [1]", other.to_base58()),
            data.clone(),
            format!("Program {} success", other.to_base58()),
        ];
        assert!(!bound_mint_event(&spoof, program, user, 5, order));
        let after_return = vec![
            format!("Program {} invoke [1]", program.to_base58()),
            format!("Program {} success", program.to_base58()),
            data,
        ];
        assert!(!bound_mint_event(&after_return, program, user, 5, order));
    }
}
