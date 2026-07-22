// Copyright (c) 2024 The Botho Foundation

//! Squads Protocol v4 `vault_transaction` mint assembly (ADR 0012).
//!
//! ## Why this module exists
//!
//! Per [ADR 0002] the Solana wBTH `mint_authority` is a validator/federation
//! multisig. [ADR 0012] pins *which* multisig and *how* it mints:
//!
//! - **Model (ADR 0012 §1, Option A):** the on-chain `bridge.mint_authority` is
//!   a **Squads v4 vault PDA**. A vault PDA is off-curve and cannot sign with a
//!   private key, so the only way to satisfy the program's `mint_authority:
//!   Signer` constraint is a Squads `invoke_signed` CPI — the Squads program
//!   itself submits the inner `bridge_mint`. The relayer's local Ed25519 key is
//!   one Squads **member** (an approver), never the standalone authority.
//! - **Flow:** a federated mint is a Squads `vault_transaction` whose wrapped
//!   inner instruction is the *byte-identical*
//!   [`build_bridge_mint_instruction`] output: `vault_transaction_create`
//!   (records the wrapped `bridge_mint`) → `proposal_create` → `t` ×
//!   `proposal_approve` (distinct members, distinct machines) →
//!   `vault_transaction_execute` (the vault PDA `invoke_signed`s
//!   `bridge_mint`).
//!
//! This module provides the *assembly primitives* — the Squads program id, PDA
//! derivations, instruction encoders, the wrapped-message serializer, and the
//! composed [`assemble_vault_transaction_mint`] — all in the lightweight
//! raw-bytes style of [`crate::solana_rpc`] (no `solana-sdk`/`solana-client`,
//! no `squads-multisig` crate).
//!
//! ## Exactly-once (unchanged)
//!
//! Exactly-once stays anchored where ADR 0012 §2 puts it: the inner
//! `bridge_mint`'s per-order marker PDA (`seeds = [b"order", order_id]`, #850),
//! which fails `init` on a duplicate order id regardless of how many Squads
//! proposals or executes are attempted. This module changes *who submits*
//! `bridge_mint` (a Squads CPI instead of a lone key); it does not touch the
//! replay guard.
//!
//! ## ⚠️ Byte-layout provenance — MUST be verified before Tier-2/3
//!
//! ADR 0012 is explicit that it is **not** the byte-layout source of truth:
//! *"Verify the Squads v4 program id and all discriminators against the live
//! program / published v4 IDL before pinning."* Accordingly:
//!
//! - **Instruction discriminators** are the deterministic Anchor
//!   `sha256("global:<name>")[..8]` (Squads v4 is an Anchor program). These are
//!   self-verifying and pinned as known vectors in the tests — a rename or a
//!   drift in the derivation breaks CI, not a mainnet mint.
//! - **The Squads program id, the borsh argument layouts, the account-meta
//!   orders, and the wrapped `TransactionMessage` wire format** are implemented
//!   to the published Squads v4 program structure and pinned by the Tier-1 unit
//!   tests below, but they have **NOT** been executed against a live Squads v4
//!   program in this change (no Squads program is loaded in the Solana test
//!   harness yet — ADR 0012 §3 Tier-2/3, sequenced with the operator Squads
//!   setup #1086/#1052). Before the Tier-2 localnet e2e and any mainnet use,
//!   re-confirm every value in this module against the deployed Squads v4
//!   program / its published IDL. The Tier-1 tests here are drift detectors,
//!   not proof the CPI executes.
//!
//! [ADR 0002]: ../../../../docs/decisions/0002-bridge-custody-scp-validator-federation.md
//! [ADR 0012]: ../../../../docs/decisions/0012-solana-squads-pda-mint-execution.md

use crate::solana_rpc::{AccountMeta, Instruction, Pubkey, SYSTEM_PROGRAM_ID};

use super::{solana::anchor_discriminator, MintError};

/// Squads Protocol **v4** program id
/// (`SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf`), the canonical
/// mainnet/devnet v4 deployment pinned by ADR 0012 §1.
///
/// Stored as raw bytes so it is a `const` (matches
/// [`crate::solana_rpc::TOKEN_PROGRAM_ID`]). The
/// [`tests::test_squads_program_id_base58`] test asserts these bytes render to
/// the expected base58 string, so a typo in either representation is caught.
pub const SQUADS_V4_PROGRAM_ID: Pubkey = Pubkey([
    6, 129, 196, 206, 71, 226, 35, 104, 184, 177, 85, 94, 200, 135, 175, 9, 46, 252, 126, 251, 182,
    108, 163, 245, 47, 191, 104, 212, 172, 156, 183, 168,
]);

// --- Squads v4 PDA seed prefixes (from the program's `state` module) --------

