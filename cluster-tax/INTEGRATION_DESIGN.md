# Cluster Taxation Integration Design

## Overview

This document specifies how cluster taxation integrates with the existing MobileCoin transaction infrastructure. The design is phased:

- **Phase 1**: Public tags (simpler, proves the concept)
- **Phase 2**: Committed tags with ZK proofs (full privacy)

---

## Phase 1: Public Tags

### 1.1 Extended TxOut Structure

```protobuf
// In external.proto or new file
message TxOut {
    // Existing fields (tags 1-6)
    oneof masked_amount { ... }
    CompressedRistretto target_key = 2;
    CompressedRistretto public_key = 3;
    EncryptedFogHint e_fog_hint = 4;
    EncryptedMemo e_memo = 5;

    // NEW: Cluster tags (Phase 1 - public)
    ClusterTagVector cluster_tags = 7;
}

message ClusterTagVector {
    // Number of valid entries (max 16)
    uint32 count = 1;

    // Cluster IDs and corresponding weights
    // Weights are in parts per million (1,000,000 = 100%)
    repeated uint64 cluster_ids = 2;
    repeated uint32 weights = 3;
}
```

### 1.2 Rust Type Changes

```rust
// In transaction/types/src/cluster_tags.rs (new file)

use mc_crypto_digestible::Digestible;
use prost::Message;
use serde::{Deserialize, Serialize};

pub const MAX_CLUSTER_TAGS: usize = 16;
pub const TAG_WEIGHT_SCALE: u32 = 1_000_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Digestible, Message, Serialize, Deserialize)]
pub struct ClusterTagVector {
    #[prost(uint32, tag = "1")]
    pub count: u32,

    #[prost(uint64, repeated, tag = "2")]
    pub cluster_ids: Vec<u64>,

    #[prost(uint32, repeated, tag = "3")]
    pub weights: Vec<u32>,
}

impl ClusterTagVector {
    /// Create tags for a newly minted coin (100% to new cluster)
    pub fn new_coinbase(cluster_id: u64) -> Self {
        Self {
            count: 1,
            cluster_ids: vec![cluster_id],
            weights: vec![TAG_WEIGHT_SCALE],
        }
    }

    /// Compute total attributed weight
    pub fn total_weight(&self) -> u32 {
        self.weights.iter().take(self.count as usize).sum()
    }

    /// Get weight for a specific cluster
    pub fn get_weight(&self, cluster_id: u64) -> u32 {
        for i in 0..self.count as usize {
            if self.cluster_ids[i] == cluster_id {
                return self.weights[i];
            }
        }
        0
    }
}
```

```rust
// Modified TxOut in transaction/core/src/tx.rs

#[derive(Clone, Deserialize, Digestible, Eq, Hash, Message, PartialEq, Serialize, Zeroize)]
pub struct TxOut {
    #[prost(oneof = "MaskedAmount", tags = "1, 6")]
    #[digestible(name = "amount")]
    pub masked_amount: Option<MaskedAmount>,

    #[prost(message, required, tag = "2")]
    pub target_key: CompressedRistrettoPublic,

    #[prost(message, required, tag = "3")]
    pub public_key: CompressedRistrettoPublic,

    #[prost(message, required, tag = "4")]
    pub e_fog_hint: EncryptedFogHint,

    #[prost(message, tag = "5")]
    pub e_memo: Option<EncryptedMemo>,

    // NEW: Cluster tags for progressive fee computation
    #[prost(message, tag = "7")]
    pub cluster_tags: Option<ClusterTagVector>,
}
```

### 1.3 Cluster Wealth State

The cluster wealth state must be maintained by validators:

