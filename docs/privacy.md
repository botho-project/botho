# Privacy Features

Botho provides strong transaction privacy through a combination of cryptographic techniques, designed from the ground up with post-quantum security. All cryptographic primitives use NIST-standardized post-quantum algorithms.

## Overview

| Privacy Goal | Technique | Effectiveness |
|--------------|-----------|---------------|
| Hide recipient | PQ stealth addresses (ML-KEM-768) | Perfect (all transactions) |
| Hide sender | LION ring signatures (ring=7) | 94.8% efficient (6.3 of 7 effective) |
| Hide amounts | Pedersen commitments + Bulletproofs | Perfect (Standard & Private) |
| Secure communication | Encrypted memos (AES-256-CTR) | Perfect (all transactions) |
| Quantum resistance | Pure PQ (no classical fallback) | ~128-192 bit PQ security |

**Privacy tradeoff**: Visible cluster tags for progressive fees reduce ring signature anonymity by ~0.15 bits (from 2.81 to 2.66 bits average). We consider this acceptable for the anti-inequality benefits of wealth-based transaction fees.

## Transaction Types

Botho supports three transaction types with different privacy trade-offs. See [Transaction Types](transactions.md) for complete details.

| Type | Recipient | Amount | Sender | Use Case |
|------|-----------|--------|--------|----------|
| **Minting** | Hidden | Public | Known | Block rewards |
| **Standard** | Hidden | Hidden | Visible | Most transfers |
| **Private** | Hidden | Hidden | Hidden | Maximum privacy |

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

Botho implements a novel **cluster-based progressive fee** system designed to reduce wealth concentration without sacrificing privacy or enabling Sybil attacks.

### How It Works

Transaction fees are based on coin *ancestry*, not account identity:

1. **Clusters**: Each coin-creation event (minting reward) spawns a new "cluster" identity
2. **Tag Vectors**: Every account carries a sparse vector of weights indicating what fraction of its coins trace back to each cluster origin
3. **Cluster Wealth**: The total value in the system tagged to a given cluster (`W = Σ balance × tag_weight`)
4. **Progressive Fees**: Fee rate increases with cluster wealth via a sigmoid curve

```
Fee Rate = sigmoid(cluster_wealth) → ranges from 0.05% to 30%
```

### Why It's Sybil-Resistant

Splitting transactions or creating multiple accounts doesn't reduce fees because:

- Fee rate depends on **cluster wealth**, not transaction size or account count
- All accounts holding coins from the same minting origin pay the same rate
- The only way to reduce fees is through genuine economic activity that diffuses coins

### Tag Decay

Tags decay by ~5% per transaction hop:

- Coins that circulate widely pay lower fees over time
- Hoarded coins retain high cluster attribution and pay higher fees
- ~14 transaction hops to halve a tag's weight

### Parameters

| Parameter | Value | Description |
|-----------|-------|-------------|
| Minimum fee | 0.05% | Small/diffused clusters |
| Maximum fee | 30% | Large concentrated clusters |
| Decay rate | 5% per hop | Tag decay per transaction |
| Midpoint | 10M BTH | Sigmoid inflection point |

### Tag Vector Limits

To bound storage and reduce fingerprinting, cluster tag vectors are truncated:

| Parameter | Value | Description |
|-----------|-------|-------------|
| Max entries | 16 | Maximum clusters tracked per output |
| Min weight | 0.1% | Weights below this are pruned to "background" |
| Truncation | By weight | Lowest-weight entries dropped first |

Weights below 0.1% (1000 in parts-per-million) become "background" — unattributed ancestry that's indistinguishable across outputs. This ensures old ancestry eventually disappears and tag vectors don't grow unbounded.

## Ring Signatures (Private Transactions)

Private transactions use ring signatures to hide the true sender among a group of possible signers. Botho uses LION (Lattice-based lInkable ring signatures fOr aNonymity), a post-quantum ring signature scheme.

> **Note**: Ring signatures are only used in Private transactions. Standard transactions use direct ML-DSA signatures (sender is visible).

### How Ring Signatures Work

1. **Decoy Selection**: When spending, the sender selects 6 decoy outputs from the blockchain
2. **Ring Construction**: Creates a ring of 7 possible signers (1 real + 6 decoys)
3. **LION Signing**: Produces a signature proving ownership of ONE ring member without revealing which
4. **Verification**: Anyone can verify the signature is valid, but cannot determine the true signer

### Technical Implementation

Botho uses LION lattice-based ring signatures:

```
Ring = [decoy_1, decoy_2, ..., real_input, ..., decoy_6]  (shuffled, 7 total)
Signature = LION.sign(ring, real_index, secret_key)
KeyImage = LION.key_image(secret_key)
```

