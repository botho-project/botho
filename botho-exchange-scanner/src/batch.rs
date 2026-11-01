//! One scan pass: plan a confirmed block range, fetch it, credit its deposits,
//! and advance the checkpoint.
//!
//! # Checkpoint semantics (#1779)
//!
//! The scanner only ever scans blocks that have already reached
//! `min_confirmations` (see [`SyncState::plan_batch`]). A deposit is therefore
//! credited in the single pass that first scans its block, and the checkpoint
//! then moves past that block for good. Deposits in younger blocks are not
//! looked at (let alone skipped) until a later pass, after they mature.
//!
//! The checkpoint advances only after the output handler has accepted the
//! whole batch. If the handler fails part-way, the batch is retried on the
//! next pass, so handlers must de-duplicate on
//! [`DetectedDeposit::deposit_id`] to stay idempotent under that retry.

use std::future::Future;

use crate::{
    deposit::DetectedDeposit,
    output::OutputHandler,
    scanner::{ExchangeScanner, RpcOutput},
    sync::SyncState,
};

/// Run one scan pass against a chain at `chain_height`.
///
/// `fetch(start, end)` must return every output in blocks `start..=end`
/// (the RPC layer enforces that every requested block is served, #1656).
/// Returns the number of deposits credited, `0` if there was no newly
/// confirmed block to scan.
#[allow(clippy::too_many_arguments)]
pub async fn scan_confirmed_batch<F, Fut>(
    scanner: &ExchangeScanner,
    handler: &dyn OutputHandler,
    sync_state: &mut SyncState,
    chain_height: u64,
    min_confirmations: u64,
    batch_size: u64,
    fetch: F,
) -> anyhow::Result<u64>
where
    F: FnOnce(u64, u64) -> Fut,
    Fut: Future<Output = anyhow::Result<Vec<RpcOutput>>>,
{
    let Some((start, end)) = sync_state.plan_batch(chain_height, min_confirmations, batch_size)
    else {
        tracing::debug!(
            "No newly confirmed blocks (chain height {chain_height}, \
             min_confirmations {min_confirmations}, next {})",
            sync_state.next_scan_height()
        );
        return Ok(0);
    };

    tracing::debug!("Scanning confirmed blocks {start} to {end} (chain height: {chain_height})");

    let outputs = fetch(start, end).await?;

    // An output outside the requested range would be credited now and again
    // when its own block is scanned; refuse rather than double-credit.
    if let Some(stray) = outputs
        .iter()
        .find(|o| o.block_height < start || o.block_height > end)
    {
        anyhow::bail!(
            "output {}:{} at height {} is outside the requested range [{start}, {end}]",
            stray.tx_hash,
            stray.output_index,
            stray.block_height
        );
    }

    let deposits: Vec<DetectedDeposit> = scanner.scan_outputs(&outputs, chain_height);
    // Every block in range is confirmed by construction.
    debug_assert!(deposits
        .iter()
        .all(|d| d.confirmations >= min_confirmations.max(1)));

    let deposit_count = deposits.len() as u64;
    let total_amount: u64 = deposits.iter().map(|d| d.amount).sum();

    if !deposits.is_empty() {
        handler.handle_batch(&deposits).await?;
    }

    sync_state.update_after_batch(end, deposit_count, total_amount, outputs.len() as u64);

    Ok(deposit_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use bth_account_keys::AccountKey;
    use bth_crypto_keys::RistrettoPublic;
    use bth_transaction_clsag::TxOutput;
    use rand_core::SeedableRng;
    use std::sync::Mutex;

    /// Records every deposit handed to it, in order.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<DetectedDeposit>>);

    #[async_trait]
    impl OutputHandler for Recorder {
        async fn handle(&self, deposit: &DetectedDeposit) -> anyhow::Result<()> {
            self.0.lock().unwrap().push(deposit.clone());
            Ok(())
        }
    }

    impl Recorder {
        fn ids(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|d| d.deposit_id())
                .collect()
        }
    }

    /// A handler that rejects every batch (e.g. webhook down).
    struct Failing;

    #[async_trait]
    impl OutputHandler for Failing {
        async fn handle(&self, _deposit: &DetectedDeposit) -> anyhow::Result<()> {
            anyhow::bail!("webhook unavailable")
        }
    }

    /// An in-memory chain of `(height, outputs)` blocks.
    struct Chain {
        outputs: Vec<RpcOutput>,
    }

    impl Chain {
        fn fetch(&self, start: u64, end: u64) -> anyhow::Result<Vec<RpcOutput>> {
            Ok(self
                .outputs
                .iter()
                .filter(|o| o.block_height >= start && o.block_height <= end)
                .cloned()
                .collect())
        }
    }

    fn setup() -> (ExchangeScanner, AccountKey) {
        let mut rng = rand::rngs::StdRng::seed_from_u64(1779);
        let account = AccountKey::random(&mut rng);
        let scanner = ExchangeScanner::new(
            *account.view_private_key(),
            RistrettoPublic::from(account.spend_private_key()),
            0,
            10,
        )
        .unwrap();
        (scanner, account)
    }

    fn deposit_at(account: &AccountKey, subaddress: u64, height: u64, tag: u8) -> RpcOutput {
        let out = TxOutput::new(1_000 + tag as u64, &account.subaddress(subaddress));
        RpcOutput {
            tx_hash: hex::encode([tag; 32]),
            output_index: 0,
            target_key: hex::encode(out.target_key),
            public_key: hex::encode(out.public_key),
            amount: 1_000 + tag as u64,
            block_height: height,
            kem_ciphertext: None,
        }
    }

    async fn pass(
        scanner: &ExchangeScanner,
        handler: &dyn OutputHandler,
        state: &mut SyncState,
        chain: &Chain,
        chain_height: u64,
        min_conf: u64,
        batch: u64,
    ) -> anyhow::Result<u64> {
        scan_confirmed_batch(
            scanner,
            handler,
            state,
            chain_height,
            min_conf,
            batch,
            |s, e| async move { chain.fetch(s, e) },
        )
        .await
    }

    /// The #1779 regression: a deposit seen with too few confirmations must
    /// not be skipped forever. It is not credited while young, and is credited
    /// exactly once when a later pass sees it mature.
    #[tokio::test]
    async fn young_deposit_is_credited_exactly_once_after_it_matures() {
        let (scanner, account) = setup();
        let chain = Chain {
            outputs: vec![deposit_at(&account, 3, 20, 0xAA)],
        };
        let rec = Recorder::default();
        let mut state = SyncState::default();

        // Chain at 22: the deposit at 20 has 3 of 10 confirmations.
        let n = pass(&scanner, &rec, &mut state, &chain, 22, 10, 100)
            .await
            .unwrap();
        assert_eq!(n, 0);
        assert!(
            rec.ids().is_empty(),
            "unconfirmed deposit must not be credited"
        );
        assert!(
            state.next_scan_height() <= 20,
            "checkpoint must not pass the unconfirmed deposit's block (next = {})",
            state.next_scan_height()
        );

        // Chain grows, still short of 10 confirmations (9 at height 28).
        pass(&scanner, &rec, &mut state, &chain, 28, 10, 100)
            .await
            .unwrap();
        assert!(rec.ids().is_empty());

        // Height 29: exactly 10 confirmations -> credited.
        let n = pass(&scanner, &rec, &mut state, &chain, 29, 10, 100)
            .await
            .unwrap();
        assert_eq!(n, 1);
        let credited = rec.0.lock().unwrap().clone();
        assert_eq!(credited.len(), 1);
        assert_eq!(credited[0].subaddress_index, 3);
        assert_eq!(credited[0].block_height, 20);
        assert_eq!(credited[0].confirmations, 10);

        // Many more passes as the chain keeps growing: never re-credited.
        for h in [29, 30, 31, 100, 5_000] {
            pass(&scanner, &rec, &mut state, &chain, h, 10, 1000)
                .await
                .unwrap();
        }
        assert_eq!(rec.ids().len(), 1, "deposit must be credited exactly once");
        assert_eq!(state.total_deposits, 1);
    }

    /// Re-running a pass at the same chain height credits nothing new.
    #[tokio::test]
    async fn rescan_at_the_same_height_is_idempotent() {
        let (scanner, account) = setup();
        let chain = Chain {
            outputs: vec![
                deposit_at(&account, 1, 0, 0x01),
                deposit_at(&account, 2, 5, 0x02),
                deposit_at(&account, 4, 40, 0x03),
            ],
        };
        let rec = Recorder::default();
        let mut state = SyncState::default();

        for _ in 0..5 {
            pass(&scanner, &rec, &mut state, &chain, 49, 10, 100)
                .await
                .unwrap();
        }
        let mut ids = rec.ids();
        ids.sort();
        ids.dedup();
        assert_eq!(rec.ids().len(), 3, "each deposit credited once");
        assert_eq!(ids.len(), 3);
    }

    /// Paging (#1656) and confirmation gating compose: with a small batch the
    /// scanner walks the confirmed range page by page, never past the
    /// confirmed tip, and credits each deposit exactly once in height order.
    #[tokio::test]
    async fn paged_scan_stops_at_the_confirmed_tip() {
        let (scanner, account) = setup();
        let chain = Chain {
            outputs: (0..10u8)
                .map(|i| deposit_at(&account, i as u64, i as u64 * 7, i))
                .collect(),
        };
        let rec = Recorder::default();
        let mut state = SyncState::default();

        // Chain at 60, min 6: confirmed tip 55 -> deposits at 0..=49 (8 of
        // them); the ones at 56 and 63 must wait.
        for _ in 0..20 {
            pass(&scanner, &rec, &mut state, &chain, 60, 6, 4)
                .await
                .unwrap();
        }
        assert_eq!(state.last_scanned_height, 55);
        assert_eq!(rec.ids().len(), 8);

        for _ in 0..20 {
            pass(&scanner, &rec, &mut state, &chain, 70, 6, 4)
                .await
                .unwrap();
        }
        assert_eq!(state.last_scanned_height, 65);
        let heights: Vec<u64> = rec
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|d| d.block_height)
            .collect();
        assert_eq!(heights, (0..10u64).map(|i| i * 7).collect::<Vec<_>>());
    }

    /// A handler failure leaves the checkpoint where it was, so the deposit is
    /// retried rather than lost.
    #[tokio::test]
    async fn handler_failure_does_not_advance_the_checkpoint() {
        let (scanner, account) = setup();
        let chain = Chain {
            outputs: vec![deposit_at(&account, 0, 3, 0x10)],
        };
        let mut state = SyncState::default();

        assert!(pass(&scanner, &Failing, &mut state, &chain, 20, 10, 100)
            .await
            .is_err());
        assert_eq!(state.next_scan_height(), 0);

        let rec = Recorder::default();
        pass(&scanner, &rec, &mut state, &chain, 20, 10, 100)
            .await
            .unwrap();
        assert_eq!(rec.ids().len(), 1);
    }

    /// A node that returns outputs outside the requested range is rejected:
    /// crediting them now would credit them again when their block is
    /// scanned.
    #[tokio::test]
    async fn out_of_range_outputs_are_refused() {
        let (scanner, account) = setup();
        let stray = deposit_at(&account, 0, 25, 0x20);
        let rec = Recorder::default();
        let mut state = SyncState::default();

        let res = scan_confirmed_batch(&scanner, &rec, &mut state, 30, 10, 100, |_, _| async {
            Ok(vec![stray])
        })
        .await;
        assert!(res.is_err());
        assert!(rec.ids().is_empty());
        assert_eq!(state.next_scan_height(), 0);
    }
}
