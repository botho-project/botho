//! TEST/DRT ONLY: the production cluster-tag checks of block validation
//! over an in-memory [`ValidationReads`] double, for the Lean differential
//! tests in `formal-drt/` (#1840).
//!
//! Compiled only with the non-default `test-fixtures` feature, like
//! [`super::formal_drt`], so no node build contains it. It changes no
//! consensus code: each function calls the production default method of
//! `ValidationReads` unchanged (`verify_cluster_tag_inheritance`, which runs
//! `check_cluster_tag_inheritance`; `effective_cluster_wealth_from_outputs`;
//! `import_floor_factor_from_outputs`). Only the pre-state those methods read
//! (UTXOs by target key, per-cluster wealth, the bridge-import cluster set)
//! is in memory.

use super::{store::validation::ValidationReads, LedgerError};
use crate::transaction::{
    ClsagRingInput, RingMember, Transaction, TxInputs, TxOutput, Utxo, UtxoId,
};
use bth_transaction_types::ClusterTagVector;
use std::collections::{BTreeMap, BTreeSet};

/// A ring member or an output: `(amount, tags)`.
pub type Coin = (u64, ClusterTagVector);

#[derive(Default)]
struct Double {
    utxos: BTreeMap<[u8; 32], Utxo>,
    wealth: BTreeMap<u64, u128>,
    imports: BTreeSet<u64>,
}

impl ValidationReads for Double {
    fn get_utxo_by_target_key(&self, key: &[u8; 32]) -> Result<Option<Utxo>, LedgerError> {
        Ok(self.utxos.get(key).cloned())
    }
    fn get_cluster_wealth(&self, id: u64) -> Result<u128, LedgerError> {
        Ok(self.wealth.get(&id).copied().unwrap_or(0))
    }
    fn is_bridge_import_cluster(&self, id: u64) -> Result<bool, LedgerError> {
        Ok(self.imports.contains(&id))
    }
    fn is_key_image_spent(&self, _key: &[u8; 32]) -> Result<Option<u64>, LedgerError> {
        Ok(None)
    }
}

/// A distinct 32-byte key for ring `ring`, member `member` (outputs use
/// `ring = u32::MAX`).
fn key(ring: u32, member: u32) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[..4].copy_from_slice(&ring.to_le_bytes());
    k[4..8].copy_from_slice(&member.to_le_bytes());
    k[8] = 0xA5;
    k
}

fn output((amount, tags): &Coin, target_key: [u8; 32]) -> TxOutput {
    TxOutput {
        amount: *amount,
        target_key,
        public_key: target_key,
        e_memo: None,
        cluster_tags: tags.clone(),
        kem_ciphertext: None,
    }
}

fn outputs(outs: &[Coin]) -> Vec<TxOutput> {
    outs.iter()
        .enumerate()
        .map(|(i, c)| output(c, key(u32::MAX, i as u32)))
        .collect()
}

/// C6 as block validation runs it
/// (`ValidationReads::verify_cluster_tag_inheritance` over a transaction whose
/// rings resolve to `rings`): one CLSAG input per ring, every member a stored
/// UTXO. The signature is not checked by C6.
pub fn cluster_tag_inheritance(rings: &[Vec<Coin>], outs: &[Coin]) -> Result<(), LedgerError> {
    let mut double = Double::default();
    let mut inputs = Vec::with_capacity(rings.len());
    for (r, ring) in rings.iter().enumerate() {
        let mut members = Vec::with_capacity(ring.len());
        for (m, coin) in ring.iter().enumerate() {
            let out = output(coin, key(r as u32, m as u32));
            members.push(RingMember::from_output(&out));
            double.utxos.insert(
                out.target_key,
                Utxo {
                    id: UtxoId::new(out.target_key, 0),
                    output: out,
                    created_at: 0,
                },
            );
        }
        inputs.push(ClsagRingInput {
            ring: members,
            key_image: key(r as u32, u32::MAX),
            commitment_key_image: [0u8; 32],
            clsag_signature: Vec::new(),
            pseudo_output_amount: 0,
        });
    }
    let tx = Transaction {
        inputs: TxInputs::new(inputs),
        outputs: outputs(outs),
        fee: 0,
        created_at_height: 0,
        settlement: None,
    };
    double.verify_cluster_tag_inheritance(&tx)
}

/// `ValidationReads::effective_cluster_wealth_from_outputs` with the given
/// per-cluster wealth (absent clusters have wealth 0).
pub fn effective_cluster_wealth(
    outs: &[Coin],
    wealth: &[(u64, u128)],
) -> Result<u128, LedgerError> {
    let double = Double {
        wealth: wealth.iter().copied().collect(),
        ..Double::default()
    };
    double.effective_cluster_wealth_from_outputs(&outputs(outs))
}

/// `ValidationReads::import_floor_factor_from_outputs` with the given
/// bridge-import cluster set.
pub fn import_floor_factor(outs: &[Coin], imports: &[u64]) -> Result<u64, LedgerError> {
    let double = Double {
        imports: imports.iter().copied().collect(),
        ..Double::default()
    };
    double.import_floor_factor_from_outputs(&outputs(outs))
}
