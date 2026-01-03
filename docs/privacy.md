# Privacy Features

Botho provides strong transaction privacy through a combination of cryptographic techniques, designed from the ground up with post-quantum security where it matters most.

## Overview

| Privacy Goal | Technique | Quantum Safety | Effectiveness |
|--------------|-----------|----------------|---------------|
| Hide recipient | ML-KEM-768 stealth addresses | **PQ-safe** | Perfect |
| Hide amounts | Pedersen commitments + Bulletproofs | **PQ-safe** (hiding) | Perfect |
| Hide sender | CLSAG ring signatures (ring=20) | Classical | ~10+ effective anonymity |
| Secure communication | Encrypted memos (AES-256-CTR) | Classical | Perfect |

**Privacy architecture**: Botho prioritizes post-quantum protection for **recipient identity** and **amount privacy** because these are permanent (on-chain forever). Sender anonymity uses classical CLSAG ring signatures, which provides excellent present-day privacy while keeping transactions small and accessible. See [Why This Architecture?](#why-this-architecture) for the detailed rationale.

## Transaction Types

Botho supports two transaction types. See [Transaction Types](transactions.md) for complete details.

| Type | Recipient | Amount | Sender | Quantum Safety | Use Case |
|------|-----------|--------|--------|----------------|----------|
| **Minting** | Hidden | Public | Known | Full | Block rewards |
| **Private** | Hidden | Hidden | Hidden (CLSAG) | Recipient + Amount | All transfers |

## Stealth Addresses

Every transaction creates a **unique one-time destination address**, ensuring that:

- Outside observers cannot link multiple payments to the same recipient
- The blockchain reveals no information about who received funds
- Only the recipient can identify their incoming transactions

### How It Works

Botho implements post-quantum stealth addresses using ML-KEM-768:

1. **Sender creates transaction:**
   - Recipient publishes view public key `V` and spend public key `S`
   - Sender encapsulates shared secret: `(ciphertext, shared_secret) = ML-KEM.Encapsulate(V)`
   - Computes one-time destination key: `P = H(shared_secret)*G + S`

2. **Transaction is published:**
   - Contains the one-time key `P` and the ML-KEM ciphertext
   - `P` looks random to everyone except the recipient

3. **Recipient scans blockchain:**
   - Decapsulates: `shared_secret = ML-KEM.Decapsulate(ciphertext, view_secret_key)`
   - Computes `P' = H(shared_secret)*G + S`
   - If `P' == P`, the transaction belongs to them
   - Spending key: `x = H(shared_secret) + spend_secret_key`

```mermaid
sequenceDiagram
    participant S as Sender
    participant B as Blockchain
    participant R as Recipient

    Note over R: Publishes V (view key) and S (spend key)

    S->>S: Encapsulate shared secret using V
    S->>S: Compute one-time address P = H(ss)*G + S
    S->>B: Publish (P, ciphertext, amount)

    R->>B: Scan new transactions
    B-->>R: Return transaction with ciphertext
    R->>R: Decapsulate shared secret using view key
    R->>R: Compute P' = H(ss)*G + S
    alt P' matches P
        R->>R: Transaction is mine!
        R->>R: Derive spend key x = H(ss) + s
    else No match
        R->>R: Not my transaction
    end
```

### Post-Quantum Security

Unlike classical ECDH-based stealth addresses, ML-KEM provides:

- **~192-bit post-quantum security**: Resistant to Shor's algorithm
- **No classical fallback**: Pure PQ design, not hybrid
- **Forward secrecy**: Compromised view key doesn't reveal past shared secrets

### Subaddresses

Botho supports subaddresses for enhanced privacy:

- **Index 0**: Default receiving subaddress
- **Index 1**: Change subaddress (for transaction change outputs)

All subaddresses derive from the same mnemonic but are cryptographically unlinkable.

## Progressive Transaction Fees

Botho implements a novel **provenance-based progressive fee** system designed to reduce wealth concentration without sacrificing privacy or enabling Sybil attacks.

![Whale vs Poor Fees](images/cluster-tax/whale_vs_poor.png)

### How It Works

Transaction fees are based on coin *ancestry* (source_wealth), not account identity:

1. **Source Wealth**: Every UTXO tracks the wealth of its original minter
2. **Persistence**: Splitting doesn't change source_wealth—provenance tags persist
3. **Blending**: Combining UTXOs creates a value-weighted average
4. **Progressive Rate**: Fee rate increases with source_wealth via 3-segment curve (1% → 15%)

### Why It's Sybil-Resistant

Splitting transactions or creating multiple accounts doesn't reduce fees because:

- Fee rate depends on **source_wealth**, not transaction size or account count
- All UTXOs from the same origin retain the same provenance tag
- The only way to reduce fees is through genuine economic activity that diffuses coins

### Tag Decay

Tags decay by ~5% per transaction hop:

- Coins that circulate widely pay lower fees over time
- Hoarded coins retain high source_wealth → pay higher fees
- ~10 transaction hops through merchants reduces source_wealth by 90%

### Parameters

| Parameter | Value | Description |
|-----------|-------|-------------|
| Poor segment | 0-15% of max | 1% flat rate |
| Middle segment | 15-70% of max | 2% to 10% linear |
| Rich segment | 70%+ of max | 15% flat rate |
| Decay rate | 5% per hop | Tag decay per transaction |

> **See also**: [Progressive Fees](progressive-fees.md) for detailed analysis, simulation results, and ZK compatibility.

### Tag Vector Limits

To bound storage and reduce fingerprinting, cluster tag vectors are truncated:

| Parameter | Value | Description |
|-----------|-------|-------------|
| Max entries | 16 | Maximum clusters tracked per output |
| Min weight | 0.1% | Weights below this are pruned to "background" |
| Truncation | By weight | Lowest-weight entries dropped first |

Weights below 0.1% (1000 in parts-per-million) become "background" — unattributed ancestry that's indistinguishable across outputs. This ensures old ancestry eventually disappears and tag vectors don't grow unbounded.

## Ring Signatures

Ring signature transactions hide the true sender among a group of possible signers.

| Scheme | Ring Size | Signature Size | Quantum Safety |
|--------|-----------|----------------|----------------|
| **CLSAG** | 20 | ~700 bytes | Classical |

> **Note**: Ring signatures are used in all private transactions. Minting transactions use ML-DSA signatures (minter is known).

### How Ring Signatures Work

1. **Decoy Selection**: When spending, the sender selects 19 decoy outputs from the blockchain
2. **Ring Construction**: Creates a ring of 20 possible signers (1 real + 19 decoys)
3. **Signing**: Produces a signature proving ownership of ONE ring member without revealing which
4. **Verification**: Anyone can verify the signature is valid, but cannot determine the true signer

```mermaid
flowchart TB
    subgraph Ring["Ring of 20 Possible Signers"]
        direction LR
        D1[Decoy 1]
        D2[Decoy 2]
        D3[...]
        R[Real Signer]
        D4[...]
        D5[Decoy 19]
    end

    Ring --> SIG[Ring Signature]
    SIG --> KI[Key Image]

    KI --> V{Verifier}
    V -->|Valid + New Key Image| ACCEPT[✓ Accept]
    V -->|Duplicate Key Image| REJECT[✗ Reject<br/>Double Spend]

    style R fill:#22c55e,color:#000
    style ACCEPT fill:#22c55e,color:#000
    style REJECT fill:#ef4444,color:#fff
```

**Key insight**: The verifier knows the signature is valid (one ring member signed) but cannot determine *which* member. The key image prevents double-spending without revealing the signer.

### CLSAG

CLSAG (Concise Linkable Spontaneous Anonymous Group) is an efficient ring signature:

```
Ring = [decoy_1, ..., real_input, ..., decoy_19]  (shuffled, 20 total)
Signature = CLSAG.sign(ring, real_index, secret_key)
KeyImage = x * Hp(P)  // 32 bytes
```

- **45% smaller than MLSAG** through response aggregation
- **Based on curve25519** (discrete log security)
- **~128-bit classical security**
- **Ring size 20** (larger than Monero's 16)

### Benefits

- **Sender Unlinkability**: Observers cannot determine which input is being spent
- **Plausible Deniability**: Any of the 20 ring members could be the true sender
- **No Coordination**: Decoys don't know they're being used
- **Linkable**: Key images prevent double-spending without revealing the signer
- **Compact**: ~700 bytes per input enables desktop-friendly blockchain growth

### Cluster Tag Privacy Considerations

Botho's progressive fee system uses **cluster tags** to track coin ancestry for wealth-based taxation. These tags are visible on transaction outputs, which creates a potential privacy consideration for ring signatures.

#### The Challenge

When a ring signature transaction creates outputs, the cluster tags on those outputs are derived from the input's tags (with 5% decay). An observer could potentially:

1. Examine the ring of 20 possible inputs
2. Compare each input's cluster tags to the output's tags
3. Identify which input's tags, after decay, best match the output

If only one ring member's tags produce a plausible output pattern, the ring signature anonymity is reduced.

#### Example Attack Scenario

```
Ring Member A: {cluster_17: 0.80, cluster_42: 0.15}
Ring Member B: {cluster_3: 0.95}
Ring Member C: {cluster_17: 0.40, cluster_42: 0.40}
... (17 more members with different patterns)

Output tags:  {cluster_17: 0.76, cluster_42: 0.14}  (after 5% decay)
```

An observer calculates: 0.80 × 0.95 = 0.76, 0.15 × 0.95 ≈ 0.14 — only Ring Member A matches!

#### Mitigation: Cluster-Aware Decoy Selection

Botho's OSPEAD algorithm addresses cluster fingerprinting by selecting decoys with **similar cluster tag profiles** (≥70% cosine similarity). When all ring members have comparable tag patterns, the fingerprinting attack fails because multiple members produce plausible outputs.

With ring size 20 and cluster-aware selection, we achieve **10+ effective anonymity** even against sophisticated adversaries using cluster fingerprinting attacks.

#### Running Your Own Simulations

The privacy simulation is available as a CLI tool:

```bash
cargo run -p bth-cluster-tax --features cli --release --bin cluster-tax-sim -- privacy \
  -n 10000 \
  --pool-size 100000 \
  --ring-size 20 \
  --standard-fraction 0.50 \
  --decay-rate 5.0 \
  --cluster-aware \
  --min-similarity 0.70
```

#### Design Philosophy

This creates an intentional correlation between wealth concentration and privacy:

- **Diffuse clusters** (low fees): Coins have circulated widely, tags are mixed, privacy is strong
- **Concentrated clusters** (high fees): Tags are distinctive, privacy is slightly reduced

This aligns with Botho's progressive philosophy—privacy is marginally more expensive for concentrated wealth. Users seeking maximum privacy are incentivized to circulate their coins, which diffuses cluster tags over time.

## Transaction Types and Fees

Botho supports two transaction types. See [Transaction Types](transactions.md) for complete technical details.

### Transaction Types

| Type | Amount Privacy | Sender Privacy | Signature | Use Case |
|------|---------------|----------------|-----------|----------|
| Minting | Public | Known (minter) | ML-DSA | Block rewards |
| Private | Hidden | Hidden (CLSAG ring=20) | CLSAG | All transfers |

### Fee Structure

Botho uses size-based fees: `fee = fee_per_byte × tx_size × cluster_factor`

| Transaction Type | Signature Size | Typical Total Size | Fee (1x cluster) |
|-----------------|----------------|-------------------|------------------|
| Minting | ~3.3 KB (ML-DSA) | ~1.5 KB | 0 |
| Private | ~0.7 KB (CLSAG) | ~4 KB | ~4,000 nanoBTH |

**Why these sizes?**

- **Minting transactions** have no fee (they create coins, not transfer them)
- **Private transactions** use CLSAG ring signatures (~700 bytes per input)

Size-based fees naturally reflect the network resources each transaction type consumes. The compact CLSAG signatures keep transactions small, enabling desktop-friendly blockchain growth (~100 GB/year at moderate throughput).

### Fee Calculation

All fees follow this formula:

```
total_fee = fee_per_byte * tx_size * cluster_factor
```

Where:
- `fee_per_byte` = 1 nanoBTH per byte (default)
- `tx_size` = transaction size in bytes
- `cluster_factor` = progressive multiplier (1x to 6x) based on sender's cluster wealth

See [Tokenomics](tokenomics.md) for details on cluster-based progressive fees.

## Confidential Amounts

All transaction types except Minting hide amounts using Pedersen commitments and Bulletproofs range proofs.

### How It Works

1. **Pedersen Commitments**: Amounts are encoded as `C = v*H + b*G` where `v` is the amount and `b` is a blinding factor
2. **Balance Proof**: Proves that inputs equal outputs without revealing values (homomorphic property)
3. **Range Proofs**: Bulletproofs prove amounts are in valid range `[0, 2^64)` without revealing them

### Benefits

- **Amount Privacy**: Observers cannot see how much is being transferred
- **Verifiable**: Network can still verify no coins are created from nothing
- **Compact**: Bulletproofs provide efficient, aggregatable range proofs

### Security Note

Pedersen commitments use classical elliptic curves. The **hiding** property is information-theoretic (unconditionally secure), but the **binding** property could theoretically be broken by quantum computers. This means:

- **Amounts remain hidden** even against quantum adversaries
- A quantum attacker could potentially forge invalid proofs, but not reveal hidden amounts
- This is an acceptable trade-off vs. lattice-based commitments (which are much larger and less mature)

## Encrypted Memos

Botho provides an encrypted communication channel between sender and recipient, protecting memo content from blockchain observers.

### How It Works

Each transaction output includes a 66-byte encrypted payload:

1. **Key Derivation**: Uses HKDF with Blake2b to derive an encryption key from the transaction's shared secret
2. **Encryption**: AES-256-CTR with authenticated encryption protects the memo content
3. **Decryption**: Only the recipient (with their view key) can decrypt the memo

### Memo Format

```
| 2 bytes |   64 bytes   |
|  type   |     data     |
```

Supported memo types include:
- **Destination memos**: Authenticated destination information
- **Sender memos**: Payment request IDs and sender identification
- **Gift code memos**: For redeemable payment codes

### Privacy Properties

- Memo content is invisible to blockchain observers
- Only the recipient can decrypt using their view private key
- Sender identity can be optionally included (authenticated)

## Why This Architecture?

Botho uses ML-KEM-768 (post-quantum) for stealth addresses and CLSAG (classical) for ring signatures. This is a deliberate design choice, not a limitation.

### What's Protected Against Quantum Computers

| Component | Quantum Safety | Rationale |
|-----------|---------------|-----------|
| **Recipient identity** | ✓ PQ-safe (ML-KEM) | Permanent on-chain, "harvest now, decrypt later" threat |
| **Amount values** | ✓ PQ-safe (info-theoretic) | Pedersen hiding is unconditional, not computational |
| **Sender anonymity** | Classical (CLSAG) | See rationale below |

### Why Sender Anonymity is Classical

We evaluated post-quantum ring signatures (LION) extensively and decided against them for several reasons:

**1. Size vs. Adoption Trade-off**

| Metric | CLSAG | LION (deprecated) |
|--------|-------|-------------------|
| Signature size | ~700 bytes | ~36,000 bytes |
| Transaction size (2-in/2-out) | ~4 KB | ~75 KB |
| Annual blockchain growth | ~100 GB | ~1.8 TB |
| Desktop node feasibility | ✓ Yes | ✗ Datacenter only |

A privacy system that nobody can run provides no privacy. LION would have restricted node operation to datacenters, centralizing the network and reducing the anonymity set.

**2. Network-Level Deanonymization Dominates**

Even with perfect cryptographic sender anonymity, an omniscient adversary can deanonymize senders through:

```
┌────────────────────────────────────────────────────────────────┐
│              SENDER DEANONYMIZATION VECTORS                    │
├────────────────────────────────────────────────────────────────┤
│                                                                │
│  CRYPTOGRAPHIC (what ring signatures protect)                 │
│  └── Ring analysis → quantum threat in ~20-30 years           │
│                                                                │
│  NETWORK-LEVEL (not protected by any signature scheme)        │
│  ├── IP address correlation with transaction broadcast        │
│  ├── Transaction propagation timing analysis                  │
│  ├── ISP/AS-level traffic analysis                            │
│  └── Tor/I2P exit node correlation                            │
│                                                                │
│  BEHAVIORAL (not protected by cryptography)                   │
│  ├── Timing patterns (regular payments)                       │
│  ├── Amount patterns (number of outputs)                      │
│  └── Exchange KYC correlation                                 │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

The weakest link for sender privacy is network-level surveillance, not cryptographic ring analysis. Investing in PQ ring signatures doesn't address the actual dominant threat.

**3. Sender Privacy is Inherently Ephemeral**

Unlike recipient identity (permanent on-chain), the value of sender anonymity degrades over time:
- Economic context becomes historical
- UTXOs get spent, reducing ring membership relevance
- Chain analysis becomes less actionable

Protecting sender identity for 50+ years against quantum computers isn't worth 50x larger transactions when:
- The quantum threat is 20-30 years away
- Network-level attacks work today
- Larger transactions mean smaller anonymity sets

**4. Recipient Privacy is the Higher Priority**

Recipients cannot control who sends them funds. A journalist receiving a whistleblower tip, an activist receiving donations, or a merchant receiving payments—all need their identity protected permanently. ML-KEM stealth addresses provide this.

Senders actively choose to transact and can use operational security (timing, network privacy) to supplement cryptographic anonymity.

### The Practical Result

```
┌─────────────────────────────────────────────────────────────────┐
│                    Botho Privacy Architecture                    │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  RECIPIENT IDENTITY   → ML-KEM stealth      → PQ-safe ✓         │
│  AMOUNT VALUES        → Pedersen hiding     → PQ-safe ✓         │
│  SENDER ANONYMITY     → CLSAG ring (20)     → Classical         │
│  NETWORK PRIVACY      → Dandelion++         → Best-effort       │
│                                                                  │
│  Desktop-friendly: ~100 GB/year blockchain growth               │
│  Large anonymity set: More users can run nodes                  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

This architecture provides excellent present-day privacy while protecting the most critical data (recipient identity, amounts) against future quantum computers.

## Post-Quantum Cryptography

Botho provides post-quantum protection for recipient identity and amount privacy.

### PQ Primitives Used

| Component | Algorithm | Standard | Quantum Safety |
|-----------|-----------|----------|----------------|
| Stealth addresses | ML-KEM-768 | FIPS 203 | ✓ Full |
| Minting signatures | ML-DSA-65 | FIPS 204 | ✓ Full |
| Ring signatures | CLSAG | curve25519 | Classical |
| Amount hiding | Pedersen | info-theoretic | ✓ Full |

### Ring Size

Botho uses ring size 20 for CLSAG signatures:

| Scheme | Ring Size | Signature Size | Privacy Bits | Efficiency |
|--------|-----------|----------------|--------------|------------|
| CLSAG | 20 | ~700 bytes | 4.32 bits | 94.8% |

**Why ring size 20?**

- Larger than Monero's 16, providing better anonymity
- Compact signatures (~700 bytes) make larger rings practical
- Achieves 94.8% of theoretical maximum privacy
- ~10+ effective anonymity after accounting for heuristic attacks

### OSPEAD Decoy Selection

Private transactions use OSPEAD (Optimal Selection Probability to Evade Analysis of Decoys):

- **Gamma distribution**: Matches decoy ages to real spending patterns
- **Age-weighted selection**: Newer outputs more likely to be selected
- **Cluster similarity**: Prefers decoys with similar cluster tag profiles (≥70% cosine similarity)
- **Effective anonymity**: Achieves 10+ effective anonymity with ring size 20

#### Cluster-Aware Selection

Because cluster tags are visible on transaction outputs, an observer could potentially identify the true sender by matching output tags to input tags. To prevent this, OSPEAD prioritizes decoys with similar cluster profiles:

```
similarity(a, b) = cosine(a.cluster_tags, b.cluster_tags)
```

Selection criteria:
1. Dominant clusters overlap (top-3 clusters match)
2. Tag weights within ~20% of each other
3. Age and amount remain plausible

This ensures all ring members would produce similar output tag patterns, preventing fingerprinting attacks.

### Key Derivation

All keys derive deterministically from the BIP39 mnemonic:

```
mnemonic → SLIP-10 seed → HKDF → {ML-KEM keypair, ML-DSA keypair, classical keypair}
```

### Transaction Sizes

| Transaction Type | Size per Input | Size per Output |
|-----------------|----------------|-----------------|
| Minting | N/A (coinbase) | ~1.2 KB |
| Private | ~0.7 KB (CLSAG) | ~1.2 KB |

Compact transaction sizes enable desktop-friendly blockchain growth while maintaining strong privacy.

## Privacy Best Practices

### For Users

1. **Use fresh addresses**: Generate new subaddresses for each payment request
2. **Allow time between transactions**: Spacing transactions makes timing analysis harder
3. **Use ring signature transactions**: When sender privacy matters, use Standard-Private or PQ-Private
4. **Don't reuse patterns**: Vary transaction amounts and timing to avoid fingerprinting

### Privacy Limitations

- **Network-level privacy**: Botho doesn't provide IP-level privacy. Consider using Tor or I2P.
- **Metadata**: Transaction timing and frequency may leak information
- **Exchange interactions**: KYC exchanges can link your identity to addresses

## Comparison with Other Privacy Coins

| Feature | Botho | Monero | Zcash |
|---------|---------|--------|-------|
| Stealth addresses | All tx (ML-KEM) | All tx (ECDH) | Shielded only |
| Ring signatures | All tx (CLSAG) | All tx (CLSAG) | No |
| Ring size | 20 | 16 | N/A |
| **Effective anonymity** | **10+ of 20** | ~11 of 16 (estimated) | Perfect (ZK) |
| Confidential amounts | All types | Yes | Shielded only |
| Encrypted memos | Yes | No | Shielded only |
| Post-quantum stealth | Yes (ML-KEM-768) | No | No |
| Post-quantum amounts | Yes (info-theoretic) | No | No |
| Privacy by default | Yes | Yes | No (opt-in) |
| Progressive fees | Yes (cluster tags) | No | No |
| Desktop node feasible | Yes (~100 GB/yr) | Yes | Yes |
| Consensus | SCP (Federated) | PoW (RandomX) | PoW (Equihash) |

**Note on effective anonymity**: Botho's effective anonymity (10+ of 20) reflects cluster-aware decoy selection mitigating fingerprinting attacks. Monero's estimate is based on similar age-based heuristic analysis. Zcash shielded transactions use zero-knowledge proofs with perfect hiding.

**Botho's unique position**: We're the first privacy coin with post-quantum protection for recipient identity (ML-KEM stealth addresses) and unconditionally secure amount hiding, while maintaining desktop-friendly transaction sizes.

## Technical References

- [CryptoNote Whitepaper](https://cryptonote.org/whitepaper.pdf) - Original stealth address specification
- [CLSAG Paper](https://eprint.iacr.org/2019/654.pdf) - Concise Linkable Ring Signatures
- [Bulletproofs Paper](https://eprint.iacr.org/2017/1066.pdf) - Range proof system
- [ML-KEM (FIPS 203)](https://csrc.nist.gov/pubs/fips/203/final) - Post-quantum key encapsulation
- [ML-DSA (FIPS 204)](https://csrc.nist.gov/pubs/fips/204/final) - Post-quantum digital signatures
