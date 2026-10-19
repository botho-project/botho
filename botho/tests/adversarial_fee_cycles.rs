//! Bounded production-path fee experiments; no network throughput is simulated.
//! Seeded classical outputs isolate fee policy from ML-KEM and live decoy
//! supply.
use botho::{
    block::{dynamic_timing::compute_block_time, Block},
    consensus::ConsensusConfig,
    ledger::{ChainState, Ledger, LedgerError, UtxoSnapshot},
    mempool::{Mempool, MempoolError},
    transaction::{
        ClsagRingInput, RingMember, Transaction, TxOutput, Utxo, UtxoId, MIN_RING_SIZE, MIN_TX_FEE,
    },
};
use bth_account_keys::AccountKey;
use bth_cluster_tax::{DynamicFeeBase, FeeConfig, TransactionType};
use bth_crypto_keys::RistrettoPrivate;
use bth_util_from_random::FromRandom;
use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha20Rng;
use std::collections::HashSet;

const SEEDS: [u64; 3] = [1476, 0xdead_beef, 0x7265_636f_7665_7279];
const HEIGHT: u64 = 100;
const VALUE: u64 = 1_000_000_000_000;

struct Fixture {
    _dir: tempfile::TempDir,
    ledger: Ledger,
    account: AccountKey,
    utxos: Vec<Utxo>,
    rng: ChaCha20Rng,
}

impl Fixture {
    fn new(seed: u64) -> Self {
        eprintln!("adversarial fee fixture seed={seed:#x}");
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let account = AccountKey::new(
            &RistrettoPrivate::from_random(&mut rng),
            &RistrettoPrivate::from_random(&mut rng),
        );
        let utxos: Vec<_> = (0..MIN_RING_SIZE + 12)
            .map(|i| Utxo {
                id: UtxoId::new([i as u8; 32], 0),
                output: TxOutput::new_with_key(
                    VALUE,
                    &account.default_subaddress(),
                    &RistrettoPrivate::from_random(&mut rng),
                ),
                created_at: HEIGHT - 20,
            })
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        let snapshot = UtxoSnapshot::new(
            HEIGHT,
            [0; 32],
            ChainState {
                height: HEIGHT,
                ..ChainState::default()
            },
            utxos.clone(),
            vec![],
            vec![],
        )
        .unwrap();
        ledger.load_from_snapshot(&snapshot, None).unwrap();
        Self {
            _dir: dir,
            ledger,
            account,
            utxos,
            rng,
        }
    }

    fn transaction(&mut self, input: usize, fee: u64) -> Transaction {
        let real = &self.utxos[input].output;
        let outputs = [VALUE / 3, VALUE - VALUE / 3 - fee]
            .map(|amount| {
                TxOutput::new_with_key(
                    amount,
                    &self.account.default_subaddress(),
                    &RistrettoPrivate::from_random(&mut self.rng),
                )
            })
            .to_vec();
        let signing_hash =
            Transaction::new_clsag(vec![], outputs.clone(), fee, HEIGHT).signing_hash();
        let mut ring: Vec<_> = std::iter::once(real)
            .chain(
                self.utxos
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != input)
                    .take(MIN_RING_SIZE - 1)
                    .map(|(_, u)| &u.output),
            )
            .map(RingMember::from_output)
            .collect();
        ring.shuffle(&mut self.rng);
        let index = ring
            .iter()
            .position(|m| m.target_key == real.target_key)
            .unwrap();
        let key = real.recover_spend_key(&self.account, 0).unwrap();
        let input =
            ClsagRingInput::new(ring, index, &key, VALUE, &signing_hash, &mut self.rng).unwrap();
        let tx = Transaction::new_clsag(vec![input], outputs, fee, HEIGHT);
        self.ledger.verify_transaction(&tx).unwrap();
        assert!(tx.fee >= self.ledger.consensus_fee_floor(&tx, HEIGHT).unwrap());
        tx
    }
}

/// Only timestamps and transaction counts are consumed by dynamic_timing.
/// These headers are deliberately not mined, signed, or applied to a ledger.
fn timing_window(count: usize, seconds: u64, specimen: &Transaction) -> Vec<Block> {
    let mut first = Block::genesis();
    first.header.timestamp = 1_000;
    let mut last = first.clone();
    last.header.timestamp += seconds;
    last.transactions = vec![specimen.clone(); count];
    vec![first, last]
}

