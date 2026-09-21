//! Shared synthetic model only; historical and reinvestment tests stay
//! separate.
use super::reference;
use botho::{
    decoy_selection::{GammaDecoySelector, OutputCandidate},
    transaction::{Utxo, UtxoId},
};
use bth_cluster_tax::{LotteryCandidate, LotteryDrawConfig, TagVector};
use bth_transaction_clsag::{EncryptedMemo, TxOutput};
use bth_transaction_types::ClusterTagVector;
use rand::{rngs::StdRng, SeedableRng};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const BTH: u64 = 1_000_000_000_000;
pub(crate) const ATTACKER: usize = 100;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Coin {
    pub(crate) id: [u8; 36],
    pub(crate) owner: usize,
    pub(crate) target_key: [u8; 32],
    pub(crate) value: u64,
    pub(crate) created: u64,
    pub(crate) spent: bool,
    pub(crate) payout: bool,
}
impl Coin {
    pub(crate) fn utxo(&self) -> Utxo {
        let mut hash = [0; 32];
        hash.copy_from_slice(&self.id[..32]);
        // Synthetic record identity only; no encrypted memo or signed transaction is
        // claimed.
        let mut identity = [0u8; 66];
        identity[..36].copy_from_slice(&self.id);
        Utxo {
            id: UtxoId::new(hash, u32::from_le_bytes(self.id[32..].try_into().unwrap())),
            output: TxOutput {
                amount: self.value,
                target_key: self.target_key,
                public_key: self.target_key,
                e_memo: EncryptedMemo::from_bytes(&identity),
                cluster_tags: ClusterTagVector::empty(),
                kem_ciphertext: None,
            },
            created_at: self.created,
        }
    }
    pub(crate) fn candidate(&self) -> LotteryCandidate {
        LotteryCandidate::new(self.id, self.value, 1000, &TagVector::new(), self.created)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Model {
    pub(crate) coins: BTreeMap<[u8; 36], Coin>,
    pub(crate) serial: u64,
}
impl Model {
    pub(crate) fn add(&mut self, owner: usize, value: u64, height: u64, payout: bool) -> [u8; 36] {
        self.serial += 1;
        let mut hash = Sha256::new();
        hash.update(b"CT1_WORKLOAD_OUTPUT_V1");
        hash.update(self.serial.to_le_bytes());
        let mut id = [0; 36];
        id[..32].copy_from_slice(&hash.finalize());
        assert!(self
            .coins
            .insert(
                id,
                Coin {
                    id,
                    owner,
                    target_key: id[..32].try_into().unwrap(),
                    value,
                    created: height,
                    spent: false,
                    payout
                }
            )
            .is_none());
        id
    }
    pub(crate) fn accounted(&self, owner: usize) -> u128 {
        self.coins
            .values()
            .filter(|c| c.owner == owner && !c.spent)
            .map(|c| c.value as u128)
            .sum()
    }
    pub(crate) fn spendable(&self, owner: usize) -> u128 {
        self.coins
            .values()
            .filter(|c| c.owner == owner && !c.spent && !c.payout)
            .map(|c| c.value as u128)
            .sum()
    }
    pub(crate) fn locked(&self, owner: usize) -> u128 {
        self.coins
            .values()
            .filter(|c| c.owner == owner && !c.spent && c.payout)
            .map(|c| c.value as u128)
            .sum()
    }
    pub(crate) fn largest(&self, owner: usize, height: u64) -> Option<[u8; 36]> {
        self.coins
            .values()
            .filter(|c| {
                c.owner == owner && !c.spent && !c.payout && height.saturating_sub(c.created) >= 10
            })
            .max_by_key(|c| (c.value, c.id))
            .map(|c| c.id)
    }
    // Source-parity adapter for Ledger's public keyspace rotation. The real
    // ledger test below checks full order, not merely membership.
    pub(crate) fn candidates(&self, height: u64, hash: &[u8; 32]) -> Vec<LotteryCandidate> {
        let mut h = Sha256::new();
        h.update(b"LOTTERY_CANDIDATE_OFFSET_V1");
        h.update(hash);
        h.update(height.to_le_bytes());
        let head = h.finalize();
        let mut t = Sha256::new();
        t.update(b"LOTTERY_CANDIDATE_OFFSET_V1_TAIL");
        t.update(head);
        let tail = t.finalize();
        let mut offset = [0; 36];
        offset[..32].copy_from_slice(&head);
        offset[32..].copy_from_slice(&tail[..4]);
        let cfg = LotteryDrawConfig::default();
        let result: Vec<_> = self
            .coins
            .range(offset..)
            .chain(self.coins.range(..offset))
            .map(|(_, c)| c.candidate())
            .filter(|c| c.is_eligible(height, &cfg))
            .collect();
        assert!(
            result.len() <= 10000,
            "do not approximate the production candidate cap"
        );
        result
    }
    pub(crate) fn transfer(
        &mut self,
        inputs: &[[u8; 36]],
        owner: usize,
        recipient: usize,
        payment: Option<u64>,
        outputs: usize,
        height: u64,
        seed: u64,
    ) -> Result<(u64, Vec<[u8; 36]>), &'static str> {
        self.transfer_with_award_policy(
            inputs, owner, recipient, payment, outputs, height, seed, None,
        )
    }
    // Explicit model-only policy; None is the unchanged historical locked path.
    pub(crate) fn transfer_with_award_policy(
        &mut self,
        inputs: &[[u8; 36]],
        owner: usize,
        recipient: usize,
        payment: Option<u64>,
        outputs: usize,
        height: u64,
        seed: u64,
        award_age: Option<u64>,
    ) -> Result<(u64, Vec<[u8; 36]>), &'static str> {
        if inputs.is_empty() {
            return Err("no_private_input");
        }
        if inputs.iter().copied().collect::<BTreeSet<_>>().len() != inputs.len() {
            return Err("duplicate_input");
        }
        let mut total = 0u64;
        let mut charges = vec![];
        let excluded: Vec<_> = inputs
            .iter()
            .map(|id| self.coins[id].utxo().output.target_key)
            .collect();
        let pool: Vec<_> = self
            .coins
            .values()
            .filter(|c| {
                c.created <= height.saturating_sub(10)
                    && !excluded.contains(&c.utxo().output.target_key)
            })
            .map(|c| OutputCandidate::from_utxo(&c.utxo(), height))
            .collect();
        for (position, id) in inputs.iter().enumerate() {
            let c = &self.coins[id];
            assert_eq!(c.owner, owner);
            assert!(!c.spent);
            if c.payout {
                let Some(minimum_age) = award_age else {
                    return Err("locked_payout");
                };
                if height.saturating_sub(c.created) < minimum_age {
                    return Err("policy_deferred");
                }
            }
            assert!(height >= c.created + 10);
            let mut rng = StdRng::seed_from_u64(
                seed ^ height.rotate_left(17) ^ (owner as u64).rotate_left(32) ^ position as u64,
            );
            let selected = GammaDecoySelector::new()
                .select_decoys_for_input(&pool, 19, &excluded, height - c.created, &mut rng)
                .map_err(|e| match e {
                    botho::decoy_selection::DecoySelectionError::InsufficientCandidates {
                        ..
                    } => "selection_failure",
                    other => panic!("unexpected node selector error: {other}"),
                })?;
            let keys: BTreeSet<_> = selected.iter().map(|o| o.target_key).collect();
            if selected.len() != 19 || keys.len() != 19 || keys.iter().any(|k| excluded.contains(k))
            {
                return Err("invalid_selected_membership");
            }
            let age = selected
                .iter()
                .map(|output| selected_record_age(output, &pool))
                .max()
                .unwrap()
                .max(height - c.created);
            charges.push(reference::due(c.value, 1000, 1000, age, 200));
            total = total.checked_add(c.value).unwrap();
        }
        let fee = reference::fee(&charges, outputs, 2).ok_or("fee_overflow")?;
        let remaining = total.checked_sub(fee).ok_or("unaffordable")?;
        let minimum = LotteryDrawConfig::default().min_utxo_value;
        let values = if let Some(pay) = payment {
            let change = remaining.checked_sub(pay).ok_or("unaffordable")?;
            if pay < minimum || change < minimum {
                return Err("dust_or_unaffordable");
            }
            vec![(recipient, pay), (owner, change)]
        } else {
            let each = remaining / outputs as u64;
            if each < minimum {
                return Err("dust_or_unaffordable");
            }
            let mut v = vec![(owner, each); outputs];
            v.last_mut().unwrap().1 += remaining % outputs as u64;
            v
        };
        for id in inputs {
            self.coins.get_mut(id).unwrap().spent = true;
        }
        let ids = values
            .into_iter()
            .map(|(owner, v)| self.add(owner, v, height, false))
            .collect();
        Ok((fee, ids))
    }
}
// The production selector returns outputs without record metadata. Fixture-only
// memo identities preserve the exact selected record, including inherited keys.
pub(crate) fn selected_record_age(output: &TxOutput, pool: &[OutputCandidate]) -> u64 {
    let mut matches = pool
        .iter()
        .filter(|candidate| candidate.output.e_memo == output.e_memo);
    let candidate = matches.next().expect("selected fixture record must exist");
    assert!(matches.next().is_none(), "fixture identity must be unique");
    assert_eq!(
        &candidate.output, output,
        "selected output must match its tagged record"
    );
    candidate.age_blocks
}
