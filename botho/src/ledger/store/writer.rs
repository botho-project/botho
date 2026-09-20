//! Shared actual ledger effects, extracted without changing V1 validation or
//! encoding. This module is private to the ledger. Representation is NOT a
//! validation token.
use super::*;
use crate::ledger::experimental::StoredContext;
use bth_transaction_clsag::lottery_v2::Record;
#[derive(Clone, Copy)]
pub(in crate::ledger) struct WriteTables {
    pub(in crate::ledger) blocks_db: Database<U64<heed::byteorder::LE>, Bytes>,
    pub(in crate::ledger) meta_db: Database<Bytes, Bytes>,
    pub(in crate::ledger) utxo_db: Database<Bytes, Bytes>,
    pub(in crate::ledger) address_index_db: Database<Bytes, Bytes>,
    pub(in crate::ledger) key_images_db: Database<Bytes, Bytes>,
    pub(in crate::ledger) tx_index_db: Database<Bytes, Bytes>,
    pub(in crate::ledger) cluster_wealth_db: Database<Bytes, Bytes>,
    pub(in crate::ledger) bridge_import_clusters_db: Database<Bytes, Bytes>,
}

#[derive(Clone, Copy)]
pub(in crate::ledger) enum Representation<'a> {
    Legacy,
    Experimental {
        contexts: Database<Bytes, Bytes>,
        records: &'a [Record],
    },
}
impl Representation<'_> {
    fn check_new(self, tables: &WriteTables, txn: &RwTxn, id: &UtxoId) -> Result<(), LedgerError> {
        if let Self::Experimental { contexts, .. } = self {
            if tables
                .utxo_db
                .get(txn, &id.to_bytes())
                .map_err(|e| LedgerError::Database(e.to_string()))?
                .is_some()
                || contexts
                    .get(txn, &id.to_bytes())
                    .map_err(|e| LedgerError::Database(e.to_string()))?
                    .is_some()
            {
                return Err(LedgerError::InconsistentRecord("existing outpoint".into()));
            }
        }
        Ok(())
    }
    fn write_context(
        self,
        txn: &mut RwTxn,
        u: &Utxo,
        ordinal: Option<u32>,
        after: &mut dyn FnMut() -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        if let Self::Experimental { contexts, records } = self {
            let c = match ordinal {
                None => StoredContext::Direct {
                    height: u.created_at,
                    base_index: u.id.output_index,
                },
                Some(ordinal) => StoredContext::Lottery {
                    height: u.created_at,
                    ordinal,
                    context: records
                        .get(ordinal as usize)
                        .ok_or_else(|| {
                            LedgerError::InconsistentRecord("missing payout record".into())
                        })?
                        .context
                        .clone(),
                },
            };
            contexts
                .put(txn, &u.id.to_bytes(), &c.encode()?)
                .map_err(|e| LedgerError::Database(e.to_string()))?;
            after()?;
        }
        Ok(())
    }
}
impl WriteTables {
    #[cfg(test)]
    pub(in crate::ledger) fn initialize_fixture_metadata(
        meta: Database<Bytes, Bytes>,
        txn: &mut RwTxn,
    ) -> Result<(), LedgerError> {
        let state = ChainState::default();
        for (key, value) in [
            (META_HEIGHT, state.height),
            (META_DIFFICULTY, state.difficulty),
            (META_TOTAL_TX, state.total_tx),
            (META_EPOCH_TX, state.epoch_tx),
            (META_EPOCH_EMISSION, state.epoch_emission),
            (META_EPOCH_BURNS, state.epoch_burns),
            (META_CURRENT_REWARD, state.current_reward),
        ] {
            meta.put(txn, key, &value.to_le_bytes())
                .map_err(|e| LedgerError::Database(e.to_string()))?;
        }
        for key in [META_TOTAL_MINED, META_FEES_BURNED, META_LOTTERY_POOL] {
            meta.put(txn, key, &0u128.to_le_bytes())
                .map_err(|e| LedgerError::Database(e.to_string()))?;
        }
        meta.put(txn, META_TIP_HASH, &state.tip_hash)
            .map_err(|e| LedgerError::Database(e.to_string()))
    }
    // Minimal accounting view needed by the shared effects. Full experimental
    // consensus-state/candidate validation is a subsequent integration child.
    pub(in crate::ledger) fn fixture_accounting_state(
        &self,
        txn: &heed::RoTxn<'_>,
    ) -> Result<ChainState, LedgerError> {
        let read_u128 = |key| -> Result<u128, LedgerError> {
            let bytes = self
                .meta_db
                .get(txn, key)
                .map_err(|e| LedgerError::Database(e.to_string()))?
                .ok_or_else(|| {
                    LedgerError::InconsistentRecord("missing accounting metadata".into())
                })?;
            Ok(u128::from_le_bytes(bytes.try_into().map_err(|_| {
                LedgerError::StorageEncoding("accounting width".into())
            })?))
        };
        Ok(ChainState {
            total_mined: read_u128(META_TOTAL_MINED)?,
            total_fees_burned: read_u128(META_FEES_BURNED)?,
            ..ChainState::default()
        })
    }
    // All arguments are explicit to preserve the existing transaction ownership,
    // validation callback placement and optional emission behavior.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::ledger) fn block_effects(
        &self,
        wtxn: &mut RwTxn,
        block: &Block,
        state: &ChainState,
        new_lottery_pool: u128,
        emission: Option<EmissionStateUpdate>,
        block_bytes: &[u8],
        representation: Representation<'_>,
        verify_transaction: &mut dyn FnMut(&BothoTransaction) -> Result<(), LedgerError>,
        after_write: &mut dyn FnMut() -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        self.blocks_db
            .put(wtxn, &block.height(), block_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to put block: {}", e)))?;
        after_write()?;

        let new_hash = block.hash();
        let new_height = block.height();
        let new_total_mined = state.total_mined + block.minting_tx.reward as u128;

        // Fee accounting. Only the burn share of fees is actually destroyed;
        // the remainder flows to the redistribution lottery pool (and is paid
        // back out as lottery UTXOs). `total_fees_burned` therefore tracks the
        // validated burn amount, NOT the gross fee total — counting the full
        // fee would overstate destroyed supply 5x and break conservation
        // (audit cycle 6, M4). The lottery summary's burn amount was verified
        // against pool accounting by the V1 caller before this writer.
        // Experimental fixtures explicitly supply unvalidated accounting.
        //
        // The V1 caller already ran its checked fee-sum guard.
        let actually_burned = block.lottery_summary.amount_burned;
        let new_total_fees_burned = state.total_fees_burned + actually_burned as u128;

        // Create UTXO from minting reward (coinbase)
        let coinbase_utxo_id = UtxoId::new(new_hash, 0);
        let coinbase_utxo = Utxo {
            id: coinbase_utxo_id,
            output: block.minting_tx.to_tx_output(),
            created_at: new_height,
        };
        representation.check_new(self, wtxn, &coinbase_utxo.id)?;
        let coinbase_bytes = bincode::serialize(&coinbase_utxo)
            .map_err(|e| LedgerError::Serialization(e.to_string()))?;
        self.utxo_db
            .put(wtxn, &coinbase_utxo_id.to_bytes(), &coinbase_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to put coinbase utxo: {}", e)))?;
        after_write()?;
        // Add to address index
        representation.write_context(wtxn, &coinbase_utxo, None, after_write)?;
        self.add_to_address_index(wtxn, &coinbase_utxo, after_write)?;
        // Update cluster wealth tracking
        self.update_cluster_wealth_for_output(wtxn, &coinbase_utxo.output, after_write)?;
        debug!("Created coinbase UTXO at height {}", new_height);

        // Verify and process regular transactions
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            // Verify transaction signatures before processing
            verify_transaction(tx)?;

            let tx_hash = tx.hash();

            // Index transaction for fast lookups (exchange integration)
            self.add_tx_to_index(wtxn, &tx_hash, new_height, tx_idx as u32, after_write)?;

            // Process spent inputs - record key images to prevent double-spend
            for input in tx.inputs.clsag() {
                self.record_key_image(wtxn, &input.key_image, new_height, after_write)?;
            }

            // Add new UTXOs (outputs)
            for (idx, output) in tx.outputs.iter().enumerate() {
                let utxo_id = UtxoId::new(tx_hash, idx as u32);
                let utxo = Utxo {
                    id: utxo_id,
                    output: output.clone(),
                    created_at: new_height,
                };
                representation.check_new(self, wtxn, &utxo.id)?;
                let utxo_bytes = bincode::serialize(&utxo)
                    .map_err(|e| LedgerError::Serialization(e.to_string()))?;
                self.utxo_db
                    .put(wtxn, &utxo_id.to_bytes(), &utxo_bytes)
                    .map_err(|e| LedgerError::Database(format!("Failed to put utxo: {}", e)))?;
                after_write()?;
                // Add to address index
                representation.write_context(wtxn, &utxo, None, after_write)?;
                self.add_to_address_index(wtxn, &utxo, after_write)?;
                // Update cluster wealth tracking
                self.update_cluster_wealth_for_output(wtxn, output, after_write)?;
                // ADR 0007 (#938): if this output carries this-epoch's
                // bridge-import tag (an unwrap's minted output), record the
                // import cluster so the ≥F floor is enforceable at spend time.
                self.record_bridge_import_clusters_for_output(
                    wtxn,
                    output,
                    new_height,
                    after_write,
                )?;
            }
        }

        // Persist lottery payout records. Legacy callers performed binding and
        // draw validation before entering this writer; experimental callers are
        // storage fixtures only. Persistence does not establish spendability.
        // Coinbase uses block-hash index 0; payouts use index 1 + ordinal.
        for (lottery_idx, lottery_output) in block.lottery_outputs.iter().enumerate() {
            let winner_id = lottery_output.winner_utxo_id();
            let winner_bytes = self
                .utxo_db
                .get(wtxn, &winner_id)
                .map_err(|e| LedgerError::Database(format!("Failed to read winning utxo: {}", e)))?
                .ok_or_else(|| {
                    LedgerError::InvalidBlock(format!(
                        "Lottery winner UTXO {} not found in set",
                        hex::encode(&winner_id[..8])
                    ))
                })?;
            let winner_utxo: Utxo = bincode::deserialize(winner_bytes)
                .map_err(|e| LedgerError::Serialization(e.to_string()))?;

            let payout_output = TxOutput {
                amount: lottery_output.payout,
                target_key: winner_utxo.output.target_key,
                public_key: winner_utxo.output.public_key,
                e_memo: None,
                cluster_tags: winner_utxo.output.cluster_tags.clone(),
                // Preserve the existing V1 envelope bytes. The separate #1286
                // derivation-index/independent-spend repair remains outstanding.
                kem_ciphertext: winner_utxo.output.kem_ciphertext.clone(),
            };

            let payout_output = match representation {
                Representation::Legacy => payout_output,
                Representation::Experimental { .. } => {
                    lottery_output.to_tx_output(winner_utxo.output.cluster_tags.clone())
                }
            };

            let payout_utxo_id = UtxoId::new(new_hash, (lottery_idx as u32) + 1);
            let payout_utxo = Utxo {
                id: payout_utxo_id,
                output: payout_output,
                created_at: new_height,
            };
            representation.check_new(self, wtxn, &payout_utxo.id)?;
            let payout_bytes = bincode::serialize(&payout_utxo)
                .map_err(|e| LedgerError::Serialization(e.to_string()))?;
            self.utxo_db
                .put(wtxn, &payout_utxo_id.to_bytes(), &payout_bytes)
                .map_err(|e| {
                    LedgerError::Database(format!("Failed to put lottery payout utxo: {}", e))
                })?;
            after_write()?;
            representation.write_context(
                wtxn,
                &payout_utxo,
                Some(lottery_idx as u32),
                after_write,
            )?;
            self.add_to_address_index(wtxn, &payout_utxo, after_write)?;
            self.update_cluster_wealth_for_output(wtxn, &payout_utxo.output, after_write)?;
        }

        self.meta_db
            .put(wtxn, META_HEIGHT, &new_height.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put height: {}", e)))?;
        after_write()?;
        self.meta_db
            .put(wtxn, META_TIP_HASH, &new_hash)
            .map_err(|e| LedgerError::Database(format!("Failed to put tip_hash: {}", e)))?;
        after_write()?;
        self.meta_db
            .put(wtxn, META_TOTAL_MINED, &new_total_mined.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put total_mined: {}", e)))?;
        after_write()?;
        self.meta_db
            .put(wtxn, META_FEES_BURNED, &new_total_fees_burned.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put fees_burned: {}", e)))?;
        after_write()?;
        self.meta_db
            .put(wtxn, META_LOTTERY_POOL, &new_lottery_pool.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put lottery_pool: {}", e)))?;
        after_write()?;

        // H3 (#558): fold the emission-controller state into the SAME write txn
        // as the block. These are the difficulty/reward/epoch counters that are
        // a pure function of the applied block; writing them here (rather than
        // in a separate `update_emission_state` commit) makes block + emission
        // state crash-atomic. Mirrors `update_emission_state` exactly — same
        // keys, same encoding — so a no-crash node persists identical values.
        if let Some(e) = emission {
            self.meta_db
                .put(wtxn, META_DIFFICULTY, &e.difficulty.to_le_bytes())
                .map_err(|err| {
                    LedgerError::Database(format!("Failed to put difficulty: {}", err))
                })?;
            after_write()?;
            self.meta_db
                .put(wtxn, META_TOTAL_TX, &e.total_tx.to_le_bytes())
                .map_err(|err| LedgerError::Database(format!("Failed to put total_tx: {}", err)))?;
            after_write()?;
            self.meta_db
                .put(wtxn, META_EPOCH_TX, &e.epoch_tx.to_le_bytes())
                .map_err(|err| LedgerError::Database(format!("Failed to put epoch_tx: {}", err)))?;
            after_write()?;
            self.meta_db
                .put(wtxn, META_EPOCH_EMISSION, &e.epoch_emission.to_le_bytes())
                .map_err(|err| {
                    LedgerError::Database(format!("Failed to put epoch_emission: {}", err))
                })?;
            after_write()?;
            self.meta_db
                .put(wtxn, META_EPOCH_BURNS, &e.epoch_burns.to_le_bytes())
                .map_err(|err| {
                    LedgerError::Database(format!("Failed to put epoch_burns: {}", err))
                })?;
            after_write()?;
            self.meta_db
                .put(wtxn, META_CURRENT_REWARD, &e.current_reward.to_le_bytes())
                .map_err(|err| {
                    LedgerError::Database(format!("Failed to put current_reward: {}", err))
                })?;
            after_write()?;
        }

        Ok(())
    }
    pub(in crate::ledger) fn add_to_address_index(
        &self,
        wtxn: &mut RwTxn,
        utxo: &Utxo,
        after_write: &mut dyn FnMut() -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        // Index by target_key for UTXO retrieval after stealth detection
        let target_key = &utxo.output.target_key;

        // Get existing IDs or empty vec
        let existing = match self.address_index_db.get(wtxn, target_key.as_slice()) {
            Ok(Some(bytes)) => bytes.to_vec(),
            Ok(None) => Vec::new(),
            Err(e) => {
                return Err(LedgerError::Database(format!(
                    "Failed to get address index: {}",
                    e
                )))
            }
        };

        // Append the new UTXO ID
        let mut ids = existing;
        ids.extend_from_slice(&utxo.id.to_bytes());

        self.address_index_db
            .put(wtxn, target_key.as_slice(), &ids)
            .map_err(|e| LedgerError::Database(format!("Failed to put address index: {}", e)))?;
        after_write()?;

        Ok(())
    }
    pub(in crate::ledger) fn record_key_image(
        &self,
        wtxn: &mut RwTxn,
        key_image: &[u8; 32],
        height: u64,
        after_write: &mut dyn FnMut() -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        // Check if already exists.
        //
        // As with `verify_transaction`, distinguish a DB failure (node-local,
        // propagate via `?`) from an actual collision (consensus-invalid). The
        // previous `if let Ok(Some(..))` swallowed a DB `Err` and fell through to
        // the `put` below, which would record the key image and let a
        // double-spend through on a transient read error (fail-open, M7).
        let existing = self
            .key_images_db
            .get(wtxn, key_image.as_slice())
            .map_err(|e| LedgerError::Database(format!("Failed to get key image: {}", e)))?;
        if let Some(existing_height_bytes) = existing {
            let existing_height =
                u64::from_le_bytes(existing_height_bytes.try_into().unwrap_or([0u8; 8]));
            warn!(
                "Key image collision: {} already spent at height {}, trying to spend at height {}",
                hex::encode(&key_image[0..8]),
                existing_height,
                height
            );
            return Err(LedgerError::InvalidBlock(
                "Key image already spent (double-spend)".to_string(),
            ));
        }

        self.key_images_db
            .put(wtxn, key_image.as_slice(), &height.to_le_bytes())
            .map_err(|e| LedgerError::Database(format!("Failed to put key image: {}", e)))?;
        after_write()
    }
    pub(in crate::ledger) fn add_tx_to_index(
        &self,
        wtxn: &mut RwTxn,
        tx_hash: &[u8; 32],
        block_height: u64,
        tx_index: u32,
        after_write: &mut dyn FnMut() -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        // Encode location as 12 bytes: height (8) + tx_index (4)
        let mut location_bytes = [0u8; 12];
        location_bytes[0..8].copy_from_slice(&block_height.to_le_bytes());
        location_bytes[8..12].copy_from_slice(&tx_index.to_le_bytes());

        self.tx_index_db
            .put(wtxn, tx_hash.as_slice(), &location_bytes)
            .map_err(|e| LedgerError::Database(format!("Failed to index transaction: {}", e)))?;
        after_write()
    }
    pub(in crate::ledger) fn update_cluster_wealth_for_output(
        &self,
        wtxn: &mut RwTxn,
        output: &TxOutput,
        after_write: &mut dyn FnMut() -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        for entry in &output.cluster_tags.entries {
            // Contribution = output_amount × tag_weight / TAG_WEIGHT_SCALE.
            // Kept in full u128 (no down-cast): the accumulator is now u128 so
            // cumulative wealth can exceed the former u64::MAX pico ceiling.
            let contribution =
                (output.amount as u128) * (entry.weight as u128) / (TAG_WEIGHT_SCALE as u128);

            if contribution > 0 {
                let cluster_key = entry.cluster_id.0.to_le_bytes();

                // Get current wealth (16-byte LE u128, reject-legacy).
                let current = match self
                    .cluster_wealth_db
                    .get(wtxn, cluster_key.as_slice())
                    .map_err(|e| {
                        LedgerError::Database(format!("Failed to get cluster wealth: {}", e))
                    })? {
                    Some(bytes) => decode_cluster_wealth(bytes)?,
                    None => 0u128,
                };

                // Add contribution. saturating_add on u128 keeps byte-identical
                // semantics with the rebuild path (`rebuild_cluster_wealth_index`);
                // saturation at u128::MAX is astronomically unreachable but the
                // discipline is preserved (M3 lesson, #604/#607).
                let new_wealth = current.saturating_add(contribution);
                self.cluster_wealth_db
                    .put(wtxn, cluster_key.as_slice(), &new_wealth.to_le_bytes())
                    .map_err(|e| {
                        LedgerError::Database(format!("Failed to update cluster wealth: {}", e))
                    })?;
                after_write()?;
            }
        }
        Ok(())
    }
    pub(in crate::ledger) fn record_bridge_import_clusters_for_output(
        &self,
        wtxn: &mut RwTxn,
        output: &TxOutput,
        height: u64,
        after_write: &mut dyn FnMut() -> Result<(), LedgerError>,
    ) -> Result<(), LedgerError> {
        let epoch_import_id = bth_cluster_tax::import_cluster_id_for_height(height).0;
        for entry in &output.cluster_tags.entries {
            if entry.cluster_id.0 == epoch_import_id {
                let key = epoch_import_id.to_le_bytes();
                self.bridge_import_clusters_db
                    .put(wtxn, key.as_slice(), &[])
                    .map_err(|e| {
                        LedgerError::Database(format!(
                            "Failed to record bridge-import cluster: {}",
                            e
                        ))
                    })?;
                after_write()?;
            }
        }
        Ok(())
    }
}