/// Common seed prefix for every Squads v4 PDA (`b"multisig"`).
pub const SEED_PREFIX: &[u8] = b"multisig";
/// Multisig-account seed component (`b"multisig"`).
pub const SEED_MULTISIG: &[u8] = b"multisig";
/// Vault seed component (`b"vault"`).
pub const SEED_VAULT: &[u8] = b"vault";
/// Transaction seed component (`b"transaction"`).
pub const SEED_TRANSACTION: &[u8] = b"transaction";
/// Proposal seed component (`b"proposal"`).
pub const SEED_PROPOSAL: &[u8] = b"proposal";

// --- PDA derivations ---------------------------------------------------------

/// Derive the Squads **multisig** account PDA from its `create_key`
/// (`seeds = [b"multisig", b"multisig", create_key]`).
pub fn derive_multisig_pda(create_key: &Pubkey) -> Result<Pubkey, MintError> {
    Pubkey::find_program_address(
        &[SEED_PREFIX, SEED_MULTISIG, &create_key.0],
        &SQUADS_V4_PROGRAM_ID,
    )
    .map(|(pda, _bump)| pda)
    .ok_or_else(|| MintError::Config("could not derive squads multisig PDA".to_string()))
}

/// Derive a Squads **vault** PDA for `multisig` and `vault_index`
/// (`seeds = [b"multisig", multisig, b"vault", [vault_index]]`).
///
/// This is the account that becomes the on-chain `bridge.mint_authority`: the
/// program `invoke_signed`s `bridge_mint` on its behalf. Vault index 0 is the
/// default primary vault.
pub fn derive_vault_pda(multisig: &Pubkey, vault_index: u8) -> Result<Pubkey, MintError> {
    Pubkey::find_program_address(
        &[SEED_PREFIX, &multisig.0, SEED_VAULT, &[vault_index]],
        &SQUADS_V4_PROGRAM_ID,
    )
    .map(|(pda, _bump)| pda)
    .ok_or_else(|| MintError::Config("could not derive squads vault PDA".to_string()))
}

/// Derive the Squads **transaction** PDA for `multisig` and a
/// `transaction_index` (`seeds = [b"multisig", multisig, b"transaction",
/// transaction_index_le]`).
///
/// `transaction_index` is a **mutable monotonic counter on the multisig
/// account** (little-endian u64), NOT a function of `order_id` — see ADR 0012
/// §2's `transaction_index` wrinkle. Order-idempotent creation is the caller's
/// responsibility (persist `order_id → transaction_index`).
pub fn derive_transaction_pda(
    multisig: &Pubkey,
    transaction_index: u64,
) -> Result<Pubkey, MintError> {
    Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            &multisig.0,
            SEED_TRANSACTION,
            &transaction_index.to_le_bytes(),
        ],
        &SQUADS_V4_PROGRAM_ID,
    )
    .map(|(pda, _bump)| pda)
    .ok_or_else(|| MintError::Config("could not derive squads transaction PDA".to_string()))
}

/// Derive the Squads **proposal** PDA for a transaction index
/// (`seeds = [b"multisig", multisig, b"transaction", transaction_index_le,
/// b"proposal"]`).
pub fn derive_proposal_pda(multisig: &Pubkey, transaction_index: u64) -> Result<Pubkey, MintError> {
    Pubkey::find_program_address(
        &[
            SEED_PREFIX,
            &multisig.0,
            SEED_TRANSACTION,
            &transaction_index.to_le_bytes(),
            SEED_PROPOSAL,
        ],
        &SQUADS_V4_PROGRAM_ID,
    )
    .map(|(pda, _bump)| pda)
    .ok_or_else(|| MintError::Config("could not derive squads proposal PDA".to_string()))
}

// --- borsh helpers -----------------------------------------------------------

/// Append a borsh `Option<String>` (`0x00` for `None`; `0x01` + u32-le length +
/// UTF-8 bytes for `Some`).
fn push_borsh_option_string(out: &mut Vec<u8>, s: Option<&str>) {
    match s {
        None => out.push(0),
        Some(text) => {
            out.push(1);
            out.extend_from_slice(&(text.len() as u32).to_le_bytes());
            out.extend_from_slice(text.as_bytes());
        }
    }
}

// --- wrapped Squads `TransactionMessage` -------------------------------------

/// The Squads v4 `TransactionMessage` that `vault_transaction_create` records
/// and `vault_transaction_execute` replays via `invoke_signed`.
///
/// It is a compact structure distinct from a Solana `LegacyMessage`: account
/// counts are `u8`, the account list and per-instruction account-index list are
/// `SmallVec<u8, _>` (a `u8` length prefix), and instruction data is
/// `SmallVec<u16, u8>` (a little-endian `u16` length prefix). There is no fee
/// payer and no blockhash — the outer Squads transaction carries those.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SquadsTransactionMessage {
    /// Number of signer accounts (leading entries of `account_keys`).
    pub num_signers: u8,
    /// Of the signers, how many are writable.
    pub num_writable_signers: u8,
    /// Of the non-signers, how many are writable.
    pub num_writable_non_signers: u8,
    /// Unique account keys, ordered: writable-signers, readonly-signers,
    /// writable-non-signers, readonly-non-signers.
    pub account_keys: Vec<Pubkey>,
    /// Instructions referencing `account_keys` by index.
    pub instructions: Vec<SquadsCompiledInstruction>,
}

