//! Shared ordinary validation and lottery candidate algorithm. V1 adapters
//! retain their existing reads/errors; experimental adapters pin a strict
//! pre-state.
use super::{writer::WriteTables, *};
#[derive(Clone, Copy)]
pub(in crate::ledger) enum RootRule {
    Legacy,
    Experimental([u8; 32]),
}
#[derive(Clone, Copy)]
pub(in crate::ledger) enum ReadPolicy {
    Legacy,
    Strict,
}

pub(in crate::ledger) trait ValidationReads {
    fn get_utxo_by_target_key(&self, key: &[u8; 32]) -> Result<Option<Utxo>, LedgerError>;
    fn get_cluster_wealth(&self, id: u64) -> Result<u128, LedgerError>;
    fn is_bridge_import_cluster(&self, id: u64) -> Result<bool, LedgerError>;
    fn is_key_image_spent(&self, key: &[u8; 32]) -> Result<Option<u64>, LedgerError>;
    fn verify_transaction(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        // Verify key images haven't been spent (double-spend check).
        //
        // A DB error here is a node-local operational failure, NOT a statement
        // that the transaction is invalid. We distinguish two outcomes and fail
        // CLOSED on the error path (audit cycle 6, M7):
        //   - lookup succeeds and finds the key image spent -> reject as invalid
        //     (`LedgerError::InvalidBlock`, the existing/correct verdict)
        //   - lookup returns Err (DB failure) -> propagate the DB error up via `?` so
        //     the caller aborts/retries. Previously this used `if let Ok(Some(..))`,
        //     which silently treated a DB error as "not spent" and let a double-spend
        //     pass (fail-open). Propagating with `?` preserves the happy-path verdict
        //     exactly while closing the fail-open hole — a DB error never gets branded
        //     as block-invalid.
        for (i, input) in tx.inputs.clsag().iter().enumerate() {
            if let Some(spent_height) = self.is_key_image_spent(&input.key_image)? {
                return Err(LedgerError::InvalidBlock(format!(
                    "Input {} uses key image already spent at height {}",
                    i, spent_height
                )));
            }
        }

        // Verify CLSAG ring signatures
        tx.verify_ring_signatures()
            .map_err(|e| LedgerError::InvalidBlock(format!("Invalid ring signature: {}", e)))?;

        Ok(())
    }
    fn verify_ring_members(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        for (input_idx, input) in tx.inputs.clsag().iter().enumerate() {
            // Track whether the claimed per-input pseudo-output amount matches
            // any resolved ring member's real UTXO amount. The real input is
            // hidden among decoys, so we cannot identify it directly — but the
            // CLSAG balance proof asserts the real member's committed amount
            // equals `pseudo_output_amount`, and every ring member is verified
            // below to match a real UTXO. Requiring the pseudo-output amount to
            // equal some ring member's UTXO amount therefore binds it to a real
            // UTXO and prevents a producer claiming an inflated input amount to
            // unbalance the transaction-level sum (audit finding I4; composes
            // with the C3 commitment check below).
            let mut pseudo_amount_bound = false;

            for (member_idx, member) in input.ring.iter().enumerate() {
                let utxo = self
                    .get_utxo_by_target_key(&member.target_key)?
                    .ok_or_else(|| {
                        LedgerError::InvalidBlock(format!(
                            "Input {} ring member {} target_key not in UTXO set",
                            input_idx, member_idx
                        ))
                    })?;
                let expected = RingMember::from_output(&utxo.output);
                if expected != *member {
                    return Err(LedgerError::InvalidBlock(format!(
                        "Input {} ring member {} does not match UTXO (target_key/public_key/commitment mismatch — possible counterfeit amount)",
                        input_idx, member_idx
                    )));
                }
                if utxo.output.amount == input.pseudo_output_amount {
                    pseudo_amount_bound = true;
                }
            }

            if !pseudo_amount_bound {
                return Err(LedgerError::InvalidBlock(format!(
                    "Input {} pseudo-output amount {} does not match any resolved ring member's UTXO amount (possible counterfeit input amount)",
                    input_idx, input.pseudo_output_amount
                )));
            }
        }
        Ok(())
    }
    fn verify_cluster_tag_inheritance(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        let mut input_rings: Vec<Vec<(ClusterTagVector, u64)>> = Vec::new();
        for input in tx.inputs.clsag() {
            let mut ring: Vec<(ClusterTagVector, u64)> = Vec::new();
            for member in &input.ring {
                if let Some(utxo) = self.get_utxo_by_target_key(&member.target_key)? {
                    ring.push((utxo.output.cluster_tags.clone(), utxo.output.amount));
                }
            }
            input_rings.push(ring);
        }
        check_cluster_tag_inheritance(&input_rings, &tx.outputs)
    }
    fn verify_settlement(&self, tx: &BothoTransaction) -> Result<(), LedgerError> {
        let Some(settlement) = &tx.settlement else {
            return Ok(());
        };

        // Tag-rewrite rule: a settlement produces ONLY background value.
        for (i, out) in tx.outputs.iter().enumerate() {
            if !out.cluster_tags.is_empty() {
                return Err(LedgerError::InvalidBlock(format!(
                    "settlement output {} is not background (carries {} cluster tag(s)); \
                     a settlement must reclassify all value to factor-1",
                    i,
                    out.cluster_tags.len()
                )));
            }
        }

        // Certified value must equal the summed (all-background) outputs.
        let output_sum = tx
            .outputs
            .iter()
            .fold(0u64, |acc, o| acc.saturating_add(o.amount));
        if settlement.settled_value != output_sum {
            return Err(LedgerError::InvalidBlock(format!(
                "settlement settled_value {} does not equal output sum {}",
                settlement.settled_value, output_sum
            )));
        }

        Ok(())
    }
    fn verify_consensus_fee_floor(
        &self,
        tx: &BothoTransaction,
        block_height: u64,
    ) -> Result<(), LedgerError> {
        let floor = self.consensus_fee_floor(tx, block_height)?;
        if tx.fee < floor {
            return Err(LedgerError::InvalidBlock(format!(
                "transfer tx fee {} is below the consensus fee floor {} (height {})",
                tx.fee, floor, block_height
            )));
        }
        Ok(())
    }
    fn consensus_fee_floor(
        &self,
        tx: &BothoTransaction,
        block_height: u64,
    ) -> Result<u64, LedgerError> {
        use bth_cluster_tax::{FeeConfig, TransactionType as FeeTransactionType};

        // Consensus fee config: the canonical, fixed FeeConfig. This is a pure
        // constant (no node-local state), so every node computes the identical
        // curve. It matches the mempool's `FeeConfig::default()`.
        let fee_config = FeeConfig::default();

        // Base minimum fee (size + cluster factor + output penalty + memos),
        // congestion-free: the dynamic base is pinned to CONSENSUS_FEE_BASE.
        let tx_size_bytes = tx.estimate_size();
        let num_outputs = tx.outputs.len();
        let num_memos = tx.outputs.iter().filter(|o| o.has_memo()).count();

        // effective cluster wealth from the tx's OUTPUT tags weighted against
        // committed global per-cluster wealth (fail-closed on DB error, M7).
        let cluster_wealth = self.effective_cluster_wealth_from_outputs(&tx.outputs)?;

        let base_minimum_fee = fee_config.minimum_fee_dynamic_with_outputs(
            FeeTransactionType::Hidden,
            tx_size_bytes,
            cluster_wealth,
            num_outputs,
            num_memos,
            CONSENSUS_FEE_BASE,
        );

        // Demurrage charge. Resolve the ring members ONCE from the committed
        // UTXO set: we need (value, created_at) for the age quantile and
        // (value, &cluster_tags) for the factor floor. Fail-closed on DB error.
        let output_sum: u64 = tx
            .outputs
            .iter()
            .fold(0u64, |acc, o| acc.saturating_add(o.amount));

        let mut ring_age_members: Vec<(u64, u64)> = Vec::new();
        // Owns the resolved cluster-tag vectors so we can hand out borrows.
        let mut ring_tag_owned: Vec<(u64, ClusterTagVector)> = Vec::new();
        for input in tx.inputs.clsag() {
            for member in &input.ring {
                if let Some(utxo) = self.get_utxo_by_target_key(&member.target_key)? {
                    ring_age_members.push((utxo.output.amount, utxo.created_at));
                    ring_tag_owned.push((utxo.output.amount, utxo.output.cluster_tags.clone()));
                }
            }
        }

        let policy = crate::monetary::mainnet_policy();
        let blocks_per_year = (365 * 24 * 60 * 60) / policy.target_block_time_secs.max(1);

        // H2/B1: max-quantile age (value-independent order statistic) instead of
        // the value-weighted mean centroid — fresh decoys can no longer drag
        // the demurrage clock to zero.
        let elapsed = bth_cluster_tax::ring_elapsed_quantile(
            &ring_age_members,
            block_height,
            CONSENSUS_RING_AGE_QUANTILE_BPS,
        );

        // B2: floor the spender-claimed factor at the ring-centroid-implied
        // factor so background-tagged outputs cannot escape a wealthy ring.
        let claimed_factor = fee_config.cluster_factor(cluster_wealth);
        let ring_tag_refs: Vec<(u64, &ClusterTagVector)> = ring_tag_owned
            .iter()
            .map(|(value, tags)| (*value, tags))
            .collect();
        let demurrage_factor = self.ring_centroid_floored_factor(
            claimed_factor,
            &ring_tag_refs,
            &fee_config.cluster_curve,
        )?;

        // ADR 0007 (#938): the bridge-import floor. Imported wealth (tagged to a
        // bridge-import cluster) is priced at ≥ F on its import-tagged fraction,
        // so an unwrapped coin cannot be spent at background factor-1 until it
        // circulates the tag off. This is a SEPARATE lower bound from the
        // ring-centroid floor above: both are `max` against the demurrage
        // factor, so composing them is a single dominating `max` — no
        // double-floor. It only ever RAISES the factor, and only for value that
        // actually traces to a recorded import cluster.
        let import_floor = self.import_floor_factor_from_outputs(&tx.outputs)?;
        let demurrage_factor = demurrage_factor.max(import_floor);

        // #925 (background-reset leak, #834): total demurrage is
        // `max(accrued_to_date, capitalized_reset_charge)`. The capitalized term
        // prices a genuine class DOWNGRADE — the spender-declared output factor
        // `claimed_factor` dropping below the composed input-class floor
        // `demurrage_factor` (ring-centroid B2 + ADR-0007 import floor) — at
        // capitalized future demurrage over the shared
        // `SETTLEMENT_HORIZON_BLOCKS`, mirroring #831's settlement charge. It
        // fires ONLY on a downgrade (`claimed_factor < demurrage_factor`); an
        // in-class or background→background spend has `claimed_factor ==
        // demurrage_factor` ⇒ zero capitalized, so honest holds pay only their
        // accrued-to-date demurrage exactly as before. The young-coin exploit
        // (elapsed ≈ 0 ⇒ accrued ≈ 0) is now caught by the capitalized term.
        // Pure integer math; only ever RAISES the floor (liveness-safe).
        let demurrage = bth_cluster_tax::spend_demurrage_charge(
            output_sum,
            demurrage_factor,
            claimed_factor,
            elapsed,
            policy.demurrage_rate_bps(block_height),
            blocks_per_year,
        );

        Ok(base_minimum_fee.saturating_add(demurrage))
    }
    fn effective_cluster_wealth_from_outputs(
        &self,
        outputs: &[TxOutput],
    ) -> Result<u128, LedgerError> {
        let mut total_weighted_wealth: u128 = 0;
        let mut total_value: u128 = 0;

        for output in outputs {
            total_value = total_value.saturating_add(output.amount as u128);
            for entry in &output.cluster_tags.entries {
                let value_fraction =
                    (output.amount as u128 * entry.weight as u128) / (TAG_WEIGHT_SCALE as u128);
                // `get_cluster_wealth` is full-u128 (16-byte accumulator, #626
                // PR2). The consensus fee floor now consumes u128 wealth
                // end-to-end (#626 PR3): the prior `as u64` truncation of the
                // centroid is gone, so `minimum_fee_dynamic_with_outputs` and
                // `cluster_factor` see the exact value. Saturating math keeps
                // the (astronomically-distant) u128 overflow deterministic —
                // pinning to u128::MAX → factor 6000 (max floor), the
                // conservative consensus direction; every node computes the
                // identical result, so no fork.
                let global_wealth = self.get_cluster_wealth(entry.cluster_id.0)?;
                total_weighted_wealth = total_weighted_wealth
                    .saturating_add(value_fraction.saturating_mul(global_wealth));
            }
        }

        if total_value == 0 {
            return Ok(0);
        }

        Ok(total_weighted_wealth / total_value)
    }
    fn import_floor_factor_from_outputs(&self, outputs: &[TxOutput]) -> Result<u64, LedgerError> {
        use bth_cluster_tax::{ClusterFactorCurve, BRIDGE_IMPORT_FACTOR_FLOOR};

        let background = ClusterFactorCurve::FACTOR_SCALE as u128; // 1× in FACTOR_SCALE
        let floor = BRIDGE_IMPORT_FACTOR_FLOOR as u128; // F in FACTOR_SCALE

        let mut total_value: u128 = 0;
        let mut import_value: u128 = 0;

        for output in outputs {
            let amount = output.amount as u128;
            total_value = total_value.saturating_add(amount);
            for entry in &output.cluster_tags.entries {
                if self.is_bridge_import_cluster(entry.cluster_id.0)? {
                    let tagged =
                        amount.saturating_mul(entry.weight as u128) / (TAG_WEIGHT_SCALE as u128);
                    import_value = import_value.saturating_add(tagged);
                }
            }
        }

        if total_value == 0 {
            return Ok(ClusterFactorCurve::FACTOR_SCALE);
        }
        // Clamp defensively: a malformed tag set could push import_value past
        // total_value; the blend must never exceed F.
        let import_value = import_value.min(total_value);
        let background_value = total_value - import_value;

        // Value-weighted blend of F (import fraction) and 1× (rest).
        let blended = (floor
            .saturating_mul(import_value)
            .saturating_add(background.saturating_mul(background_value)))
            / total_value;
        Ok(blended as u64)
    }
    fn ring_centroid_floored_factor(
        &self,
        claimed_factor: u64,
        ring_members: &[(u64, &ClusterTagVector)],
        curve: &bth_cluster_tax::ClusterFactorCurve,
    ) -> Result<u64, LedgerError> {
        // Resolve each ring member's value-normalized effective cluster wealth
        // (Σ_tag weight × W_global / TAG_WEIGHT_SCALE), fail-closed.
        let mut members: Vec<(u64, u128)> = Vec::with_capacity(ring_members.len());
        for (value, tags) in ring_members {
            let mut member_wealth: u128 = 0;
            for entry in &tags.entries {
                // `get_cluster_wealth` is full-u128 (16-byte LE accumulator).
                let global_wealth = self.get_cluster_wealth(entry.cluster_id.0)?;
                member_wealth = member_wealth.saturating_add(
                    (entry.weight as u128).saturating_mul(global_wealth) / TAG_WEIGHT_SCALE as u128,
                );
            }
            // `ring_centroid_implied_factor` now takes full-u128 wealth (#626
            // PR3) — the prior `.min(u64::MAX)` clamp is gone, so the consensus
            // fee floor sees the exact per-member cumulative wealth.
            //
            // Overflow is saturated at each multiply on this consensus path:
            //   - here, `weight × global_wealth` saturates to u128::MAX before the divide
            //     (global_wealth is the unbounded, monotonic PR2 accumulator, so the
            //     product is not provably < u128::MAX);
            //   - inside `ring_centroid_implied_factor`, the `value × member_wealth` and
            //     `total_value` products likewise saturate.
            // A saturated wealth maps deterministically to factor 6000 (the max,
            // most-conservative fee floor) — identical on every node → no fork.
            members.push((*value, member_wealth));
        }

        let implied = bth_cluster_tax::ring_centroid_implied_factor(&members, curve);
        Ok(claimed_factor.max(implied))
    }
    fn validate_ordinary_block(
        &self,
        block: &Block,
        state: &ChainState,
        root: RootRule,
    ) -> Result<u64, LedgerError> {
        // Validate block height
        let expected_height = state.height + 1;
        if block.height() != expected_height {
            return Err(LedgerError::InvalidBlock(format!(
                "Expected height {}, got {}",
                expected_height,
                block.height()
            )));
        }

        // Validate prev_block_hash
        if block.header.prev_block_hash != state.tip_hash {
            return Err(LedgerError::InvalidBlock(
                "Previous block hash mismatch".to_string(),
            ));
        }

        // Header / minting-tx consistency: the minting tx must agree with the
        // header on the fields that feed PoW and emission. Otherwise a
        // producer can declare one difficulty/height/prev-hash in the header
        // (used by is_valid_pow and our checks here) and a different one in
        // the minting tx (which the SCP proposer path would have rejected).
        if block.minting_tx.block_height != block.height() {
            return Err(LedgerError::InvalidBlock(format!(
                "Minting tx height {} does not match header height {}",
                block.minting_tx.block_height,
                block.height()
            )));
        }
        if block.minting_tx.prev_block_hash != block.header.prev_block_hash {
            return Err(LedgerError::InvalidBlock(
                "Minting tx prev_block_hash does not match header".to_string(),
            ));
        }
        if block.minting_tx.difficulty != block.header.difficulty {
            return Err(LedgerError::InvalidBlock(
                "Minting tx difficulty does not match header".to_string(),
            ));
        }
        if block.minting_tx.minter_view_key != block.header.minter_view_key
            || block.minting_tx.minter_spend_key != block.header.minter_spend_key
        {
            return Err(LedgerError::InvalidBlock(
                "Minting tx minter keys do not match header".to_string(),
            ));
        }

        // C1: Enforce chain-expected difficulty.
        //
        // is_valid_pow() only proves the PoW hash is below the header's *own*
        // difficulty field — without this check a producer can declare a
        // trivial difficulty and have us accept the block at near-zero PoW.
        if block.header.difficulty != state.difficulty {
            return Err(LedgerError::InvalidBlock(format!(
                "Block difficulty {:#x} does not match expected {:#x}",
                block.header.difficulty, state.difficulty
            )));
        }

        // Validate PoW (against the now-verified expected difficulty).
        if !block.header.is_valid_pow() {
            return Err(LedgerError::InvalidBlock(
                "Invalid proof of work".to_string(),
            ));
        }

        // C2a: Recompute the block reward from the emission schedule and
        // chain state. Without this a producer can claim any reward, inflating
        // supply arbitrarily.
        let expected_reward = calculate_block_reward(block.height(), state.total_mined);
        if block.minting_tx.reward != expected_reward {
            return Err(LedgerError::InvalidBlock(format!(
                "Block reward {} does not match expected {} at height {}",
                block.minting_tx.reward,
                expected_reward,
                block.height()
            )));
        }

        // C2b: Timestamp sanity.
        //
        // Monotonicity vs parent prevents difficulty/emission games via
        // backdating; the future bound prevents a producer from biasing
        // timestamp-derived state forward. The header timestamp and the
        // minting-tx timestamp must agree (the SCP proposer path validates
        // the minting tx's timestamp; we hold the gossip path to the same
        // rule).
        if block.minting_tx.timestamp != block.header.timestamp {
            return Err(LedgerError::InvalidBlock(
                "Minting tx timestamp does not match header".to_string(),
            ));
        }
        if block.header.timestamp < state.tip_timestamp {
            return Err(LedgerError::InvalidBlock(format!(
                "Block timestamp {} is before parent {}",
                block.header.timestamp, state.tip_timestamp
            )));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|_| LedgerError::InvalidBlock("System time before UNIX epoch".to_string()))?;
        if block.header.timestamp > now.saturating_add(MAX_FUTURE_TIMESTAMP_SECS) {
            return Err(LedgerError::InvalidBlock(format!(
                "Block timestamp {} is too far in the future (now={})",
                block.header.timestamp, now
            )));
        }

        // C4: Verify the transaction root commits to the actual tx list.
        //
        // header.tx_root feeds the header hash and therefore PoW, but is
        // never recomputed at acceptance — a relay can swap the tx list
        // under a valid PoW unless we re-derive and compare here.
        let expected_tx_root = match root {
            RootRule::Legacy => Block::compute_tx_root(&block.transactions),
            RootRule::Experimental(hash) => hash,
        };
        if block.header.tx_root != expected_tx_root {
            return Err(LedgerError::InvalidBlock(
                "Block tx_root does not match transactions".to_string(),
            ));
        }

        // Fee-sum overflow guard (#599, #663). Per-tx fees are
        // attacker-influenced, so accumulate with `checked_add` and reject on
        // overflow with a typed error rather than wrapping silently or (under
        // `overflow-checks = true`, which the release profile now carries)
        // panicking the node. Runs here — right after the tx list is bound to
        // the header (C4) and before the expensive gates — so a crafted
        // overflow block is rejected early and deterministically: the check is
        // a pure function of the block contents, so proposer and validators
        // reach the same verdict (no fork). The validated sum is reused by the
        // fee-accounting section below.
        let block_fees: u64 = checked_block_fees(block)?;

        // C5 (issue #451): Deterministic height-based staleness backstop.
        //
        // Reject any block that contains a transfer tx that is too old relative
        // to the block's OWN height. This mirrors the staleness rule that the
        // SCP transfer-validity gate used to enforce against the local *current
        // tip* (`validate_transfer_tx`, removed in #451 because tip-dependence
        // is the #417-class fork condition). Evaluated against `block.height()`
        // — which is identical on every honest node applying block N — this
        // check is deterministic and cannot diverge across nodes.
        //
        // Honest block-builders already filter these out at build time
        // (`BlockBuilder::build_from_externalized`), so this is defense-in-depth
        // against a malformed/adversarial block, not the primary gate (it never
        // fires for blocks we build → no externalize-then-reject halt).
        if let Some(tx_idx) = first_stale_transfer_tx(block) {
            let tx = &block.transactions[tx_idx];
            return Err(LedgerError::InvalidBlock(format!(
                "Transaction {} is stale: created_at_height {} + MAX_TX_AGE {} < block height {}",
                tx_idx,
                tx.created_at_height,
                MAX_TX_AGE,
                block.height()
            )));
        }

        // C3: Resolve every ring member against the UTXO set.
        //
        // CLSAG verifies signatures over the *claimed* ring; without this
        // check a producer can fabricate ring members (target_key they
        // control + arbitrary commitment) and the signature verifies while
        // the balance check passes against the fabricated amount, minting
        // value out of thin air. Mempool already does this for tx
        // admission, but blocks bypass the mempool.
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            self.verify_ring_members(tx).map_err(|e| {
                LedgerError::InvalidBlock(format!(
                    "Transaction {} ring member validation failed: {}",
                    tx_idx, e
                ))
            })?;
        }

