//! Expensive fixtures, built once per run and shared by every case: the LMDB
//! ledgers of the window leg and the RandomX outputs of the drawing leg's
//! parent headers.

use crate::wire::{b, n};
use botho::{
    block::{Block, BlockHeader},
    ledger::{ChainState, Ledger, UtxoSnapshot},
    transaction::{TxOutput, Utxo, UtxoId},
};
use bth_account_keys::AccountKey;
use bth_crypto_keys::RistrettoPrivate;
use bth_util_from_random::FromRandom;
use proptest::prelude::*;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde_json::{json, Value};
use std::cell::OnceCell;
use tempfile::TempDir;

/// The height the window ledgers are built around. Rows are created at
/// `WINDOW_H0 - age`.
pub const WINDOW_H0: u64 = 1_000_000;

/// One window ledger: its row count, and whether every row is eligible at
/// `WINDOW_H0` under the canonical configuration (so the eligible count is
/// exactly the row count, to land on `MAX_LOTTERY_CANDIDATES` ± 1).
pub struct LedgerSpec {
    pub rows: usize,
    pub all_eligible: bool,
}

pub const LEDGERS: &[LedgerSpec] = &[
    LedgerSpec {
        rows: 0,
        all_eligible: false,
    },
    LedgerSpec {
        rows: 1,
        all_eligible: true,
    },
    LedgerSpec {
        rows: 2,
        all_eligible: false,
    },
    LedgerSpec {
        rows: 3,
        all_eligible: false,
    },
    LedgerSpec {
        rows: 5,
        all_eligible: false,
    },
    LedgerSpec {
        rows: 8,
        all_eligible: false,
    },
    LedgerSpec {
        rows: 17,
        all_eligible: false,
    },
    LedgerSpec {
        rows: 64,
        all_eligible: false,
    },
    LedgerSpec {
        rows: 300,
        all_eligible: false,
    },
    LedgerSpec {
        rows: 9_999,
        all_eligible: true,
    },
    LedgerSpec {
        rows: 10_000,
        all_eligible: true,
    },
    LedgerSpec {
        rows: 10_001,
        all_eligible: true,
    },
    LedgerSpec {
        rows: 10_002,
        all_eligible: true,
    },
    LedgerSpec {
        rows: 12_000,
        all_eligible: false,
    },
];

/// A ledger index: the small ledgers far more often than the
/// `MAX_LOTTERY_CANDIDATES` ones (each of those reads and ships ~10k rows).
pub fn ledger_index() -> BoxedStrategy<usize> {
    prop_oneof![
        200 => 0usize..9,
        1 => 9usize..LEDGERS.len(),
    ]
    .boxed()
}

/// The PoW fields of a parent header (the rest of the header is generated).
pub struct Tip {
    pub height: u64,
    pub nonce: u64,
    pub prev: [u8; 32],
    pub view: [u8; 32],
    pub spend: [u8; 32],
}

const fn tip(height: u64, nonce: u64, prev: u8) -> Tip {
    Tip {
        height,
        nonce,
        prev: [prev; 32],
        view: [0x22; 32],
        spend: [0x33; 32],
    }
}

/// Parent headers of the drawing leg. Few distinct PoW preimages and RandomX
/// epochs, because each RandomX evaluation costs milliseconds and each epoch a
/// cache rebuild. The heights cover a young chain (eligibility mostly empty),
/// a mature one, and `u64::MAX` (no child height: the seed's parent check
/// fails).
pub const TIP_POOL: &[Tip] = &[
    tip(999, 0, 0x11),
    tip(999, 1, 0x11),
    tip(20_000, 0, 0x11),
    tip(20_000, 7, 0x44),
    tip(3_000_000, 0, 0x11),
    tip(3_000_000, 1, 0x44),
    tip(u64::MAX, 0, 0x11),
];

struct LedgerFx {
    _dir: TempDir,
    ledger: Ledger,
    rows: Value,
}

pub struct Fixtures {
    ledgers: Vec<OnceCell<LedgerFx>>,
    pow: Vec<OnceCell<[u8; 32]>>,
    genesis: Block,
}