/// A compiled instruction inside a [`SquadsTransactionMessage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SquadsCompiledInstruction {
    /// Index of the program id in `account_keys`.
    pub program_id_index: u8,
    /// Indices of the instruction's accounts in `account_keys`.
    pub account_indexes: Vec<u8>,
    /// Raw instruction data.
    pub data: Vec<u8>,
}

impl SquadsTransactionMessage {
    /// Compile a wrapped message from the `vault` PDA (the sole inner signer,
    /// resolved by `invoke_signed` at execute) and the inner instructions.
    ///
    /// Mirrors Solana's account partition/privilege rules — the strongest
    /// (`is_signer`, `is_writable`) seen for a key wins — but emits the Squads
    /// `u8` counts rather than a Solana message header. The `vault` is forced
    /// to a writable signer because the inner `bridge_mint` presents it as the
    /// writable `mint_authority`/rent-payer.
    pub fn compile(vault: Pubkey, instructions: &[Instruction]) -> Self {
        use std::collections::BTreeMap;

        let mut privileges: BTreeMap<Pubkey, (bool, bool)> = BTreeMap::new();
        // The vault is the invoke_signed signer and pays order-marker rent.
        privileges.insert(vault, (true, true));
        for ix in instructions {
            privileges.entry(ix.program_id).or_insert((false, false));
            for meta in &ix.accounts {
                privileges
                    .entry(meta.pubkey)
                    .and_modify(|(s, w)| {
                        *s |= meta.is_signer;
                        *w |= meta.is_writable;
                    })
                    .or_insert((meta.is_signer, meta.is_writable));
            }
        }

        let mut writable_signers = Vec::new();
        let mut readonly_signers = Vec::new();
        let mut writable_nonsigners = Vec::new();
        let mut readonly_nonsigners = Vec::new();
        for (key, (is_signer, is_writable)) in privileges {
            match (is_signer, is_writable) {
                (true, true) => writable_signers.push(key),
                (true, false) => readonly_signers.push(key),
                (false, true) => writable_nonsigners.push(key),
                (false, false) => readonly_nonsigners.push(key),
            }
        }

        let num_signers = (writable_signers.len() + readonly_signers.len()) as u8;
        let num_writable_signers = writable_signers.len() as u8;
        let num_writable_non_signers = writable_nonsigners.len() as u8;

        let mut account_keys = Vec::new();
        account_keys.extend(writable_signers);
        account_keys.extend(readonly_signers);
        account_keys.extend(writable_nonsigners);
        account_keys.extend(readonly_nonsigners);

        let index_of = |key: &Pubkey| account_keys.iter().position(|k| k == key).unwrap() as u8;

        let compiled = instructions
            .iter()
            .map(|ix| SquadsCompiledInstruction {
                program_id_index: index_of(&ix.program_id),
                account_indexes: ix.accounts.iter().map(|m| index_of(&m.pubkey)).collect(),
                data: ix.data.clone(),
            })
            .collect();

        SquadsTransactionMessage {
            num_signers,
            num_writable_signers,
            num_writable_non_signers,
            account_keys,
            instructions: compiled,
        }
    }

    /// Serialize to the Squads v4 `transaction_message: Vec<u8>` wire bytes
    /// that `vault_transaction_create` records.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = vec![
            self.num_signers,
            self.num_writable_signers,
            self.num_writable_non_signers,
            // account_keys: SmallVec<u8, Pubkey> (u8 length prefix)
            self.account_keys.len() as u8,
        ];
        for key in &self.account_keys {
            out.extend_from_slice(&key.0);
        }

        // instructions: SmallVec<u8, CompiledInstruction>
        out.push(self.instructions.len() as u8);
        for ix in &self.instructions {
            out.push(ix.program_id_index);
            // account_indexes: SmallVec<u8, u8>
            out.push(ix.account_indexes.len() as u8);
            out.extend_from_slice(&ix.account_indexes);
            // data: SmallVec<u16, u8>
            out.extend_from_slice(&(ix.data.len() as u16).to_le_bytes());
            out.extend_from_slice(&ix.data);
        }

        // address_table_lookups: SmallVec<u8, _> — always empty (legacy).
        out.push(0);
        out
    }
}

// --- instruction data encoders ----------------------------------------------

/// `vault_transaction_create(args)` instruction data: the Anchor discriminator
/// then the borsh-encoded `VaultTransactionCreateArgs`
/// (`vault_index: u8`, `ephemeral_signers: u8`, `transaction_message: Vec<u8>`,
/// `memo: Option<String>`).
pub fn encode_vault_transaction_create_data(
    vault_index: u8,
    ephemeral_signers: u8,
    transaction_message: &[u8],
    memo: Option<&str>,
) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&anchor_discriminator("vault_transaction_create"));
    data.push(vault_index);
    data.push(ephemeral_signers);
    // transaction_message: Vec<u8> -> borsh u32-le length + bytes.
    data.extend_from_slice(&(transaction_message.len() as u32).to_le_bytes());
    data.extend_from_slice(transaction_message);
    push_borsh_option_string(&mut data, memo);
    data
}

