//! Native node-wallet reads through persisted derivation contexts (#1286 P7).
//!
//! Under `botho.ledger.2` an output's one-time key is `base + tweak`, where the
//! base is the ordinary stealth derivation at the context's `base_index` and
//! the tweak is zero for direct outputs and the cumulative V2 lottery tweak
//! for payouts. A V2 payout therefore has its own target key (and its own key
//! image), so the classical `scan_output(&utxo.output, output_index)` cannot
//! see it. Every node-wallet path (balance, faucet, settlement, minting
//! balance gate, input recovery) instead goes through [`recover_owned`], the
//! one recovery rule shared with the private experimental wallet
//! (`experimental/validated/wallet.rs`, #1360/#1365), which ends in the clsag
//! `x·G = P` check: [`clsag_v2::recover_direct_owner`] (over
//! [`clsag_v2::recover_direct`]) for a `Direct` context and
//! [`clsag_v2::recover_payout_owner`] (over [`clsag_v2::recover_with_context`])
//! for a `Lottery` one. The dispatch is load-bearing: `recover_with_context`
//! fails closed on the zero tweak (#1667), so routing a direct output through
//! it would make every ordinary output vanish from balance, faucet and minting
//! gate.
//!
//! Contexts are read strictly from P2's store ([`read_context`]): a missing or
//! inconsistent context is an error, never a Direct default. Legacy aliased
//! rows (a second outpoint whose target key is already indexed to an earlier
//! outpoint, the V1 payout shape) are never reported as owned, exactly as
//! `Wallet::canonical_inputs` / `WalletRead::validate_input` exclude them.
use super::*;
use crate::{transaction::TxOutput, wallet::Wallet};
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_ring_signature::KeyImage;
use bth_transaction_clsag::lottery_v2 as clsag_v2;
use tracing::debug;

/// Recover `(subaddress, one-time key)` for `output` under `context` with a
/// caller-supplied base recovery. Shared by every wallet adapter. A `Direct`
/// context (no tweak) goes to [`clsag_v2::recover_direct_owner`]; a `Lottery`
/// context goes to [`clsag_v2::recover_payout_owner`], which rejects a zero or
/// non-canonical tweak before the base callback runs. Both end in the same
/// final `x·G = P` check.
pub(in crate::ledger) fn recover_with_base(
    output: &TxOutput,
    context: &StoredContext,
    base: impl FnOnce(&TxOutput, u32) -> Option<(u64, RistrettoPrivate)>,
) -> Result<(u64, RistrettoPrivate), clsag_v2::Error> {
    // The owner-reporting forms are the shared clsag helpers (#1667), the same
    // ones the wasm-signer core and the bridge use.
    match context {
        StoredContext::Direct { base_index, .. } => {
            clsag_v2::recover_direct_owner(output, *base_index, base)
        }
        StoredContext::Lottery { context, .. } => {
            clsag_v2::recover_payout_owner(output, context, base)
        }
    }
}

/// The native wallet's recovery of one persisted output: the unified hybrid
/// scan (#970) on the context's base output, then the context tweak.
pub(in crate::ledger) fn recover_owned(
    wallet: &Wallet,
    output: &TxOutput,
    context: &StoredContext,
) -> Result<(u64, RistrettoPrivate), clsag_v2::Error> {
    recover_with_base(output, context, |base, index| {
        let subaddress = wallet.scan_output(base, index)?;
        let key = wallet.recover_output_spend_key(base, subaddress, index)?;
        Some((subaddress, key))
    })
}

/// One output of the node wallet, discovered through its derivation context.
#[derive(Clone, Debug)]
pub(crate) struct WalletOutput {
    pub(crate) utxo: Utxo,
    /// Subaddress the base output pays (no production reader yet).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) subaddress: u64,
    pub(crate) key_image: [u8; 32],
    /// Height at which the key image was spent on chain, if it was.
    pub(crate) spent_at: Option<u64>,
    /// `true` for a V2 lottery payout (a `Lottery` derivation context).
    pub(crate) lottery_payout: bool,
}

impl WalletOutput {
    pub(crate) fn is_unspent(&self) -> bool {
        self.spent_at.is_none()
    }
}