The wallet's `create_private_transaction()` method handles:
- Automatic decoy selection using OSPEAD algorithm
- Ring construction with fixed size of 7
- Cryptographically secure shuffling
- Post-quantum secure signing

### Benefits

- **Sender Unlinkability**: Observers cannot determine which input is being spent
- **Plausible Deniability**: Any of the 7 ring members could be the true sender
- **No Coordination**: Decoys don't know they're being used
- **Linkable**: Key images prevent double-spending without revealing the signer
- **Post-Quantum**: Secure against quantum computer attacks

### Cluster Tag Privacy Considerations

Botho's progressive fee system uses **cluster tags** to track coin ancestry for wealth-based taxation. These tags are visible on transaction outputs, which creates a potential privacy consideration for ring signatures.

#### The Challenge

When a private transaction creates outputs, the cluster tags on those outputs are derived from the input's tags (with 5% decay). An observer could potentially:

1. Examine the ring of 7 possible inputs
2. Compare each input's cluster tags to the output's tags
3. Identify which input's tags, after decay, best match the output

If only one ring member's tags produce a plausible output pattern, the ring signature anonymity is reduced.

#### Example Attack Scenario

```
Ring Member A: {cluster_17: 0.80, cluster_42: 0.15}
Ring Member B: {cluster_3: 0.95}
Ring Member C: {cluster_17: 0.40, cluster_42: 0.40}
... (4 more members with different patterns)

Output tags:  {cluster_17: 0.76, cluster_42: 0.14}  (after 5% decay)
```

An observer calculates: 0.80 × 0.95 = 0.76, 0.15 × 0.95 ≈ 0.14 — only Ring Member A matches!

#### Measured Privacy (Simulation Results)

We built a Monte Carlo simulation to honestly measure the privacy users can expect. The simulation models 10,000 ring formations with a realistic UTXO pool (100K outputs, 50% standard transactions, 5% tag decay).

**Against a sophisticated adversary** using both age and cluster fingerprinting heuristics:

| Metric | Value | Interpretation |
|--------|-------|----------------|
| Theoretical maximum | 2.81 bits | log₂(7) for ring size 7 |
| **Measured average** | **2.66 bits** | ~6.3 effective ring members |
| Measured median | 2.80 bits | Most transactions near-ideal |
| Worst case (5th percentile) | 2.66 bits | Even bad cases are decent |
| Privacy efficiency | 94.8% | Of theoretical maximum |

**Privacy by output type:**

| Output Type | Mean Bits | Effective Ring Size |
|-------------|-----------|---------------------|
| Standard transactions | 2.79 | 6.9 of 7 |
| Exchange outputs | 2.80 | 7.0 of 7 |
| Whale outputs | 2.80 | 7.0 of 7 |
| Coinbase (fresh) | 2.80 | 7.0 of 7 |

**Adversary identification rates:**

| Adversary Model | ID Rate | vs Random (14.3%) |
|-----------------|---------|-------------------|
| Age heuristics only | 10.7% | Better than random |
| Cluster fingerprinting only | 66.9% | 4.7× advantage |
| Combined (realistic) | 39.1% | 2.7× advantage |

#### The Honest Tradeoff

Visible cluster tags leak approximately **0.15 bits of privacy** compared to a system with no cluster information. This reduces effective anonymity from 7.0 to ~6.3 ring members on average.

We consider this an acceptable tradeoff because:

1. **Progressive fees require ancestry tracking** — The anti-inequality mechanism depends on knowing coin origins
2. **Median privacy is near-perfect** — Most transactions achieve 2.80 of 2.81 possible bits
3. **Cluster-aware selection works** — Without it, adversary ID rate would be 67%; with it, 39%
4. **Privacy improves with circulation** — The 5% decay means coins become more private over time

#### Mitigation: Cluster-Aware Decoy Selection

Botho's OSPEAD algorithm addresses cluster fingerprinting by selecting decoys with **similar cluster tag profiles** (≥70% cosine similarity). When all ring members have comparable tag patterns, the fingerprinting attack fails because multiple members produce plausible outputs.