```rust
// In a new crate: cluster-tax-state or in ledger/db

use std::collections::HashMap;

/// Global cluster wealth tracking
/// Stored in the ledger database, updated with each block
pub struct ClusterWealthState {
    /// Map from cluster_id to total wealth attributed to that cluster
    wealths: HashMap<u64, u64>,

    /// Merkle root for efficient verification (optional)
    merkle_root: [u8; 32],
}

impl ClusterWealthState {
    /// Apply a transaction's effect on cluster wealths
    pub fn apply_transaction(
        &mut self,
        inputs: &[TxOut],
        outputs: &[TxOut],
        input_values: &[u64],  // Decrypted by validator
        output_values: &[u64], // Decrypted by validator
        decay_rate: u32,
    ) {
        // For each input, subtract its tag masses from cluster wealths
        for (input, &value) in inputs.iter().zip(input_values.iter()) {
            if let Some(tags) = &input.cluster_tags {
                for i in 0..tags.count as usize {
                    let cluster = tags.cluster_ids[i];
                    let weight = tags.weights[i];
                    let mass = value * weight as u64 / TAG_WEIGHT_SCALE as u64;
                    *self.wealths.entry(cluster).or_insert(0) -= mass;
                }
            }
        }

        // For each output, add its tag masses (after decay) to cluster wealths
        let decay_factor = TAG_WEIGHT_SCALE - decay_rate;
        for (output, &value) in outputs.iter().zip(output_values.iter()) {
            if let Some(tags) = &output.cluster_tags {
                for i in 0..tags.count as usize {
                    let cluster = tags.cluster_ids[i];
                    let weight = tags.weights[i];
                    // Note: output tags should already reflect decay
                    let mass = value * weight as u64 / TAG_WEIGHT_SCALE as u64;
                    *self.wealths.entry(cluster).or_insert(0) += mass;
                }
            }
        }
    }

    /// Get fee rate for a cluster
    pub fn fee_rate_bps(&self, cluster_id: u64, fee_curve: &FeeCurve) -> u32 {
        let wealth = self.wealths.get(&cluster_id).copied().unwrap_or(0);
        fee_curve.rate_bps(wealth)
    }
}
```

### 1.4 Transaction Validation

Add new validation function in `transaction/core/src/validation/validate.rs`:

```rust
/// Validate cluster tag inheritance and progressive fee
pub fn validate_cluster_tags(
    tx: &Tx,
    input_tx_outs: &[TxOut],
    input_values: &[u64],
    cluster_wealth: &ClusterWealthState,
    fee_curve: &FeeCurve,
    decay_rate: u32,
) -> TransactionValidationResult<()> {
    // 1. Compute input tag masses
    let mut input_masses: HashMap<u64, u64> = HashMap::new();
    let mut total_input_value: u64 = 0;

    for (tx_out, &value) in input_tx_outs.iter().zip(input_values.iter()) {
        total_input_value += value;
        if let Some(tags) = &tx_out.cluster_tags {
            for i in 0..tags.count as usize {
                let mass = value * tags.weights[i] as u64 / TAG_WEIGHT_SCALE as u64;
                *input_masses.entry(tags.cluster_ids[i]).or_insert(0) += mass;
            }
        }
    }

    // 2. Compute output tag masses
    let mut output_masses: HashMap<u64, u64> = HashMap::new();
    let mut total_output_value: u64 = 0;

    for tx_out in tx.prefix.outputs.iter() {
        let value = tx_out.get_value()?; // Need access to decrypted value
        total_output_value += value;
        if let Some(tags) = &tx_out.cluster_tags {
            for i in 0..tags.count as usize {
                let mass = value * tags.weights[i] as u64 / TAG_WEIGHT_SCALE as u64;
                *output_masses.entry(tags.cluster_ids[i]).or_insert(0) += mass;
            }
        }
    }

    // 3. Verify tag mass conservation with decay
    let decay_factor = TAG_WEIGHT_SCALE - decay_rate;
    for (&cluster, &input_mass) in &input_masses {
        let expected = input_mass * decay_factor as u64 / TAG_WEIGHT_SCALE as u64;
        let actual = output_masses.get(&cluster).copied().unwrap_or(0);
        let tolerance = (input_mass / 1000).max(1);

        if actual > expected + tolerance {
            return Err(TransactionValidationError::ClusterTagInflation {
                cluster,
                expected,
                actual,
            });
        }
    }

    // 4. Compute and verify progressive fee
    let declared_fee = total_input_value - total_output_value;
    let required_fee = compute_required_fee(
        &input_masses,
        total_input_value,
        total_output_value,
        cluster_wealth,
        fee_curve,
    );

    if declared_fee < required_fee {
        return Err(TransactionValidationError::InsufficientProgressiveFee {
            required: required_fee,
            actual: declared_fee,
        });
    }

    Ok(())
}

fn compute_required_fee(
    input_masses: &HashMap<u64, u64>,
    total_input: u64,
    transfer_amount: u64,
    cluster_wealth: &ClusterWealthState,
    fee_curve: &FeeCurve,
) -> u64 {
    let mut weighted_rate: u128 = 0;

    for (&cluster, &mass) in input_masses {
        let rate = cluster_wealth.fee_rate_bps(cluster, fee_curve) as u128;
        weighted_rate += mass as u128 * rate;
    }

    // Add background contribution
    let total_mass: u64 = input_masses.values().sum();
    let background_mass = total_input - total_mass;
    weighted_rate += background_mass as u128 * fee_curve.background_rate_bps as u128;

    // Effective rate
    let effective_rate = weighted_rate / total_input as u128;

    // Required fee
    (transfer_amount as u128 * effective_rate / 10_000) as u64
}
```