/// `proposal_create(args)` instruction data: discriminator then
/// `ProposalCreateArgs` (`transaction_index: u64`, `draft: bool`).
pub fn encode_proposal_create_data(transaction_index: u64, draft: bool) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&anchor_discriminator("proposal_create"));
    data.extend_from_slice(&transaction_index.to_le_bytes());
    data.push(draft as u8);
    data
}

/// `proposal_approve(args)` instruction data: discriminator then
/// `ProposalVoteArgs` (`memo: Option<String>`).
pub fn encode_proposal_approve_data(memo: Option<&str>) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&anchor_discriminator("proposal_approve"));
    push_borsh_option_string(&mut data, memo);
    data
}

/// `vault_transaction_execute()` instruction data: the discriminator only
/// (no args).
pub fn encode_vault_transaction_execute_data() -> Vec<u8> {
    anchor_discriminator("vault_transaction_execute").to_vec()
}

// --- instruction builders ----------------------------------------------------

/// All accounts/indices needed to derive and reference one order's Squads
/// proposal. Resolved once (from the multisig, vault index, order's inner
/// instruction, and the chosen `transaction_index`) and shared by every
/// builder below so the four instructions stay mutually consistent.
#[derive(Debug, Clone)]
pub struct SquadsMintContext {
    /// The Squads multisig account PDA.
    pub multisig: Pubkey,
    /// The vault PDA (the on-chain `bridge.mint_authority`).
    pub vault: Pubkey,
    /// The vault index the `vault` PDA was derived at.
    pub vault_index: u8,
    /// The monotonic transaction index this order's proposal lives at.
    pub transaction_index: u64,
    /// The transaction PDA for `transaction_index`.
    pub transaction_pda: Pubkey,
    /// The proposal PDA for `transaction_index`.
    pub proposal_pda: Pubkey,
    /// The local member key (creator / rent-payer / approver / executor).
    pub member: Pubkey,
    /// The wrapped inner `bridge_mint` instruction (byte-identical to
    /// [`super::solana::build_bridge_mint_instruction`]).
    pub inner: Instruction,
}

impl SquadsMintContext {
    /// Resolve every PDA for `order`'s Squads proposal from the multisig, the
    /// vault index, the chosen `transaction_index`, the local `member`, and the
    /// already-built byte-identical inner `bridge_mint` instruction.
    pub fn resolve(
        multisig: Pubkey,
        vault_index: u8,
        transaction_index: u64,
        member: Pubkey,
        inner: Instruction,
    ) -> Result<Self, MintError> {
        let vault = derive_vault_pda(&multisig, vault_index)?;
        let transaction_pda = derive_transaction_pda(&multisig, transaction_index)?;
        let proposal_pda = derive_proposal_pda(&multisig, transaction_index)?;
        Ok(Self {
            multisig,
            vault,
            vault_index,
            transaction_index,
            transaction_pda,
            proposal_pda,
            member,
            inner,
        })
    }

    /// The wrapped `TransactionMessage` for this order's inner `bridge_mint`.
    pub fn transaction_message(&self) -> SquadsTransactionMessage {
        SquadsTransactionMessage::compile(self.vault, std::slice::from_ref(&self.inner))
    }

    /// Build the `vault_transaction_create` [`Instruction`].
    ///
    /// Accounts (Squads v4 `VaultTransactionCreate` order): multisig (mut),
    /// transaction PDA (mut, init), creator (signer), rent_payer (mut, signer),
    /// system program. The local member is both creator and rent_payer.
    pub fn build_vault_transaction_create(&self) -> Instruction {
        let message = self.transaction_message().serialize();
        Instruction {
            program_id: SQUADS_V4_PROGRAM_ID,
            accounts: vec![
                AccountMeta::writable(self.multisig),
                AccountMeta::writable(self.transaction_pda),
                AccountMeta::readonly_signer(self.member),
                AccountMeta::writable_signer(self.member),
                AccountMeta::readonly(SYSTEM_PROGRAM_ID),
            ],
            data: encode_vault_transaction_create_data(self.vault_index, 0, &message, None),
        }
    }

    /// Build the `proposal_create` [`Instruction`].
    ///
    /// Accounts (Squads v4 `ProposalCreate` order): multisig, proposal PDA
    /// (mut, init), creator (signer), rent_payer (mut, signer), system program.
    pub fn build_proposal_create(&self) -> Instruction {
        Instruction {
            program_id: SQUADS_V4_PROGRAM_ID,
            accounts: vec![
                AccountMeta::readonly(self.multisig),
                AccountMeta::writable(self.proposal_pda),
                AccountMeta::readonly_signer(self.member),
                AccountMeta::writable_signer(self.member),
                AccountMeta::readonly(SYSTEM_PROGRAM_ID),
            ],
            data: encode_proposal_create_data(self.transaction_index, false),
        }
    }