See [OSPEAD Decoy Selection](#ospead-decoy-selection) for implementation details.

#### Running Your Own Simulations

The privacy simulation is available as a CLI tool:

```bash
cargo run -p bth-cluster-tax --features cli --release --bin cluster-tax-sim -- privacy \
  -n 10000 \
  --pool-size 100000 \
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

Botho supports three transaction types. See [Transaction Types](transactions.md) for complete technical details.

### Transaction Types

| Type | Amount Privacy | Sender Privacy | Signature | Use Case |
|------|---------------|----------------|-----------|----------|
| Minting | Public | Known (minter) | ML-DSA | Block rewards |
| Standard | Hidden | Visible | ML-DSA | Most transfers |
| Private | Hidden | Hidden (ring) | LION (ring=7) | Maximum privacy |

### Fee Structure by Transaction Type

| Transaction Type | Base Fee | Size | Typical Total |
|-----------------|----------|------|---------------|
| Minting | 0 | ~1.5 KB | 0 (coinbase) |
| Standard | 400 µBTH | ~3-4 KB | ~600 µBTH |
| Private | 400 µBTH | ~22 KB | ~2,500 µBTH |

**Why the difference?**

- **Minting transactions** have no fee (they create coins, not transfer them)
- **Standard transactions** use ML-DSA signatures (~3.3 KB per input)
- **Private transactions** use LION ring signatures (~17.5 KB per input)

### Choosing Transaction Type

**Use Standard transactions when:**
- Sender privacy isn't critical for this payment
- You want lower fees
- Business payments requiring audit trails
- Sending to exchanges (they may prefer identifiable senders)

**Use Private transactions when:**
- You need sender anonymity
- Long-term privacy is essential
- Sensitive or high-value transfers
- Protection against transaction graph analysis

### Fee Calculation

All fees follow this formula:

```
total_fee = max(base_fee, tx_size * fee_per_byte) + cluster_fee
```

Where:
- `base_fee` = 400 µBTH (minimum)
- `fee_per_byte` = dynamic based on mempool congestion
- `cluster_fee` = progressive fee based on coin ancestry (0.05% - 30%)

See [Tokenomics](/docs/tokenomics) for details on cluster-based progressive fees.

## Confidential Amounts

Standard and Private transactions hide amounts using Pedersen commitments and Bulletproofs range proofs.

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

## Post-Quantum Cryptography

Botho is designed from the ground up with post-quantum security. All cryptographic primitives use NIST-standardized algorithms or equivalent security levels.

### Why Post-Quantum?

Large-scale quantum computers could break classical elliptic curve cryptography. Adversaries may be recording encrypted transactions today to decrypt them later ("harvest now, decrypt later" attacks). Botho provides:

- **Pure PQ design**: No classical fallback—quantum-safe from day one
- **NIST-standardized**: ML-KEM-768 and ML-DSA-65 from FIPS 203/204
- **Future-proof privacy**: Transactions remain private even against quantum adversaries
- **Consistent security level**: ~128-192 bit post-quantum security throughout

### PQ Primitives Used

| Component | Algorithm | Standard | Used In |
|-----------|-----------|----------|---------|
| Stealth addresses | ML-KEM-768 | FIPS 203 | All transactions |
| Minting/Standard signatures | ML-DSA-65 | FIPS 204 | Minting, Standard |
| Ring signatures | LION | Module-LWE | Private only |
| Key images | LION-derived | Module-LWE | Private only |

### LION Ring Signatures

LION (Lattice-based lInkable ring signatures fOr aNonymity) provides sender privacy in Private transactions.

| Parameter | Value | Description |
|-----------|-------|-------------|
| Ring size | 7 | Fixed ring size for privacy/efficiency balance |
| Security level | ~128-bit PQ | Based on Module-LWE hardness |
| Lattice dimension | N=256, K=L=4 | Module rank matches Dilithium-3 |
| Signature size | ~23 KB | Per ring (includes all responses) |

### Why Ring Size 7?

We chose ring size 7 based on a cost/benefit analysis of signature size vs. privacy:

**Signature Size by Ring Size:**

| Ring Size | Signature | Theoretical Max | Measured | Efficiency |
|-----------|-----------|-----------------|----------|------------|
| 5 | 17.0 KB | 2.32 bits | 2.19 bits | 94.5% |
| **7** | **23.0 KB** | **2.81 bits** | **2.68 bits** | **95.4%** |
| 9 | 29.0 KB | 3.17 bits | 3.01 bits | 95.0% |
| 11 | 35.0 KB | 3.46 bits | 3.26 bits | 94.3% |
| 13 | 41.0 KB | 3.70 bits | 3.52 bits | 95.1% |

**Diminishing Returns:**

| Comparison | Size Increase | Privacy Increase | Efficiency |
|------------|---------------|------------------|------------|
| 5 → 7 | +35% | +21% | 0.59 |
| 7 → 9 | +26% | +13% | 0.50 |
| 7 → 11 | +52% | +23% | 0.44 |
| 7 → 13 | +78% | +32% | 0.41 |

**Why not smaller (ring 5)?**
- Only 2.32 bits theoretical (vs 2.81 for ring 7)
- 21% less privacy for 35% size savings — poor tradeoff

**Why not larger (ring 9+)?**
- Each +2 ring members adds ~6 KB but only ~0.3 bits
- Ledger bloat: 1M transactions = +5.7 GB per ring size increase
- Cluster leakage (~0.15 bits) is constant regardless of ring size

**Ring 7 is the sweet spot** because:
1. Best measured efficiency (95.4% of theoretical)
2. Last ring size before severe diminishing returns
3. Reasonable signature size for post-quantum crypto
4. Cluster-aware selection works optimally at this size

**Ledger Impact (per 1M private transactions):**

| Ring Size | Ledger Storage |
|-----------|----------------|
| 5 | 16.2 GB |
| 7 | 22.0 GB |
| 9 | 27.7 GB |
| 11 | 33.4 GB |
| 13 | 39.1 GB |

Run your own analysis:
```bash
cargo run -p bth-cluster-tax --features cli --release --bin cluster-tax-sim -- ring-size --simulate
```

### OSPEAD Decoy Selection

Botho uses OSPEAD (Optimal Selection Probability to Evade Analysis of Decoys) to select ring decoys:

- **Gamma distribution**: Matches decoy ages to real spending patterns
- **Age-weighted selection**: Newer outputs more likely to be selected
- **Cluster similarity**: Prefers decoys with similar cluster tag profiles
- **Effective anonymity**: Multiple ring members appear equally likely

#### Cluster-Aware Selection

Because cluster tags are visible on transaction outputs, an observer could potentially identify the true sender by matching output tags to input tags. To prevent this, OSPEAD prioritizes decoys with similar cluster profiles:

```
similarity(a, b) = cosine(a.cluster_tags, b.cluster_tags)
```

Selection criteria:
1. Dominant clusters overlap (top-3 clusters match)
2. Tag weights within ~20% of each other
3. Age and amount remain plausible

This ensures all ring members would produce similar output tag patterns, preventing fingerprinting attacks. The trade-off is a smaller candidate pool, but this improves as the network matures and coins circulate more widely.

### Key Derivation

All keys derive deterministically from the BIP39 mnemonic:

```
mnemonic → SLIP-10 seed → HKDF → {ML-KEM keypair, ML-DSA keypair, LION keypair}
```

### Transaction Sizes

| Transaction Type | Size per Input | Size per Output |
|-----------------|----------------|-----------------|
| Minting | N/A (coinbase) | ~1.2 KB |
| Standard | ~3.4 KB | ~1.2 KB |
| Private | ~17.5 KB | ~1.2 KB |

The larger signature sizes are the cost of quantum resistance. This is a worthwhile trade-off for long-term privacy protection.

## Privacy Best Practices

### For Users

1. **Use fresh addresses**: Generate new subaddresses for each payment request
2. **Allow time between transactions**: Spacing transactions makes timing analysis harder
3. **Use Private transactions**: When sender privacy matters, use Private transactions
4. **Don't reuse patterns**: Vary transaction amounts and timing to avoid fingerprinting

### Privacy Limitations

- **Network-level privacy**: Botho doesn't provide IP-level privacy. Consider using Tor or I2P.
- **Metadata**: Transaction timing and frequency may leak information
- **Exchange interactions**: KYC exchanges can link your identity to addresses

## Comparison with Other Privacy Coins

| Feature | Botho | Monero | Zcash |
|---------|---------|--------|-------|
| Stealth addresses | All tx (ML-KEM) | All tx (ECDH) | Shielded only |
| Ring signatures | Private tx (LION) | All tx (CLSAG) | No |
| Ring size | 7 | 16 | N/A |
| **Effective anonymity** | **6.3 of 7 (measured)** | ~11 of 16 (estimated) | Perfect (ZK) |
| Confidential amounts | Standard & Private | Yes | Shielded only |
| Encrypted memos | Yes | No | Shielded only |
| Post-quantum crypto | Yes (pure PQ) | No | No |
| Privacy by default | Yes | Yes | No (opt-in) |
| Sender-visible option | Standard tx | No | Transparent tx |
| Progressive fees | Yes (cluster tags) | No | No |
| Consensus | SCP (Federated) | PoW (RandomX) | PoW (Equihash) |

**Note on effective anonymity**: Botho's 6.3/7 effective ring members reflects the measured cost of visible cluster tags for progressive fees. Monero's estimate is based on similar age-based heuristic analysis. Zcash shielded transactions use zero-knowledge proofs with perfect hiding.

## Technical References

- [CryptoNote Whitepaper](https://cryptonote.org/whitepaper.pdf) - Original stealth address specification
- [Bulletproofs Paper](https://eprint.iacr.org/2017/1066.pdf) - Range proof system
- [LION Ring Signatures](https://link.springer.com/chapter/10.1007/978-981-95-3540-8_17) - Lattice-based linkable ring signatures
- [ML-KEM (FIPS 203)](https://csrc.nist.gov/pubs/fips/203/final) - Post-quantum key encapsulation
- [ML-DSA (FIPS 204)](https://csrc.nist.gov/pubs/fips/204/final) - Post-quantum digital signatures