### 1.5 Transaction Building

Extend `transaction/builder` to compute output tags:

```rust
// In transaction/builder/src/lib.rs

impl TransactionBuilder {
    /// Build transaction with cluster tag inheritance
    pub fn build_with_tags(
        &mut self,
        ring_members: &[TxOut],
        real_input_indices: &[usize],
        decay_rate: u32,
    ) -> Result<Tx, BuildError> {
        // 1. Get real inputs and their tags
        let real_inputs: Vec<&TxOut> = real_input_indices
            .iter()
            .map(|&i| &ring_members[i])
            .collect();

        // 2. Compute output tags with proper inheritance
        let output_values: Vec<u64> = self.outputs.iter()
            .map(|o| o.amount.value)
            .collect();

        let output_tags = compute_inherited_tags(
            &real_inputs,
            &output_values,
            decay_rate,
        );

        // 3. Attach tags to outputs
        for (output, tags) in self.outputs.iter_mut().zip(output_tags.into_iter()) {
            output.cluster_tags = Some(tags);
        }

        // 4. Build rest of transaction normally
        self.build()
    }
}

fn compute_inherited_tags(
    inputs: &[&TxOut],
    output_values: &[u64],
    decay_rate: u32,
) -> Vec<ClusterTagVector> {
    // Aggregate input tag masses
    let mut input_masses: HashMap<u64, u64> = HashMap::new();
    let mut total_input: u64 = 0;

    for input in inputs {
        let value = input.get_value().unwrap_or(0);
        total_input += value;

        if let Some(tags) = &input.cluster_tags {
            for i in 0..tags.count as usize {
                let mass = value * tags.weights[i] as u64 / TAG_WEIGHT_SCALE as u64;
                *input_masses.entry(tags.cluster_ids[i]).or_insert(0) += mass;
            }
        }
    }

    // Apply decay
    let decay_factor = TAG_WEIGHT_SCALE - decay_rate;
    let decayed_masses: HashMap<u64, u64> = input_masses
        .into_iter()
        .map(|(c, m)| (c, m * decay_factor as u64 / TAG_WEIGHT_SCALE as u64))
        .collect();

    // Distribute to outputs proportionally
    let total_output: u64 = output_values.iter().sum();

    output_values
        .iter()
        .map(|&out_value| {
            let mut tags = ClusterTagVector::default();

            if out_value == 0 || total_output == 0 {
                return tags;
            }

            // Sort by mass descending, take top MAX_CLUSTER_TAGS
            let mut entries: Vec<_> = decayed_masses
                .iter()
                .map(|(&cluster, &mass)| {
                    let out_mass = mass * out_value / total_output;
                    let weight = (out_mass * TAG_WEIGHT_SCALE as u64 / out_value) as u32;
                    (cluster, weight)
                })
                .filter(|(_, w)| *w >= 1000) // Min 0.1%
                .collect();

            entries.sort_by(|a, b| b.1.cmp(&a.1));
            entries.truncate(MAX_CLUSTER_TAGS);

            tags.count = entries.len() as u32;
            tags.cluster_ids = entries.iter().map(|(c, _)| *c).collect();
            tags.weights = entries.iter().map(|(_, w)| *w).collect();

            tags
        })
        .collect()
}
```

### 1.6 Files to Modify

