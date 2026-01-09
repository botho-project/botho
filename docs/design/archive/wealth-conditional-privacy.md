# Wealth-Conditional Privacy: Design Specification

## Status

**ARCHIVED** - Withdrawn

> **Why archived**: This proposal was based on the premise that cluster tags track
> "wealth concentration." Upon review, we clarified that cluster tags actually track
> **minting proximity** (where coins originated), not wealth. Since we're not tracking
> wealth, there's no justification for reducing privacy based on source_wealth.
>
> **See instead**: [Minting Proximity Fees](../minting-proximity-fees.md) for the
> corrected conceptual framing.

## Overview

This document proposes a privacy model where transaction amount visibility is conditioned on the sender's **source wealth** (cluster tag ancestry), not the transaction amount itself. This creates Sybil-resistant threshold privacy that reveals amounts only for transactions originating from concentrated wealth.

### Key Insight

Traditional threshold privacy fails because large transactions can be split into many small ones. Botho's cluster tag system tracks **provenance** (where coins came from), not denomination. Since source_wealth persists through splits, we can condition privacy on wealth origin rather than transaction size—defeating the Sybil attack that makes threshold privacy impossible in other systems.

```
Traditional threshold:              Botho's approach:

tx_amount > threshold?              source_wealth > threshold?
     │                                   │
     ├─ Yes → Transparent               ├─ Yes → Amount visible
     └─ No  → Private                   └─ No  → Fully private

Sybil attack:                       Sybil attack:
Split into small txs → SUCCEEDS     Split... but tags persist → FAILS
```

## Philosophy

### Why Not Absolute Privacy?

Botho's existing design already implements the philosophy that **concentrated wealth should bear additional costs**:

| Feature | How It Implements This Philosophy |
|---------|-----------------------------------|
| Progressive fees | Higher source_wealth → higher fees (1-15%) |
| Tag decay | Commerce diffuses wealth → rewards circulation |
| Burn mechanism | Fees removed from supply → deflationary pressure |

Wealth-conditional privacy is a logical extension: **privacy becomes another privilege that's subsidized for normal users but not for concentrated wealth**.

### Wealth Should Be Subject to Public Scrutiny

The position taken here is that:

1. **Recipients are always protected** — Recipients cannot control who sends them funds. Journalists, activists, merchants, whistleblower tip recipients all need permanent identity protection.

2. **Sender identity remains protected** — Ring signatures still hide who initiated a transaction, even when amounts are visible.

3. **Amounts from concentrated wealth become visible** — Large capital flows affect the entire economy. Market manipulation, whale movements, and concentrated wealth transfers have externalities that justify transparency.

4. **Normal users retain full privacy** — The vast majority of users (99%+) will never cross the threshold and maintain complete transaction privacy.

## Design

### Privacy Levels

| Level | Recipient | Amount | Sender | When Applied |
|-------|-----------|--------|--------|--------------|
| **Full Private** | Hidden (stealth) | Hidden | Hidden (ring) | source_wealth ≤ full_privacy_threshold |
| **Amount Visible** | Hidden (stealth) | **Visible** | Hidden (ring) | source_wealth ≥ transparency_threshold |
| **Probabilistic** | Hidden | Maybe visible | Hidden | Between thresholds |

**Critical**: Recipient identity is **always** protected via ML-KEM stealth addresses, regardless of source wealth. This is non-negotiable.

### Threshold Parameters

```rust
pub struct PrivacyPolicy {
    /// Source wealth at or below this: guaranteed full privacy
    /// Recommended: 10,000 BTH (~0.01% of mature supply)
    pub full_privacy_threshold: u64,

    /// Source wealth at or above this: amounts always visible
    /// Recommended: 100,000 BTH (~0.1% of mature supply)
    pub transparency_threshold: u64,
}
```

**Coverage Analysis** (at 100M BTH supply):

| Source Wealth | % of Supply | Privacy Level | Affected Users |
|---------------|-------------|---------------|----------------|
| ≤ 10,000 BTH | ≤ 0.01% | Full privacy | ~99.9% of users |
| 10,000 - 100,000 BTH | 0.01% - 0.1% | Probabilistic | ~0.09% of users |
| ≥ 100,000 BTH | ≥ 0.1% | Amount visible | ~0.01% of users |

### Probabilistic Zone

Between the thresholds, privacy is determined probabilistically:

```rust
impl PrivacyPolicy {
    pub fn determine_privacy(
        &self,
        source_wealth: u64,
        tx_entropy: [u8; 32],  // Derived from transaction hash
    ) -> OutputPrivacy {
        if source_wealth <= self.full_privacy_threshold {
            return OutputPrivacy::FullPrivate;
        }

        if source_wealth >= self.transparency_threshold {
            return OutputPrivacy::AmountVisible;
        }

        // Probabilistic zone: linear interpolation
        let range = self.transparency_threshold - self.full_privacy_threshold;
        let position = source_wealth - self.full_privacy_threshold;
        let p_transparent = position as f64 / range as f64;

        // Deterministic from tx entropy (verifiable by all nodes)
        let roll = u64::from_le_bytes(tx_entropy[0..8].try_into().unwrap());
        let threshold = (p_transparent * u64::MAX as f64) as u64;

        if roll < threshold {
            OutputPrivacy::AmountVisible
        } else {
            OutputPrivacy::FullPrivate
        }
    }
}
```

**Why probabilistic?**

1. Creates uncertainty for adversaries trying to structure transactions
2. Smooth transition avoids cliff effects at threshold boundaries
3. Deterministic from tx hash ensures all nodes agree on privacy level
4. Users cannot know in advance which transactions will be revealed

### What "Amount Visible" Means

When a transaction output has `AmountVisible` privacy:

```rust
pub struct TransparentOutput {
    // Still hidden (stealth address)
    pub one_time_destination: PublicKey,
    pub ml_kem_ciphertext: [u8; 1088],

    // Now visible (no Pedersen commitment)
    pub amount: u64,  // Plaintext amount in nanoBTH

    // Still present for balance verification
    pub range_proof: Option<Bulletproof>,  // None for transparent

    // Cluster tags (already visible for fee calculation)
    pub cluster_tags: ClusterTagVector,
}
```

**Balance verification** works because:
- Transparent outputs have known amounts
- Private outputs have Pedersen commitments
- Sum of inputs = Sum of outputs (mix of known + committed values)

## Sybil Resistance Analysis

### Attack 1: Splitting Before Threshold

**Strategy**: Whale with 1M BTH source_wealth splits into many UTXOs before transacting.

**Result**:
```
Original: 1 UTXO × 1,000,000 BTH, source_wealth = 1,000,000
After split: 1,000 UTXOs × 1,000 BTH each

Each UTXO's source_wealth = 1,000,000 (UNCHANGED)
All transactions remain in Amount Visible zone
```

**Attack defeated**: Source wealth persists through splits.

### Attack 2: Sybil Shuffle

**Strategy**: Create many sybil addresses, distribute coins to appear as many small holders.

**Result**:
```
Whale sends to 100 sybil addresses
Each sybil receives: 10,000 BTH with source_wealth = 1,000,000

Sybils transact: source_wealth still = 1,000,000
All sybil transactions remain Amount Visible
```

**Attack defeated**: Source wealth is inherited by recipients.

### Attack 3: Mixing with Low-Wealth Coins

**Strategy**: Combine whale coins with many small, low source_wealth coins to dilute.

**Attempt**:
```
Whale UTXO: 100,000 BTH, source_wealth = 1,000,000
Poor UTXOs: 100 × 100 BTH, source_wealth = 1,000 each

Combined source_wealth (value-weighted):
= (100,000 × 1,000,000 + 100 × 100 × 1,000) / (100,000 + 10,000)
= (100,000,000,000 + 10,000,000) / 110,000
= 909,181 BTH (still 91% of original!)
```

**Attack defeated**: Value-weighted blending means large UTXOs dominate.

### Attack 4: Slow Commerce Decay

**Strategy**: Patiently trade through many merchants to decay source_wealth below threshold.

**Result**:
```
Starting: source_wealth = 1,000,000 BTH
After 10 hops (5% decay each): source_wealth ≈ 600,000 BTH
After 20 hops: source_wealth ≈ 360,000 BTH
After 50 hops: source_wealth ≈ 77,000 BTH (approaches threshold!)
```

**This is acceptable**:
- 50 genuine economic transactions through diverse counterparties
- ~100 days minimum (age-gated decay at 12 eligible/day max)
- Actually represents real economic participation
- System is working as designed—circulating wealth earns privacy

### Attack 5: Transaction Structuring

**Strategy**: Time transactions to maximize probability of landing in private zone.

**Result**:
- Probability is determined by tx hash (unpredictable before signing)
- Even with repeated attempts, expected transparency rate matches wealth level
- Structuring costs fees on each attempt
- Statistical analysis would reveal structuring patterns

**Attack mitigated**: Probabilistic determination prevents gaming.

## Ring Signature Interactions

### The Problem

If some outputs are Amount Visible and others are Full Private, ring signature decoy selection becomes complex:

```
Ring member outputs:
  - Decoy 1: Amount Visible (amount = 5,000 BTH)
  - Decoy 2: Full Private (amount hidden)
  - Real:    Full Private (amount hidden)
  - Decoy 3: Amount Visible (amount = 12,000 BTH)
  ...
```

An observer might use visible amounts to eliminate decoys with implausible values.

### Solution: Segregated Anonymity Sets

Maintain separate pools for decoy selection:

```rust
pub enum OutputPool {
    /// Outputs where amount is visible
    Transparent,
    /// Outputs where amount is hidden
    Private,
}

impl DecoySelector {
    pub fn select_decoys(
        &self,
        real_output: &Output,
        ring_size: usize,
    ) -> Vec<Output> {
        // Only select decoys from the same pool as the real output
        let pool = if real_output.is_amount_visible() {
            OutputPool::Transparent
        } else {
            OutputPool::Private
        };

        self.select_from_pool(pool, ring_size - 1)
    }
}
```

**Properties**:
- Private outputs only use private decoys (no amount leakage)
- Transparent outputs use transparent decoys (amounts already public)
- Cluster-aware selection still applies within each pool
- Each pool maintains effective anonymity ≥ 10

### Anonymity Set Size Concerns

With segregated pools, we need sufficient outputs in each:

| Phase | Private Pool | Transparent Pool | Concern |
|-------|--------------|------------------|---------|
| Early network | Large | Small | Transparent users have small anonymity set |
| Mature network | Very large | Medium | Adequate anonymity for both |

**Mitigation for early phase**:
- Require minimum pool size before enabling transparency
- Fall back to full privacy if transparent pool < minimum_ring_size × 10

## Exchange Integration

### The Challenge

Exchanges need to comply with KYC/AML while operating within Botho's privacy model.

### Recommended Approach: Sidechannel Verification

Similar to MobileCoin's exchange integration:

1. **Account Linking**: User proves ownership of a Botho address to exchange via signed challenge
2. **View Key Sharing**: User optionally shares view key with exchange for deposit verification
3. **Identity Sidechannel**: Exchange maintains off-chain mapping of verified identities to addresses
4. **Compliance**: Large movements through exchange are already subject to exchange's KYC

```
┌─────────────────────────────────────────────────────────────────┐
│                    EXCHANGE INTEGRATION                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   User ──────── Sidechannel ──────── Exchange                   │
│     │           (off-chain)            │                        │
│     │  1. Prove address ownership      │                        │
│     │  2. Complete KYC                 │                        │
│     │  3. Share view key (optional)    │                        │
│     │                                  │                        │
│     └──────── On-chain ───────────────┘                        │
│              (privacy preserved)                                 │
│                                                                  │
│   Exchange tracks:                                              │
│   - Which addresses are KYC'd                                   │
│   - Deposit/withdrawal history (via view key)                   │
│   - Compliance reporting (fiat side)                            │
│                                                                  │
│   Botho chain sees:                                             │
│   - Normal transactions (privacy intact)                        │
│   - No special exchange address types                           │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Exchange Compliance Notes

- Exchanges already report large fiat movements
- Transparent amounts for whale transactions provide on-chain auditability
- Exchange's own wallets (high source_wealth from concentrated deposits) naturally fall into transparent zone
- No special "exchange mode" needed—system works naturally

## Implementation Considerations

### Consensus Rules

All nodes must agree on privacy level for each output:

```rust
impl Block {
    pub fn verify_privacy_levels(&self) -> Result<(), Error> {
        for tx in &self.transactions {
            for output in &tx.outputs {
                let expected = PrivacyPolicy::default().determine_privacy(
                    output.source_wealth(),
                    tx.hash().as_bytes(),
                );

                if output.privacy_level() != expected {
                    return Err(Error::InvalidPrivacyLevel);
                }
            }
        }
        Ok(())
    }
}
```

### Output Structure

```rust
pub struct TxOutput {
    // Always present (stealth address components)
    pub one_time_key: CompressedRistretto,
    pub ml_kem_ciphertext: MlKemCiphertext,

    // Privacy-dependent amount representation
    pub amount_data: AmountData,

    // Always present (for fee calculation and privacy determination)
    pub cluster_tags: ClusterTagVector,

    // Present only for private outputs
    pub range_proof: Option<Bulletproof>,
}

