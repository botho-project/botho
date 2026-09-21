//! Private native-wallet bridge to accepted local state. No RPC/cache encoding.
use super::*;
use crate::{
    decoy_selection::{DecoySelectionError, GammaDecoySelector, OutputCandidate},
    transaction::{Transaction, TxOutput},
    wallet::{Wallet, WalletRead},
};
use bth_crypto_keys::RistrettoPrivate;
use bth_crypto_ring_signature::KeyImage;
use bth_util_from_random::OsRng;
use std::collections::BTreeSet;

// Owned only inside this boundary; never deserialized or accepted as authority.
struct OwnedOutput {
    utxo: Utxo,
    context: StoredContext,
    subaddress: u64,
    key_image: [u8; 32],
    genesis: [u8; 32],
    checkpoint: [u8; 32],
}
struct WalletView<'a, 'env> {
    pinned: PinnedView<'a, 'env>,
    state: ChainState,
}
impl WalletView<'_, '_> {
    fn recover(
        &self,
        wallet: &Wallet,
        utxo: &Utxo,
        context: &StoredContext,
    ) -> Result<(u64, RistrettoPrivate), v2::Error> {
        let mut subaddress = None;
        let key = v2::recover_with_context(&utxo.output, &context.derivation(), |base, index| {
            let found = wallet.scan_output(base, index)?;
            subaddress = Some(found);
            wallet.recover_output_spend_key(base, found, index)
        })?;
        Ok((subaddress.ok_or(v2::Error::Ownership)?, key))
    }
    // Discovery includes immature owned/unspent outputs. Construction separately
    // requires ten confirmations; lottery eligibility remains canonical720.
    fn discover(&self, wallet: &Wallet) -> Result<Vec<OwnedOutput>, LedgerError> {
        let mut owned = Vec::new();
        let genesis = fixed(self.pinned.metadata(GENESIS)?)?;
        for row in self.pinned.store.utxos.iter(self.pinned.txn).map_err(db)? {
            let (id, _) = row.map_err(db)?;
            if id.len() != 36 {
                return Err(encoding("wallet outpoint width"));
            }
            let id = UtxoId::from_bytes(id).ok_or_else(|| encoding("wallet outpoint"))?;
            let (utxo, context) = self.pinned.output(&id)?;
            let (subaddress, secret) = match self.recover(wallet, &utxo, &context) {
                Ok(result) => result,
                Err(v2::Error::Ownership) => continue,
                Err(error) => return Err(crypto(error)),
            };
            let key_image: [u8; 32] = *KeyImage::from(&secret).as_bytes();
            if self.pinned.is_key_image_spent(&key_image)?.is_none() {
                owned.push(OwnedOutput {
                    utxo,
                    context,
                    subaddress,
                    key_image,
                    genesis,
                    checkpoint: self.state.tip_hash,
                });
            }
        }
        Ok(owned)
    }
}
impl WalletRead for WalletView<'_, '_> {
    fn recover_input(&self, wallet: &Wallet, claimed: &Utxo) -> anyhow::Result<RistrettoPrivate> {
        let (actual, context) = self.pinned.output(&claimed.id)?;
        anyhow::ensure!(
            bincode::serialize(claimed)? == bincode::serialize(&actual)?,
            "owned input changed since discovery"
        );
        anyhow::ensure!(
            self.state.height.saturating_sub(actual.created_at) >= 10,
            "input not mature"
        );
        let (_, secret) = self.recover(wallet, &actual, &context).map_err(crypto)?;
        let key_image: [u8; 32] = *KeyImage::from(&secret).as_bytes();
        anyhow::ensure!(
            self.pinned.is_key_image_spent(&key_image)?.is_none(),
            "owned input already spent"
        );
        Ok(secret)
    }
    fn decoys(
        &self,
        count: usize,
        exclude: &[[u8; 32]],
        real_age: u64,
        selector: &GammaDecoySelector,
        rng: &mut OsRng,
    ) -> Result<Vec<TxOutput>, LedgerError> {
        let mut candidates = Vec::new();
        for row in self.pinned.store.utxos.iter(self.pinned.txn).map_err(db)? {
            let (key, _) = row.map_err(db)?;
            if key.len() != 36 {
                return Err(encoding("decoy outpoint width"));
            }
            let id = UtxoId::from_bytes(key).ok_or_else(|| encoding("decoy outpoint"))?;
            let (u, _) = self.pinned.output(&id)?;
            if self.state.height.saturating_sub(u.created_at) >= 10
                && !exclude.contains(&u.output.target_key)
            {
                candidates.push(OutputCandidate::from_utxo(&u, self.state.height));
            }
        }
        let chosen = selector
            .select_decoys_for_input(&candidates, count, exclude, real_age, rng)
            .map_err(|e| match e {
                DecoySelectionError::InsufficientCandidates {
                    required,
                    available,
                } => LedgerError::InsufficientDecoys {
                    required,
                    available,
                },
                DecoySelectionError::InvalidDistribution => {
                    invalid("Invalid gamma distribution parameters")
                }
            })?;
        let keys: BTreeSet<_> = chosen.iter().map(|u| u.target_key).collect();
        if chosen.len() != count || keys.len() != count || keys.iter().any(|k| exclude.contains(k))
        {
            return Err(invalid("invalid selected membership"));
        }
        Ok(chosen)
    }
}
impl ValidatedStore {
    fn discover_owned(&self, wallet: &Wallet) -> Result<Vec<OwnedOutput>, LedgerError> {
        let txn = self.store.env.read_txn().map_err(db)?;
        let pinned = self.view(&txn);
        let state = pinned.state()?;
        WalletView { pinned, state }.discover(wallet)
    }
    fn wallet_transaction(
        &self,
        wallet: &Wallet,
        owned: &[OwnedOutput],
        outputs: Vec<TxOutput>,
        fee: u64,
    ) -> anyhow::Result<Transaction> {
        let txn = self.store.env.read_txn().map_err(db)?;
        let pinned = self.view(&txn);
        let state = pinned.state()?;
        let genesis: [u8; 32] = fixed(pinned.metadata(GENESIS)?)?;
        let mut ids = BTreeSet::new();
        for input in owned {
            anyhow::ensure!(
                input.genesis == genesis && input.checkpoint == state.tip_hash,
                "stale/wrong-chain discovery; rescan required"
            );
            anyhow::ensure!(
                ids.insert(input.utxo.id.to_bytes()),
                "duplicate owned input"
            );
            let (actual, context) = pinned.output(&input.utxo.id)?;
            anyhow::ensure!(
                context == input.context && actual.output == input.utxo.output,
                "owned context changed; rescan required"
            );
        }
        let height = state.height;
        let view = WalletView { pinned, state };
        let inputs: Vec<_> = owned.iter().map(|o| o.utxo.clone()).collect();
        wallet.create_private_transaction_impl(&inputs, outputs, fee, height, &view, None)
    }
}

#[cfg(test)]
mod tests;