    /// Build a `proposal_approve` [`Instruction`] for the local member.
    ///
    /// Accounts (Squads v4 `ProposalVote` order): multisig, member (signer),
    /// proposal PDA (mut).
    pub fn build_proposal_approve(&self) -> Instruction {
        Instruction {
            program_id: SQUADS_V4_PROGRAM_ID,
            accounts: vec![
                AccountMeta::readonly(self.multisig),
                AccountMeta::readonly_signer(self.member),
                AccountMeta::writable(self.proposal_pda),
            ],
            data: encode_proposal_approve_data(None),
        }
    }

    /// Build the `vault_transaction_execute` [`Instruction`] (the
    /// `invoke_signed` step).
    ///
    /// Accounts (Squads v4 `VaultTransactionExecute` order): multisig, proposal
    /// PDA (mut), transaction PDA, member (signer), then the wrapped message's
    /// accounts and program as `remaining_accounts`. Squads resolves the vault
    /// PDA signer via `invoke_signed`; the vault appears in the remaining
    /// accounts as a writable non-signer (the program provides its signature).
    pub fn build_vault_transaction_execute(&self) -> Instruction {
        let mut accounts = vec![
            AccountMeta::readonly(self.multisig),
            AccountMeta::writable(self.proposal_pda),
            AccountMeta::readonly(self.transaction_pda),
            AccountMeta::readonly_signer(self.member),
        ];
        // remaining_accounts: every account the wrapped bridge_mint touches,
        // plus its program id, so the CPI can be replayed. The vault is a
        // program-signed PDA here (writable, not a transaction signer).
        let message = self.transaction_message();
        let signer_count = message.num_signers as usize;
        let writable_signers = message.num_writable_signers as usize;
        let writable_non_signers = message.num_writable_non_signers as usize;
        for (idx, key) in message.account_keys.iter().enumerate() {
            // Writable iff in the leading writable-signer band or the
            // writable-non-signer band of the partition.
            let is_writable = idx < writable_signers
                || (idx >= signer_count && idx < signer_count + writable_non_signers);
            // The vault PDA (and any inner signer) is program-signed via
            // invoke_signed, so every remaining account is passed as a
            // NON-signer of the outer execute transaction (only `member`
            // signs it).
            if is_writable {
                accounts.push(AccountMeta::writable(*key));
            } else {
                accounts.push(AccountMeta::readonly(*key));
            }
        }
        Instruction {
            program_id: SQUADS_V4_PROGRAM_ID,
            accounts,
            data: encode_vault_transaction_execute_data(),
        }
    }
}

/// The ordered Squads instructions that assemble and execute a federated wBTH
/// mint, per ADR 0012's flow.
#[derive(Debug, Clone)]
pub struct VaultTransactionMintAssembly {
    /// This node's create+propose+approve contribution (ADR 0012 §2
    /// `prepare_mint`): the three instructions a single relayer member submits
    /// to open the proposal and record its own approval. Idempotent create is
    /// the caller's responsibility (reuse `transaction_index` on retry).
    pub contribution: Vec<Instruction>,
    /// The `vault_transaction_execute` instruction (ADR 0012 §2
    /// `check_confirmation` at threshold): submitted once `t` approvals exist;
    /// the first executor wins and the order-marker PDA makes the rest no-ops.
    pub execute: Instruction,
}

/// Assemble the full Squads `vault_transaction` mint for one order from a
/// resolved [`SquadsMintContext`].
///
/// Returns both this node's contribution (create → proposal_create →
/// proposal_approve) and the terminal `vault_transaction_execute`, so the mint
/// engine can submit the contribution in `prepare_mint`/`broadcast` and the
/// execute in `check_confirmation` once the on-chain proposal reaches
/// threshold.
pub fn assemble_vault_transaction_mint(ctx: &SquadsMintContext) -> VaultTransactionMintAssembly {
    VaultTransactionMintAssembly {
        contribution: vec![
            ctx.build_vault_transaction_create(),
            ctx.build_proposal_create(),
            ctx.build_proposal_approve(),
        ],
        execute: ctx.build_vault_transaction_execute(),
    }
}

// --- multisig account parsing (transaction_index) ----------------------------

/// Byte offset of the `transaction_index: u64` field inside a Squads v4
/// `Multisig` account: 8-byte Anchor discriminator + `create_key`(32) +
/// `config_authority`(32) + `threshold: u16`(2) + `time_lock: u32`(4).
pub const MULTISIG_TRANSACTION_INDEX_OFFSET: usize = 8 + 32 + 32 + 2 + 4;