#[test]
fn repeated_load_cycles_cross_timing_thresholds_and_recover_without_fee_drift() {
    let mut fixture = Fixture::new(SEEDS[0]);
    let tx = fixture.transaction(0, MIN_TX_FEE);
    let max = ConsensusConfig::default().max_txs_per_slot;
    let mut pool = Mempool::new();
    // Paired windows straddle every production threshold; do not derive the
    // expected level by copying the production threshold table.
    for (count, seconds, expected) in [
        (0, 100, 40),
        (19, 100, 40),
        (20, 100, 20),
        (99, 100, 20),
        (100, 100, 10),
        (49, 10, 10),
        (50, 10, 5),
        (59, 3, 5),
        (60, 3, 3),
    ] {
        assert_eq!(
            compute_block_time(&timing_window(count, seconds, &tx)),
            expected,
            "count={count}, seconds={seconds}"
        );
    }
    for cycle in 0..8 {
        for _ in 0..64 {
            let duration = compute_block_time(&timing_window(max, 3, &tx));
            pool.update_dynamic_fee(max, max, duration == 3);
        }
        let peak = pool.current_fee_base();
        assert!(peak > 1 && peak <= 100, "cycle={cycle}, peak={peak}");
        // High fullness alone must not activate fees outside the fastest tier.
        pool.update_dynamic_fee(max, max, false);
        assert_eq!(pool.current_fee_base(), 1, "cycle={cycle}");
        // Keep the fastest timing flag while empty blocks drain the EMA: this
        // catches recovery defects hidden by simply turning adjustment off.
        for _ in 0..64 {
            pool.update_dynamic_fee(0, max, true);
        }
        assert_eq!(pool.current_fee_base(), 1, "cycle={cycle}");
        assert!(
            pool.dynamic_fee_state().ema_fullness < 0.001,
            "cycle={cycle}"
        );
        assert!(pool.dynamic_fee_state().recent_fullness.len() <= 32);
    }
}

#[test]
fn shuffled_signed_fee_cohorts_compete_for_slots_without_loss_or_duplicate_reservations() {
    for seed in SEEDS {
        let mut f = Fixture::new(seed);
        let mut cohort: Vec<_> = (0..12)
            .map(|i| f.transaction(i, MIN_TX_FEE * [1, 2, 4][i % 3]))
            .collect();
        let size = cohort[0].estimate_size();
        assert!(cohort.iter().all(|tx| tx.estimate_size() == size));
        cohort.shuffle(&mut f.rng);
        let mut pool = Mempool::new();
        // A forged high-fee offer must not buy priority or poison the valid
        // payment's reservation. Preserve structure and exact conservation so
        // neither can mask a regression that skips signature authentication.
        let mut forged = cohort[0].clone();
        let original_fee = forged.fee;
        forged.fee *= 8;
        forged.outputs[1].amount -= forged.fee - original_fee;
        assert!(forged.is_valid_structure().is_ok(), "seed={seed}");
        let input_total: u128 = forged
            .inputs
            .clsag()
            .iter()
            .map(|input| u128::from(input.pseudo_output_amount))
            .sum();
        let output_total: u128 = forged
            .outputs
            .iter()
            .map(|output| u128::from(output.amount))
            .sum();
        assert_eq!(
            input_total,
            output_total + u128::from(forged.fee),
            "seed={seed}"
        );
        assert_eq!(
            forged.verify_ring_signatures(),
            Err("Invalid CLSAG signature"),
            "seed={seed}"
        );
        match f.ledger.verify_transaction(&forged) {
            Err(LedgerError::InvalidBlock(reason)) => {
                assert_eq!(
                    reason, "Invalid ring signature: Invalid CLSAG signature",
                    "seed={seed}"
                );
            }
            result => panic!("seed={seed}, expected ledger signature failure: {result:?}"),
        }
        let forged_key_image = forged.inputs.clsag()[0].key_image;
        assert!(
            matches!(
                pool.add_tx(forged, &f.ledger),
                Err(MempoolError::InvalidSignature)
            ),
            "seed={seed}"
        );
        assert!(pool.is_empty(), "seed={seed}");
        assert!(!pool.is_key_image_pending(&forged_key_image), "seed={seed}");
        for tx in &cohort {
            pool.add_tx(tx.clone(), &f.ledger).unwrap();
        }
        let mut seen = HashSet::new();
        let mut previous_fee = u64::MAX;
        let mut paid = 0;
        // Three places for twelve independent payments creates real selection
        // contention. All outputs are untagged, so cluster factors are equal.
        for slot in 0..4 {
            let chosen = pool.get_transactions(3);
            assert_eq!(chosen.len(), 3, "seed={seed}, slot={slot}");
            for tx in &chosen {
                assert!(tx.fee <= previous_fee, "seed={seed}, slot={slot}");
                previous_fee = tx.fee;
                assert!(seen.insert(tx.hash()), "seed={seed}, duplicate selection");
                paid += tx.fee;
            }
            pool.remove_confirmed(&chosen);
            assert_eq!(pool.len(), 12 - (slot + 1) * 3, "seed={seed}");
        }
        assert_eq!(paid, 4 * (1 + 2 + 4) * MIN_TX_FEE, "seed={seed}");
        assert!(pool.is_empty());
        // Removal must release key-image reservations, not just the hash map.
        for tx in &cohort {
            pool.add_tx(tx.clone(), &f.ledger).unwrap();
        }
        assert_eq!(pool.len(), 12, "seed={seed}");
    }
}