        // C6 (issue #576, H2-B3): cluster-tag inflation guard.
        //
        // Wire the existing transaction-core conservation-of-mass validator
        // (`validate_cluster_tag_inheritance`) into the consensus path so a
        // block carrying a tx with tag-INFLATED outputs is rejected at block
        // acceptance, not merely at mempool admission (blocks bypass the
        // mempool). The check is a pure function of the transaction plus the
        // committed UTXO state — integer/`BTreeMap` only, no node-local state —
        // so a proposer and a validator reach the same verdict (no fork). It
        // runs after C3, which has already bound every ring member to a real
        // UTXO. See `check_cluster_tag_inheritance`.
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            self.verify_cluster_tag_inheritance(tx).map_err(|e| {
                LedgerError::InvalidBlock(format!(
                    "Transaction {} cluster-tag inheritance validation failed: {}",
                    tx_idx, e
                ))
            })?;
        }

        // C7 (issue #578, H1-B4): deterministic consensus fee floor.
        //
        // Reject any block containing a transfer tx whose `fee` is below the
        // consensus floor `base_minimum_fee + demurrage_charge` recomputed here
        // from the transaction and committed chain state at `block.height()`.
        // This is the sole enforcement of the demurrage stock-level term of the
        // Gini mechanism (a miner could otherwise include own/under-fee txs and
        // evade demurrage entirely). The floor is CONGESTION-FREE: the mempool's
        // node-local dynamic fee base (f64 EMA, restart-reset) is deliberately
        // excluded, so a node with a hot vs. cold congestion EMA accepts the
        // SAME set of blocks (design #574 Q1/Q2, audit cycle 6 M1). Runs after
        // C3 (ring resolution) and C6 (tag inheritance), reusing the same
        // committed-UTXO ring resolution, and BEFORE this block's outputs are
        // applied — so the proposer and every validator compute identical floors
        // (no fork). See `verify_consensus_fee_floor`.
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            self.verify_consensus_fee_floor(tx, block.height())
                .map_err(|e| match e {
                    LedgerError::InvalidBlock(msg) => LedgerError::InvalidBlock(format!(
                        "Transaction {} fee-floor validation failed: {}",
                        tx_idx, msg
                    )),
                    // Propagate DB errors unchanged (fail-closed, M7).
                    other => other,
                })?;
        }

        // C8 (issue #831): demurrage-settlement structural validity.
        //
        // A settlement is the sanctioned wrap on-ramp (#822/#825): it
        // reclassifies wealthy-cluster value down to factor-1/background in
        // exchange for wrap eligibility. C7 above already priced the capitalized
        // settlement charge — because a settlement's outputs are background, the
        // shared `spend_demurrage_charge` fires its `capitalized_reset_charge`
        // term (== `demurrage_settlement_charge`) at the ring-floored input
        // class, so an under-paid settlement is already rejected. This check
        // enforces the STRUCTURAL rules that make the `settlement` flag mean
        // exactly what `wrap_eligible` trusts: every output is background (the
        // tag-rewrite rule; the one sanctioned full mass-drop to background) and
        // the certified `settled_value` matches the outputs. Pure tag/integer
        // comparison — no node-local state, proposer == validator. Non-settlement
        // txs are untouched.
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            self.verify_settlement(tx).map_err(|e| match e {
                LedgerError::InvalidBlock(msg) => LedgerError::InvalidBlock(format!(
                    "Transaction {} settlement validation failed: {}",
                    tx_idx, msg
                )),
                other => other,
            })?;
        }

        Ok(block_fees)
    }
}
impl ValidationReads for Ledger {
    fn get_utxo_by_target_key(&self, key: &[u8; 32]) -> Result<Option<Utxo>, LedgerError> {
        Ledger::get_utxo_by_target_key(self, key)
    }
    fn get_cluster_wealth(&self, id: u64) -> Result<u128, LedgerError> {
        Ledger::get_cluster_wealth(self, id)
    }
    fn is_bridge_import_cluster(&self, id: u64) -> Result<bool, LedgerError> {
        Ledger::is_bridge_import_cluster(self, id)
    }
    fn is_key_image_spent(&self, key: &[u8; 32]) -> Result<Option<u64>, LedgerError> {
        Ledger::is_key_image_spent(self, key)
    }
}
fn candidate_record(
    row: Result<(&[u8], &[u8]), heed::Error>,
    policy: ReadPolicy,
    check_context: &mut dyn FnMut(&Utxo) -> Result<(), LedgerError>,
) -> Result<Option<Utxo>, LedgerError> {
    match policy {
        ReadPolicy::Legacy => Ok(row
            .ok()
            .and_then(|(_, value)| bincode::deserialize::<Utxo>(value).ok())),
        ReadPolicy::Strict => {
            let (key, value) = row.map_err(|e| LedgerError::Database(e.to_string()))?;
            let utxo: Utxo = crate::ledger::experimental::decode(value)?;
            if key != utxo.id.to_bytes() {
                return Err(LedgerError::InconsistentRecord("candidate key/id".into()));
            }
            check_context(&utxo)?;
            Ok(Some(utxo))
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub(in crate::ledger) fn lottery_candidates(
    tables: &WriteTables,
    txn: &heed::RoTxn<'_>,
    block_height: u64,
    prev_block_hash: &[u8; 32],
    config: &LotteryDrawConfig,
    policy: ReadPolicy,
    mut check_context: &mut dyn FnMut(&Utxo) -> Result<(), LedgerError>,
) -> Result<Vec<LotteryCandidate>, LedgerError> {
    use bth_cluster_tax::ClusterFactorCurve;
    use std::{collections::HashMap, ops::Bound};

    /// Deterministic cap on the lottery candidate set. Must be the same
    /// for proposers and validators (consensus-critical).
    const MAX_LOTTERY_CANDIDATES: usize = 10_000;

    let mut candidates = Vec::new();
    // Per-cluster global wealth cache for this scan (u128, #626)
    let mut wealth_cache: HashMap<u64, u128> = HashMap::new();
    let factor_curve = ClusterFactorCurve::default_params();

    // Seed-derived 36-byte start offset into the UTXO-id keyspace.
    let offset_key = Ledger::lottery_candidate_offset_key(prev_block_hash, block_height);
    let offset_slice: &[u8] = &offset_key;

    // Wraparound rotation: walk `[offset, end)` then `[start, offset)`.
    // The two half-open ranges partition the UTXO keyspace exactly once
    // (disjoint, no overlap, no double-count), so chaining them visits
    // every UTXO once, starting at the seed-derived offset and wrapping to
    // the start — a deterministic, per-block-rotating window. The cap break
    // stops collection once 10k eligible UTXOs are gathered; if the eligible
    // set is <= 10k we never break and collect them all.
    let upper = tables
        .utxo_db
        .range(
            txn,
            &(Bound::Included(offset_slice), Bound::<&[u8]>::Unbounded),
        )
        .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;
    let lower = tables
        .utxo_db
        .range(
            txn,
            &(Bound::<&[u8]>::Unbounded, Bound::Excluded(offset_slice)),
        )
        .map_err(|e| LedgerError::Database(format!("Failed to create iterator: {}", e)))?;

    for result in upper.chain(lower) {
        // CONSENSUS-CRITICAL: the candidate set (including the cap, the
        // seed-derived start offset, and the wraparound order) must be
        // identical for the block proposer and every validator, because
        // lottery verification re-runs the draw. The offset is a pure
        // function of (prev_block_hash, height); LMDB range iteration order
        // is key order, a deterministic function of the UTXO set.
        if candidates.len() >= MAX_LOTTERY_CANDIDATES {
            break;
        }

        if let Some(utxo) = candidate_record(result, policy, &mut check_context)? {
            // Check eligibility: circulation window (mature but recently
            // created) and dust floor. The recency upper bound
            // (age <= circulation_window) is the Path C addition; it
            // must match LotteryCandidate::is_eligible so ρ (the reward-
            // cap denominator) is consistent across nodes.
            let age = block_height.saturating_sub(utxo.created_at);
            if age >= config.min_utxo_age
                && age <= config.circulation_window
                && utxo.output.amount >= config.min_utxo_value
            {
                // Convert ClusterTagVector to TagVector for entropy calculation
                let tag_vector = Ledger::cluster_tags_to_tag_vector(&utxo.output.cluster_tags);

                // Create candidate with UTXO ID (36 bytes: tx_hash || output_index)
                let utxo_id = utxo.id.to_bytes();

                // Effective cluster wealth for this UTXO: tag weights
                // against global per-cluster wealth (background
                // contributes zero), then mapped through the
                // fixed-point factor curve. Used by ClusterWeighted
                // winner selection; deterministic across nodes.
                let mut weighted: u128 = 0;
                for entry in &utxo.output.cluster_tags.entries {
                    let global = match wealth_cache.entry(entry.cluster_id.0) {
                        std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                        std::collections::hash_map::Entry::Vacant(e) => {
                            // 16-byte LE u128 accumulator (#626). A
                            // wrong-width value fails closed (propagates)
                            // rather than silently reading as 0 wealth,
                            // which would mis-tilt the consensus lottery.
                            let w = match tables
                                .cluster_wealth_db
                                .get(txn, entry.cluster_id.0.to_le_bytes().as_slice())
                                .map_err(|e| {
                                    LedgerError::Database(format!(
                                        "Failed to get cluster wealth: {}",
                                        e
                                    ))
                                })? {
                                Some(bytes) => decode_cluster_wealth(bytes)?,
                                None => 0u128,
                            };
                            *e.insert(w)
                        }
                    };
                    weighted =
                        weighted.saturating_add((entry.weight as u128).saturating_mul(global));
                }
                // Full u128 effective wealth: `factor()` accepts u128
                // (log-domain curve, #626 PR 1), so the lottery tilt uses
                // the un-clamped accumulator directly.
                let effective_wealth = weighted / TAG_WEIGHT_SCALE as u128;
                // factor() returns FACTOR_SCALE units (1000..6000),
                // which LotteryCandidate uses directly (integer
                // fixed-point, consensus-deterministic)
                let cluster_factor = factor_curve.factor(effective_wealth);

                let candidate = LotteryCandidate::new(
                    utxo_id,
                    utxo.output.amount,
                    cluster_factor,
                    &tag_vector,
                    utxo.created_at,
                );

                candidates.push(candidate);
            }
        }
    }

    debug!(
        block_height = block_height,
        candidate_count = candidates.len(),
        "Found lottery validation candidates"
    );

    Ok(candidates)
}