/// Read the multisig's current `transaction_index` (the highest index used so
/// far) from raw `Multisig` account data. The next proposal is created at
/// `transaction_index + 1`. Bounds-checked — never a truncated read.
///
/// ⚠️ The offset is derived from the published Squads v4 `Multisig` layout and
/// must be re-confirmed against the live program before Tier-2/3 (see module
/// docs).
pub fn parse_multisig_transaction_index(data: &[u8]) -> Result<u64, MintError> {
    let end = MULTISIG_TRANSACTION_INDEX_OFFSET + 8;
    if data.len() < end {
        return Err(MintError::Rpc(format!(
            "squads multisig account too small ({} bytes) to hold transaction_index",
            data.len()
        )));
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&data[MULTISIG_TRANSACTION_INDEX_OFFSET..end]);
    Ok(u64::from_le_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mint::solana::build_bridge_mint_instruction, solana_rpc::TOKEN_PROGRAM_ID};

    fn sample_inner() -> Instruction {
        // A byte-identical bridge_mint inner instruction (the thing Squads
        // wraps). The mint_authority here is the VAULT PDA.
        let vault = derive_vault_pda(&Pubkey([0x11u8; 32]), 0).unwrap();
        build_bridge_mint_instruction(
            Pubkey([5u8; 32]), // wbth program
            Pubkey([1u8; 32]), // bridge PDA
            Pubkey([2u8; 32]), // order marker PDA
            Pubkey([3u8; 32]), // mint
            Pubkey([4u8; 32]), // recipient ATA
            Pubkey([6u8; 32]), // recipient
            vault,             // mint_authority == vault PDA
            100,
            [9u8; 32],
        )
    }

    fn sample_ctx() -> SquadsMintContext {
        let multisig = Pubkey([0x11u8; 32]);
        let member = Pubkey([0x22u8; 32]);
        SquadsMintContext::resolve(multisig, 0, 7, member, sample_inner()).unwrap()
    }

    #[test]
    fn test_squads_program_id_base58() {
        // The pinned raw bytes render to the canonical v4 program id.
        assert_eq!(
            SQUADS_V4_PROGRAM_ID.to_base58(),
            "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf"
        );
    }

    #[test]
    fn test_instruction_discriminators_known_vectors() {
        // Deterministic Anchor discriminators (sha256("global:<name>")[..8]),
        // pinned so a rename or a derivation drift breaks CI, not a mint.
        assert_eq!(
            &anchor_discriminator("vault_transaction_create"),
            &[48, 250, 78, 168, 208, 226, 218, 211]
        );
        assert_eq!(
            &anchor_discriminator("proposal_create"),
            &[220, 60, 73, 224, 30, 108, 79, 159]
        );
        assert_eq!(
            &anchor_discriminator("proposal_approve"),
            &[144, 37, 164, 136, 188, 216, 42, 248]
        );
        assert_eq!(
            &anchor_discriminator("vault_transaction_execute"),
            &[194, 8, 161, 87, 153, 164, 25, 171]
        );
    }

    #[test]
    fn test_pdas_are_off_curve_and_deterministic() {
        let create_key = Pubkey([0x11u8; 32]);
        let multisig = derive_multisig_pda(&create_key).unwrap();
        // Deterministic.
        assert_eq!(multisig, derive_multisig_pda(&create_key).unwrap());
        // Distinct derivations differ.
        let vault = derive_vault_pda(&multisig, 0).unwrap();
        let vault1 = derive_vault_pda(&multisig, 1).unwrap();
        let tx7 = derive_transaction_pda(&multisig, 7).unwrap();
        let tx8 = derive_transaction_pda(&multisig, 8).unwrap();
        let prop7 = derive_proposal_pda(&multisig, 7).unwrap();
        assert_ne!(vault, vault1);
        assert_ne!(tx7, tx8);
        assert_ne!(tx7, prop7);
        assert_ne!(multisig, vault);
        // Transaction/proposal PDAs are index-deterministic.
        assert_eq!(tx7, derive_transaction_pda(&multisig, 7).unwrap());
        assert_eq!(prop7, derive_proposal_pda(&multisig, 7).unwrap());
    }

    #[test]
    fn test_vault_transaction_create_data_layout() {
        let msg = vec![0xAB, 0xCD, 0xEF];
        let data = encode_vault_transaction_create_data(2, 0, &msg, None);
        // discriminator(8) | vault_index(1) | ephemeral(1) | len u32-le(4) |
        // msg(3) | memo None(1)
        assert_eq!(
            &data[..8],
            &anchor_discriminator("vault_transaction_create")
        );
        assert_eq!(data[8], 2); // vault_index
        assert_eq!(data[9], 0); // ephemeral_signers
        assert_eq!(&data[10..14], &3u32.to_le_bytes()); // Vec<u8> len
        assert_eq!(&data[14..17], &msg[..]);
        assert_eq!(data[17], 0); // memo None
        assert_eq!(data.len(), 8 + 1 + 1 + 4 + 3 + 1);

        // memo Some path.
        let with_memo = encode_vault_transaction_create_data(0, 0, &[], Some("hi"));
        // ... | len u32-le(0) | memo Some(1) | strlen u32-le(4) | "hi"(2)
        let tail = &with_memo[10..];
        assert_eq!(&tail[..4], &0u32.to_le_bytes());
        assert_eq!(tail[4], 1); // Some
        assert_eq!(&tail[5..9], &2u32.to_le_bytes());
        assert_eq!(&tail[9..11], b"hi");
    }

    #[test]
    fn test_proposal_create_data_layout() {
        let data = encode_proposal_create_data(7, false);
        assert_eq!(&data[..8], &anchor_discriminator("proposal_create"));
        assert_eq!(&data[8..16], &7u64.to_le_bytes());
        assert_eq!(data[16], 0); // draft = false
        assert_eq!(data.len(), 8 + 8 + 1);
        // draft = true flips the last byte.
        assert_eq!(encode_proposal_create_data(7, true)[16], 1);
    }

    #[test]
    fn test_proposal_approve_data_layout() {
        let data = encode_proposal_approve_data(None);
        assert_eq!(&data[..8], &anchor_discriminator("proposal_approve"));
        assert_eq!(data[8], 0); // memo None
        assert_eq!(data.len(), 9);
    }

    #[test]
    fn test_vault_transaction_execute_data_is_discriminator_only() {
        let data = encode_vault_transaction_execute_data();
        assert_eq!(data, anchor_discriminator("vault_transaction_execute"));
        assert_eq!(data.len(), 8);
    }

    #[test]
    fn test_wrapped_inner_instruction_is_byte_identical() {
        // ADR 0012 §3 Tier-1: the wrapped inner instruction MUST be
        // byte-identical to today's build_bridge_mint_instruction output.
        let ctx = sample_ctx();
        let message = ctx.transaction_message();

        // Exactly one wrapped instruction, and its data equals the inner
        // bridge_mint's data verbatim.
        assert_eq!(message.instructions.len(), 1);
        assert_eq!(message.instructions[0].data, ctx.inner.data);

        // Every inner account resolves back to the same pubkey through the
        // wrapped message's account_keys index table (no reordering loss).
        let compiled = &message.instructions[0];
        assert_eq!(compiled.account_indexes.len(), ctx.inner.accounts.len());
        for (meta, &idx) in ctx.inner.accounts.iter().zip(&compiled.account_indexes) {
            assert_eq!(message.account_keys[idx as usize], meta.pubkey);
        }
        // The program id also resolves back.
        assert_eq!(
            message.account_keys[compiled.program_id_index as usize],
            ctx.inner.program_id
        );
    }

    #[test]
    fn test_transaction_message_partition_and_counts() {
        let ctx = sample_ctx();
        let message = ctx.transaction_message();

        // The vault PDA leads as the sole writable signer.
        assert_eq!(message.account_keys[0], ctx.vault);
        assert_eq!(message.num_writable_signers, 1);
        assert_eq!(message.num_signers, 1);

        // bridge_mint writable non-signers: bridge, marker, mint, ATA (4).
        assert_eq!(message.num_writable_non_signers, 4);

        // Serialization begins with the three u8 counts, then the u8
        // account-key count.
        let bytes = message.serialize();
        assert_eq!(bytes[0], message.num_signers);
        assert_eq!(bytes[1], message.num_writable_signers);
        assert_eq!(bytes[2], message.num_writable_non_signers);
        assert_eq!(bytes[3] as usize, message.account_keys.len());
        // Account keys occupy 32 bytes each after the 4-byte prefix.
        let keys_end = 4 + 32 * message.account_keys.len();
        assert_eq!(
            &bytes[4..4 + 32],
            &ctx.vault.0,
            "first serialized account key is the vault PDA"
        );
        // Instruction count (SmallVec<u8>) follows the account keys.
        assert_eq!(bytes[keys_end], 1);
    }

    #[test]
    fn test_transaction_message_data_uses_u16_length_prefix() {
        // The inner instruction data length is a little-endian u16 (SmallVec
        // <u16, u8>), distinct from account_indexes' u8 prefix.
        let ctx = sample_ctx();
        let message = ctx.transaction_message();
        let bytes = message.serialize();
        let data_len = ctx.inner.data.len(); // 48 (8 disc + 8 amount + 32 order)
                                             // Find the inner data bytes and confirm they're preceded by their
                                             // u16-le length.
        let needle = &ctx.inner.data;
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
            .expect("inner data present");
        assert_eq!(&bytes[pos - 2..pos], &(data_len as u16).to_le_bytes());
    }

    #[test]
    fn test_vault_transaction_create_account_metas() {
        let ctx = sample_ctx();
        let ix = ctx.build_vault_transaction_create();
        assert_eq!(ix.program_id, SQUADS_V4_PROGRAM_ID);
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(ix.accounts[0], AccountMeta::writable(ctx.multisig));
        assert_eq!(ix.accounts[1], AccountMeta::writable(ctx.transaction_pda));
        assert_eq!(ix.accounts[2], AccountMeta::readonly_signer(ctx.member));
        assert_eq!(ix.accounts[3], AccountMeta::writable_signer(ctx.member));
        assert_eq!(ix.accounts[4], AccountMeta::readonly(SYSTEM_PROGRAM_ID));
    }

    #[test]
    fn test_proposal_create_account_metas() {
        let ctx = sample_ctx();
        let ix = ctx.build_proposal_create();
        assert_eq!(ix.accounts.len(), 5);
        assert_eq!(ix.accounts[0], AccountMeta::readonly(ctx.multisig));
        assert_eq!(ix.accounts[1], AccountMeta::writable(ctx.proposal_pda));
        assert_eq!(ix.accounts[2], AccountMeta::readonly_signer(ctx.member));
        assert_eq!(ix.accounts[3], AccountMeta::writable_signer(ctx.member));
        assert_eq!(ix.accounts[4], AccountMeta::readonly(SYSTEM_PROGRAM_ID));
    }

    #[test]
    fn test_proposal_approve_account_metas() {
        let ctx = sample_ctx();
        let ix = ctx.build_proposal_approve();
        assert_eq!(ix.accounts.len(), 3);
        assert_eq!(ix.accounts[0], AccountMeta::readonly(ctx.multisig));
        assert_eq!(ix.accounts[1], AccountMeta::readonly_signer(ctx.member));
        assert_eq!(ix.accounts[2], AccountMeta::writable(ctx.proposal_pda));
    }

    #[test]
    fn test_vault_transaction_execute_account_metas() {
        let ctx = sample_ctx();
        let ix = ctx.build_vault_transaction_execute();
        // Leading fixed accounts.
        assert_eq!(ix.accounts[0], AccountMeta::readonly(ctx.multisig));
        assert_eq!(ix.accounts[1], AccountMeta::writable(ctx.proposal_pda));
        assert_eq!(ix.accounts[2], AccountMeta::readonly(ctx.transaction_pda));
        assert_eq!(ix.accounts[3], AccountMeta::readonly_signer(ctx.member));
        // Remaining accounts mirror the wrapped message's account keys.
        let message = ctx.transaction_message();
        assert_eq!(ix.accounts.len(), 4 + message.account_keys.len());
        // The vault PDA is present but NOT a transaction signer (invoke_signed).
        let vault_meta = ix
            .accounts
            .iter()
            .find(|m| m.pubkey == ctx.vault)
            .expect("vault in remaining accounts");
        assert!(
            !vault_meta.is_signer,
            "vault is program-signed, not a tx signer"
        );
        assert!(vault_meta.is_writable, "vault pays order-marker rent");
        // Only the member signs the outer execute transaction.
        let signers: Vec<_> = ix.accounts.iter().filter(|m| m.is_signer).collect();
        assert_eq!(signers.len(), 1);
        assert_eq!(signers[0].pubkey, ctx.member);
        // The token & system programs ride along as readonly.
        assert!(ix
            .accounts
            .iter()
            .any(|m| m.pubkey == TOKEN_PROGRAM_ID && !m.is_writable));
    }

    #[test]
    fn test_assemble_vault_transaction_mint_ordering() {
        let ctx = sample_ctx();
        let assembly = assemble_vault_transaction_mint(&ctx);
        // Contribution is create -> proposal_create -> approve, in order.
        assert_eq!(assembly.contribution.len(), 3);
        assert_eq!(
            &assembly.contribution[0].data[..8],
            &anchor_discriminator("vault_transaction_create")
        );
        assert_eq!(
            &assembly.contribution[1].data[..8],
            &anchor_discriminator("proposal_create")
        );
        assert_eq!(
            &assembly.contribution[2].data[..8],
            &anchor_discriminator("proposal_approve")
        );
        assert_eq!(
            &assembly.execute.data[..8],
            &anchor_discriminator("vault_transaction_execute")
        );
    }

    #[test]
    fn test_proposal_index_binds_create_and_approve() {
        // proposal_create's transaction_index arg matches the index the
        // transaction/proposal PDAs were derived at — a mismatch would split
        // approvals across proposals (ADR 0012 §2 transaction_index wrinkle).
        let ctx = sample_ctx();
        let create = ctx.build_proposal_create();
        assert_eq!(&create.data[8..16], &ctx.transaction_index.to_le_bytes());
        assert_eq!(
            ctx.proposal_pda,
            derive_proposal_pda(&ctx.multisig, ctx.transaction_index).unwrap()
        );
        assert_eq!(
            ctx.transaction_pda,
            derive_transaction_pda(&ctx.multisig, ctx.transaction_index).unwrap()
        );
    }

    #[test]
    fn test_parse_multisig_transaction_index() {
        // discriminator(8) | create_key(32) | config_authority(32) |
        // threshold u16(2) | time_lock u32(4) | transaction_index u64-le(8)
        let mut data = vec![0u8; MULTISIG_TRANSACTION_INDEX_OFFSET];
        data.extend_from_slice(&42u64.to_le_bytes());
        data.extend_from_slice(&[0u8; 64]); // trailing fields
        assert_eq!(parse_multisig_transaction_index(&data).unwrap(), 42);
        // Truncated -> error, never a partial read.
        assert!(parse_multisig_transaction_index(&[0u8; 10]).is_err());
    }
}