pub enum AmountData {
    /// Amount hidden in Pedersen commitment
    Committed {
        commitment: CompressedRistretto,
        encrypted_amount: EncryptedAmount,  // For recipient
    },
    /// Amount visible to all
    Transparent {
        amount: u64,
    },
}
```

### Transaction Size Impact

| Output Type | Size | Components |
|-------------|------|------------|
| Full Private | ~1.2 KB | Commitment + Range proof + Encrypted amount |
| Amount Visible | ~0.4 KB | Plaintext amount (no proof needed) |

**Transparent outputs are smaller** because they don't need range proofs. This creates a minor incentive toward transparency for high-wealth users (lower fees per byte), which aligns with system goals.

### Migration Path

1. **Phase 1**: Implement segregated output pools, no transparency yet
2. **Phase 2**: Enable probabilistic transparency with conservative thresholds
3. **Phase 3**: Adjust thresholds based on observed behavior

## Parameter Governance

### Fixed Parameters (Fork to Change)

Like fee curve parameters and lottery parameters, privacy thresholds are fixed in the protocol and require a fork to modify:

```rust
pub const PRIVACY_POLICY: PrivacyPolicy = PrivacyPolicy {
    full_privacy_threshold: 10_000_000_000_000,    // 10,000 BTH in picocredits
    transparency_threshold: 100_000_000_000_000,   // 100,000 BTH in picocredits
};
```

**Rationale for fixed parameters**:
- Predictability for users and businesses
- Prevents gaming through anticipation of parameter changes
- Community consensus required for changes
- Matches governance model for other economic parameters

### Potential Future Adjustments

| Scenario | Possible Response |
|----------|-------------------|
| Thresholds too low (too many transparent) | Fork to raise thresholds |
| Thresholds too high (whales escape) | Fork to lower thresholds |
| Supply inflation changes equilibrium | Scale thresholds with supply |
| Privacy pool too small | Temporarily disable transparency |

## Security Considerations

### Threat: Timing Analysis

**Attack**: Observer notes which transactions become transparent, infers wealth levels.

**Mitigation**:
- Wealth level is already partially inferrable from cluster tags (visible for fees)
- Probabilistic zone creates uncertainty
- This is an acceptable tradeoff given the design philosophy

### Threat: Amount Correlation

**Attack**: Use visible amounts to correlate transactions across time.

**Mitigation**:
- Only amounts are visible, not recipient identity
- Ring signatures still hide sender
- Cluster tags already create some correlation (existing system)

### Threat: Pool Size Attacks

**Attack**: Flood one pool to reduce anonymity in the other.

**Mitigation**:
- Monitor pool sizes
- Minimum pool size requirements
- Fee economics make flooding expensive

## Comparison with Alternatives

| Approach | Sybil Resistant | Privacy Preserved | Complexity |
|----------|-----------------|-------------------|------------|
| No threshold (current) | N/A | Full | Low |
| Amount threshold | No | Partial | Low |
| **Source-wealth threshold** | **Yes** | **Graduated** | **Medium** |
| Time-delayed transparency | Partial | Temporary | Medium |
| Regulatory compliance mode | N/A | Opt-in | High |

## Open Questions

1. **Should transparent outputs get fee discounts?** They're smaller and provide public goods (transparency). Counter: might incentivize whales to embrace transparency rather than work toward privacy.

2. **How does this interact with bridges?** Bridge contracts likely have high source_wealth. This may be acceptable (bridge flows should be public) or require special handling.

3. **Should there be a "voluntary transparency" option?** Allow users to opt into transparency below threshold for public accountability (DAOs, charities, public figures).

4. **Testnet threshold experimentation?** Run testnet with various thresholds to observe behavioral effects before mainnet deployment.

## Summary

Wealth-conditional privacy leverages Botho's existing cluster tag infrastructure to create a Sybil-resistant threshold privacy system:

- **Normal users** (99.9%+): Full transaction privacy, always
- **Wealthy users** (0.1%): Amount visibility, recipient still protected
- **Intermediate zone**: Probabilistic privacy based on source wealth
- **Sybil resistant**: Splitting and shuffling don't reduce source_wealth
- **Philosophically consistent**: Extends progressive fee philosophy to privacy

This design acknowledges that **absolute privacy for concentrated wealth has social costs**, while protecting the privacy of normal users and always protecting recipient identity.

## References

- [Progressive Fees](../concepts/progressive-fees.md) - Existing wealth-based fee system
- [Cluster Tag Decay](cluster-tag-decay.md) - How source_wealth evolves over time
- [Privacy](../concepts/privacy.md) - Current privacy architecture
- [Tokenomics](../concepts/tokenomics.md) - Economic model context
- MobileCoin Exchange Integration - Prior art for sidechannel KYC

## Changelog

- 2026-01-05: Initial proposal