#[test]
fn default_quotes_rise_but_minimum_transaction_fee_masks_ordinary_admission() {
    let mut f = Fixture::new(SEEDS[0]);
    let tx = f.transaction(0, MIN_TX_FEE);
    let mut hot = Mempool::new();
    let mut cold = Mempool::new();
    for _ in 0..64 {
        hot.update_dynamic_fee(100, 100, true);
    }
    assert!(hot.current_fee_base() > cold.current_fee_base());
    let config = FeeConfig::default();
    let raw = config.minimum_fee_dynamic_with_outputs(
        TransactionType::Hidden,
        tx.estimate_size(),
        0,
        tx.outputs.len(),
        0,
        hot.current_fee_base(),
    );
    assert!(
        raw < MIN_TX_FEE,
        "ordinary transaction no longer masks the dynamic quote"
    );
    hot.add_tx(tx.clone(), &f.ledger).unwrap();
    cold.add_tx(tx.clone(), &f.ledger).unwrap();
    assert_eq!(
        hot.get_transactions(1)[0].fee,
        cold.get_transactions(1)[0].fee
    );
}

#[test]
fn relay_pressure_rejects_then_recovers_without_poisoning_signature_or_reservation_state() {
    for seed in SEEDS {
        let mut f = Fixture::new(seed);
        // Elevated local policy exposes admission boundaries masked by the
        // default MIN_TX_FEE. It is not the deployed network configuration.
        let config = FeeConfig::default();
        let specimen = f.transaction(0, MIN_TX_FEE);
        let baseline = config
            .minimum_fee_dynamic_with_outputs(
                TransactionType::Hidden,
                specimen.estimate_size(),
                0,
                2,
                0,
                10_000,
            )
            .max(MIN_TX_FEE);
        let tx = f.transaction(0, baseline);
        let consensus_floor = f.ledger.consensus_fee_floor(&tx, HEIGHT).unwrap();
        let mut hot = Mempool::with_dynamic_fee(
            config.clone(),
            DynamicFeeBase::new(10_000, 1_000_000, 0.75, 8.0, 0.125),
        );
        let mut cold = Mempool::new();
        cold.add_tx(tx.clone(), &f.ledger).unwrap();
        for cycle in 0..4 {
            for _ in 0..64 {
                hot.update_dynamic_fee(100, 100, true);
            }
            match hot.add_tx(tx.clone(), &f.ledger) {
                Err(MempoolError::FeeTooLow { minimum, provided }) => {
                    assert_eq!(provided, baseline, "seed={seed}, cycle={cycle}");
                    assert!(minimum > baseline && minimum >= consensus_floor);
                }
                result => panic!("seed={seed}, cycle={cycle}, expected fee rejection: {result:?}"),
            }
            assert!(hot.is_empty());
            assert!(!hot.is_key_image_pending(&tx.inputs.clsag()[0].key_image));
            f.ledger.verify_transaction(&tx).unwrap();
            assert_eq!(
                f.ledger.consensus_fee_floor(&tx, HEIGHT).unwrap(),
                consensus_floor
            );
            for _ in 0..64 {
                hot.update_dynamic_fee(0, 100, true);
            }
            hot.add_tx(tx.clone(), &f.ledger).unwrap();
            assert_eq!(hot.remove_tx(&tx.hash()).unwrap().hash(), tx.hash());
        }
        assert_eq!(hot.fee_metrics().fee_rejections, 4, "seed={seed}");
    }
}