impl ProductionView<'_, '_> {
    /// Classify one persisted outpoint for `wallet`. `Ok(None)` = not owned
    /// (or a legacy aliased row). Storage errors are errors.
    fn wallet_output(
        &self,
        wallet: &Wallet,
        id: &UtxoId,
    ) -> Result<Option<WalletOutput>, LedgerError> {
        let (utxo, context) = read_context(self.ledger, self.txn, id)?;
        let (subaddress, secret) = match recover_owned(wallet, &utxo.output, &context) {
            Ok(found) => found,
            Err(clsag_v2::Error::Ownership) => return Ok(None),
            // A rejected persisted context is store corruption, not a foreign
            // output: surface it (like a missing context) instead of letting
            // an owned output silently drop out of balance (#1667).
            Err(
                error @ (clsag_v2::Error::ZeroTweak
                | clsag_v2::Error::NonCanonicalTweak
                | clsag_v2::Error::Version),
            ) => {
                return Err(LedgerError::InconsistentRecord(format!(
                    "derivation context of {} rejected by recovery: {error:?}",
                    hex::encode(id.to_bytes())
                )))
            }
            // A foreign output whose keys are not usable points/scalars is
            // simply not ours (the classical scan treated it the same way).
            Err(error) => {
                debug!(?error, "wallet scan: output keys not recoverable");
                return Ok(None);
            }
        };
        // Legacy aliased rows stay excluded: only the outpoint the target-key
        // index resolves to is independently spendable.
        match ValidationReads::get_utxo_by_target_key(self, &utxo.output.target_key)? {
            Some(canonical) if canonical.id == utxo.id => {}
            _ => return Ok(None),
        }
        let key_image = *KeyImage::from(&secret).as_bytes();
        let spent_at = ValidationReads::is_key_image_spent(self, &key_image)?;
        Ok(Some(WalletOutput {
            utxo,
            subaddress,
            key_image,
            spent_at,
            lottery_payout: matches!(context, StoredContext::Lottery { .. }),
        }))
    }
}

impl Ledger {
    /// Every output this wallet owns (spent or not; outputs are never deleted
    /// under ring signatures), from one pinned read transaction, including V2
    /// lottery payouts recovered through their persisted contexts.
    pub(crate) fn discover_wallet_outputs(
        &self,
        wallet: &Wallet,
    ) -> Result<Vec<WalletOutput>, LedgerError> {
        let txn = self.env.read_txn().map_err(db_op("wallet read txn"))?;
        let view = self.production_view(&txn);
        let mut owned = Vec::new();
        for row in self.utxo_db.iter(&txn).map_err(db_op("wallet scan"))? {
            let (key, _) = row.map_err(db_op("wallet scan row"))?;
            let id = UtxoId::from_bytes(key)
                .ok_or_else(|| LedgerError::StorageEncoding("wallet outpoint".into()))?;
            if let Some(output) = view.wallet_output(wallet, &id)? {
                owned.push(output);
            }
        }
        Ok(owned)
    }

    /// The wallet's unspent outputs, in discovery order.
    pub(crate) fn unspent_wallet_outputs(
        &self,
        wallet: &Wallet,
    ) -> Result<Vec<WalletOutput>, LedgerError> {
        let mut owned = self.discover_wallet_outputs(wallet)?;
        owned.retain(WalletOutput::is_unspent);
        Ok(owned)
    }

    /// The V2 lottery payouts `block` created for this wallet. Payout
    /// outpoints are `(block hash, 1 + ordinal)`; a block with no payouts is
    /// answered without a ledger read.
    pub(crate) fn wallet_payouts_in_block(
        &self,
        wallet: &Wallet,
        block: &Block,
    ) -> Result<Vec<WalletOutput>, LedgerError> {
        if block.lottery_outputs.is_empty() {
            return Ok(Vec::new());
        }
        let txn = self.env.read_txn().map_err(db_op("wallet read txn"))?;
        let view = self.production_view(&txn);
        let hash = block.hash();
        let mut owned = Vec::new();
        for ordinal in 0..block.lottery_outputs.len() as u32 {
            let id = UtxoId::new(hash, ordinal + 1);
            if let Some(output) = view.wallet_output(wallet, &id)? {
                owned.push(output);
            }
        }
        Ok(owned)
    }

    /// Recover the one-time key of a claimed wallet input from its persisted
    /// context, in one pinned read. The claim must equal the persisted output
    /// and must not be spent.
    pub(crate) fn recover_wallet_input(
        &self,
        wallet: &Wallet,
        claimed: &Utxo,
    ) -> anyhow::Result<RistrettoPrivate> {
        let txn = self.env.read_txn().map_err(db_op("wallet read txn"))?;
        let (actual, context) = read_context(self, &txn, &claimed.id)?;
        anyhow::ensure!(
            bincode::serialize(claimed)? == bincode::serialize(&actual)?,
            "wallet input {} differs from the persisted output",
            hex::encode(&claimed.id.tx_hash[0..8])
        );
        let (_, secret) = recover_owned(wallet, &actual.output, &context).map_err(|e| {
            anyhow::anyhow!(
                "UTXO {} does not belong to this wallet ({e:?})",
                hex::encode(&claimed.id.tx_hash[0..8])
            )
        })?;
        let key_image = *KeyImage::from(&secret).as_bytes();
        anyhow::ensure!(
            ValidationReads::is_key_image_spent(&self.production_view(&txn), &key_image)?.is_none(),
            "wallet input {} is already spent",
            hex::encode(&claimed.id.tx_hash[0..8])
        );
        Ok(secret)
    }
}

#[cfg(test)]
mod cli_tests;
#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod tests;