| File | Change |
|------|--------|
| `transaction/types/src/lib.rs` | Add `mod cluster_tags` |
| `transaction/types/src/cluster_tags.rs` | New file: ClusterTagVector type |
| `transaction/core/src/tx.rs` | Add `cluster_tags` field to TxOut |
| `transaction/core/src/validation/error.rs` | Add new error variants |
| `transaction/core/src/validation/validate.rs` | Add `validate_cluster_tags()` |
| `transaction/builder/src/lib.rs` | Add tag inheritance in build |
| `ledger/db/src/lib.rs` | Add ClusterWealthState storage |
| `consensus/service/src/...` | Pass cluster wealth to validation |
| `api/proto/external.proto` | Add ClusterTagVector message |

---

## Phase 2: Committed Tags (Future)

### 2.1 Committed Tag Structure

Instead of public tags, each output contains:

```rust
pub struct TxOut {
    // ... existing fields ...

    /// Committed tag masses: C_k = (v * w_k) * H + r_k * G
    /// The cluster_id is public, but the mass is hidden
    #[prost(message, repeated, tag = "7")]
    pub committed_tag_masses: Vec<CommittedTagMass>,

    /// Encrypted tag vector for recipient
    #[prost(bytes, tag = "8")]
    pub encrypted_tags: Vec<u8>,
}

pub struct CommittedTagMass {
    pub cluster_id: u64,
    pub commitment: CompressedCommitment, // Pedersen commitment to (value * weight)
}
```

### 2.2 ZK Proof Requirements

The transaction must include proofs:

```rust
pub struct TagProofBundle {
    /// Proves output tag masses = (1-λ) × input tag masses for each cluster
    pub inheritance_proof: TagInheritanceProof,

    /// Proves fee ≥ f(input_tags)
    pub fee_correctness_proof: FeeCorrectnessProof,
}
```

### 2.3 Ring Signature Compatibility

**Challenge:** MLSAG hides which input is real, but fee computation needs real input tags.

**Solution Options:**

1. **Proof over ring:** Prove "∃ member in ring whose tags justify fee"
   - Complex: O(ring_size) proof complexity
   - Full privacy

2. **Aggregate statistics:** Reveal aggregate tag distribution without linking
   - Partial privacy: reveals input composition but not which ring member

3. **Conservative fee:** Pay max fee rate across all ring members
   - Simple but overpays
   - Could be acceptable for small rings

**Recommended approach for Phase 2:**

Use a SNARK/STARK that proves:
1. I know a valid opening of one input commitment in the ring
2. That input's tags justify the claimed fee
3. Output tags correctly inherit from that input

This keeps ring signature privacy while enabling progressive fees.

---

## Migration Path

### Block Version Upgrade

1. **Version N**: Current (no cluster tags)
2. **Version N+1**: Cluster tags optional (backwards compatible)
3. **Version N+2**: Cluster tags required for all new outputs

### Coinbase Handling

New blocks create coinbase outputs with fresh cluster IDs:

```rust
fn create_coinbase_output(block_height: u64, ...) -> TxOut {
    // Cluster ID derived from block hash for uniqueness
    let cluster_id = hash_to_u64(block_hash, block_height);

    TxOut {
        // ... normal fields ...
        cluster_tags: Some(ClusterTagVector::new_coinbase(cluster_id)),
    }
}
```

---

## Open Questions

1. **How to handle pre-existing outputs without tags?**
   - Option A: Treat as 100% background (lowest fee rate)
   - Option B: Assign to a "legacy" cluster
   - Option C: Require migration transaction

2. **Cluster ID collision resistance?**
   - Using 64-bit IDs with block hash derivation should be sufficient
   - Could use 128-bit if needed

3. **Storage overhead?**
   - Phase 1: ~100 bytes per output for tags
   - Phase 2: ~500 bytes per output for commitments + proof share

4. **Validation performance?**
   - Phase 1: O(num_clusters) per transaction, negligible
   - Phase 2: ZK proof verification ~10-100ms depending on system

---

## Next Steps

1. [ ] Implement Phase 1 in a feature branch
2. [ ] Add integration tests with mock cluster wealth state
3. [ ] Benchmark validation overhead
4. [ ] Design ZK circuits for Phase 2
5. [ ] Economic simulation validation (Track A)