impl Fixtures {
    pub fn new() -> Self {
        Self {
            ledgers: LEDGERS.iter().map(|_| OnceCell::new()).collect(),
            pow: TIP_POOL.iter().map(|_| OnceCell::new()).collect(),
            genesis: Block::genesis(),
        }
    }

    pub fn genesis(&self) -> &Block {
        &self.genesis
    }

    /// The RandomX output of the parent `tip` (`BlockHeader::pow_hash`), which
    /// depends only on the pool entry's fields.
    pub fn pow_hash(&self, index: usize, header: &BlockHeader) -> [u8; 32] {
        *self.pow[index].get_or_init(|| header.pow_hash())
    }

    fn ledger_fx(&self, index: usize) -> &LedgerFx {
        self.ledgers[index].get_or_init(|| build_ledger(index))
    }

    pub fn ledger(&self, index: usize) -> &Ledger {
        &self.ledger_fx(index).ledger
    }

    /// The ledger's UTXO table, in LMDB key order, as the model's input.
    pub fn ledger_rows_wire(&self, index: usize) -> Value {
        self.ledger_fx(index).rows.clone()
    }
}

fn build_ledger(index: usize) -> LedgerFx {
    let spec = &LEDGERS[index];
    let mut rng = ChaCha20Rng::seed_from_u64(0x1707_0000 + index as u64);
    let account = AccountKey::new(
        &RistrettoPrivate::from_random(&mut rng),
        &RistrettoPrivate::from_random(&mut rng),
    );
    // One real output, re-used with different amounts: the candidate read
    // never looks at the keys.
    let template = TxOutput::new_with_key(
        0,
        &account.default_subaddress(),
        &RistrettoPrivate::from_random(&mut rng),
    );
    let utxos: Vec<Utxo> = (0..spec.rows)
        .map(|_| {
            let mut tx_hash = [0u8; 32];
            rng.fill_bytes(&mut tx_hash);
            let index = if rng.random_bool(0.5) {
                0
            } else {
                rng.random::<u32>()
            };
            let (amount, age) = if spec.all_eligible {
                (
                    rng.random_range(1_000_000..1_000_000_000_000),
                    rng.random_range(2_000..8_000),
                )
            } else {
                let amount = match rng.random_range(0..10) {
                    0 => rng.random_range(999_990..1_000_010),
                    1 => rng.random::<u64>(),
                    _ => rng.random_range(1_000_000..1_000_000_000_000),
                };
                let age = match rng.random_range(0..10) {
                    0 => rng.random_range(715..725),
                    1 => rng.random_range(9_995..10_005),
                    2 => rng.random_range(0..WINDOW_H0 + 10),
                    _ => rng.random_range(0..12_000),
                };
                (amount, age)
            };
            let mut output = template.clone();
            output.amount = amount;
            Utxo {
                id: UtxoId::new(tx_hash, index),
                output,
                created_at: WINDOW_H0.saturating_sub(age),
            }
        })
        .collect();
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = Ledger::open(dir.path()).expect("open ledger");
    let snapshot = UtxoSnapshot::new(
        WINDOW_H0 - 1,
        [0; 32],
        ChainState {
            height: WINDOW_H0 - 1,
            ..ChainState::default()
        },
        utxos,
        vec![],
        vec![],
    )
    .expect("snapshot");
    ledger
        .seed_fixture_snapshot_unvalidated(&snapshot)
        .expect("seed ledger");
    // The rows in the table's own (key) order, as the model reads them.
    let rows = ledger
        .fixture_utxos()
        .expect("read back rows")
        .iter()
        .map(|u| {
            json!({
                "id": b(&u.id.to_bytes()),
                "amount": n(u.output.amount),
                "created_at": n(u.created_at),
                // The fee curve's factor is an input to the model and not
                // compared by this leg.
                "cluster_factor": n(1_000u64),
            })
        })
        .collect();
    LedgerFx {
        _dir: dir,
        ledger,
        rows: Value::Array(rows),
    }
}
