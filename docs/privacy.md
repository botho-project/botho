# Privacy Features

Botho provides strong transaction privacy through a combination of cryptographic techniques inherited from the CryptoNote protocol, enhanced with post-quantum cryptography for future-proof security.

## Overview

| Privacy Goal | Technique | Status |
|--------------|-----------|--------|
| Hide recipient | Stealth addresses (one-time keys) | Implemented |
| Hide sender | Ring signatures (MLSAG / LION) | Implemented |
| Hide amounts | RingCT (confidential transactions) | Planned |
| Secure communication | Encrypted memos (AES-256-CTR) | Implemented |
| Quantum resistance | LION lattice-based ring signatures | Implemented |

## Stealth Addresses

Every transaction creates a **unique one-time destination address**, ensuring that:

- Outside observers cannot link multiple payments to the same recipient
- The blockchain reveals no information about who received funds
- Only the recipient can identify their incoming transactions

### How It Works

Botho implements CryptoNote-style stealth addresses:

1. **Sender creates transaction:**
   - Generates a random ephemeral key pair `(r, R)` where `R = r*G`
   - Computes one-time destination key: `P = H(r*A)*G + B`
   - Where `(A, B)` are recipient's view and spend public keys

2. **Transaction is published:**
   - Contains the one-time key `P` and the public ephemeral key `R`
   - `P` looks random to everyone except the recipient

3. **Recipient scans blockchain:**
   - For each transaction, computes `P' = H(a*R)*G + B` using their view private key `a`
   - If `P' == P`, the transaction belongs to them
   - Can then spend using private key `x = H(a*R) + b`

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
| Midpoint | 10M credits | Sigmoid inflection point |

## Ring Signatures

Ring signatures hide the true sender among a group of possible signers using MLSAG (Multilayered Linkable Spontaneous Anonymous Group) signatures.

### How Ring Signatures Work

1. **Decoy Selection**: When spending, the sender selects N-1 decoy outputs from the blockchain
2. **Ring Construction**: Creates a signature that proves ownership of ONE of the N outputs, without revealing which
3. **Ring Shuffling**: Decoys are randomly shuffled with the real input using cryptographically secure RNG
4. **Verification**: Anyone can verify the signature is valid, but cannot determine the true signer

### Technical Implementation

Botho uses MLSAG signatures with domain-separated signing digests:

```
Ring = [decoy_1, decoy_2, ..., real_input, ..., decoy_n]  (shuffled)
Signature = MLSAG.sign(ring, real_index, private_key)
```

The wallet's `create_private_transaction()` method handles:
- Automatic decoy selection from the ledger
- Ring construction with configurable size
- Cryptographically secure shuffling
- Domain-separated transaction signing

### Benefits

- **Sender Unlinkability**: Observers cannot determine which input is being spent
- **Plausible Deniability**: Any of the ring members could be the true sender
- **No Coordination**: Decoys don't know they're being used
- **Linkable**: Key images prevent double-spending without revealing the signer

## Transaction Types and Fees

Botho supports multiple transaction types with different privacy levels and fee structures.

### Transaction Types

| Type | Privacy Level | Ring Size | Signature | Use Case |
|------|--------------|-----------|-----------|----------|
| Open | None | N/A | Schnorr | Exchanges, auditable payments |
| Private (MLSAG) | High | 7 | Classical ring sig | Standard private transfers |
| Private (LION) | High + PQ | 7 | Post-quantum ring sig | Long-term private storage |

### Fee Structure by Transaction Type

| Transaction Type | Base Fee | Size Multiplier | Typical Total |
|-----------------|----------|-----------------|---------------|
| Open | 400 µBTH | ~100 bytes | ~400 µBTH |
| Private (MLSAG) | 400 µBTH | ~500 bytes/input | ~600 µBTH |
| Private (LION) | 400 µBTH | ~17 KB/input | ~2,000 µBTH |

**Why the difference?**

- **Open transactions** are smallest - just a simple Schnorr signature per input
- **Private (MLSAG)** requires ring signatures with 7 members, increasing size
- **Private (LION)** uses lattice-based signatures which are inherently larger

### Choosing Transaction Type

**Use Open transactions when:**
- Sending to/from exchanges (they require transparent history)
- Business payments requiring audit trails
- You don't need sender privacy for this payment

**Use Private (MLSAG) transactions when:**
- You want sender anonymity
- Post-quantum security isn't critical for this payment
- You prefer smaller transaction sizes

**Use Private (LION) transactions when:**
- Long-term privacy is essential (coins you'll hold for years)
- You're protecting against "harvest now, decrypt later" attacks
- Quantum resistance justifies the larger fee

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

## RingCT (Planned)

Ring Confidential Transactions will hide transaction amounts using Pedersen commitments and range proofs.

### How RingCT Works

1. **Pedersen Commitments**: Amounts are encoded as `C = aG + bH` where `a` is the amount and `b` is a blinding factor
2. **Balance Proof**: Proves that inputs equal outputs without revealing values
3. **Range Proofs**: Proves amounts are positive without revealing them (using Bulletproofs)

### Benefits

- **Amount Privacy**: Observers cannot see how much is being transferred
- **Verifiable**: Network can still verify no coins are created from nothing
- **Compact**: Bulletproofs provide efficient range proofs

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

Botho implements **LION** (Lattice-based lInkable ring signatures fOr aNonymity), a purpose-built post-quantum ring signature scheme that provides both sender anonymity and quantum resistance in a single unified primitive.

### Why Post-Quantum?

Large-scale quantum computers could break classical elliptic curve cryptography. Adversaries may be recording encrypted transactions today to decrypt them later ("harvest now, decrypt later" attacks). LION provides:

- **Unified design**: Single algorithm handles both privacy AND quantum resistance
- **Simpler implementation**: One cryptographic primitive instead of hybrid combinations
- **Future-proof privacy**: Transactions remain private even against quantum adversaries
- **Linkable signatures**: Key images prevent double-spending without revealing the signer

### LION Ring Signatures

LION is a lattice-based linkable ring signature scheme based on the Module-LWE problem, providing ~128-bit post-quantum security. It uses parameters similar to ML-DSA (Dilithium) for consistency and proven security.

| Parameter | Value | Description |
|-----------|-------|-------------|
| Ring size | 7 | Fixed ring size for privacy/efficiency balance |
| Security level | ~128-bit PQ | Based on Module-LWE hardness |
| Lattice dimension | N=256, K=L=4 | Module rank matches Dilithium-3 |
| Signature size | ~17 KB | Per ring (includes all responses) |

### How It Works

1. **Key Generation**: Each user generates a LION keypair (public key, secret key)
2. **Key Image**: When spending, compute `I = H(sk) * G` - unique per secret key
3. **Ring Formation**: Select 6 decoys from the UTXO set to form a ring of 7
4. **Sign**: Produce a LION signature proving ownership of ONE ring member
5. **Verify**: Anyone can verify the signature without learning which member signed

### OSPEAD Decoy Selection

Botho uses OSPEAD (Optimal Selection Probability to Evade Analysis of Decoys) to select ring decoys:

- **Gamma distribution**: Matches decoy ages to real spending patterns
- **Age-weighted selection**: Prevents timing analysis attacks
- **1-in-4+ effective anonymity**: At least 2 ring members appear equally likely

### Key Derivation

All LION keys derive deterministically from the BIP39 mnemonic:

```
mnemonic → SLIP-10 seed → HKDF → LION keypair (pk, sk)
```

### Transaction Sizes

| Transaction Type | Classical (MLSAG) | Post-Quantum (LION) |
|-----------------|-------------------|---------------------|
| Ring size | 7 | 7 |
| Input (signature) | ~448 bytes | ~17 KB |
| Output | ~100 bytes | ~100 bytes |

The larger signature size is the cost of quantum resistance. LION signatures are larger than classical MLSAG but provide protection against future quantum attacks.

## Privacy Best Practices

### For Users

1. **Use fresh addresses**: Generate new subaddresses for each payment request
2. **Allow time between transactions**: Spacing transactions makes timing analysis harder
3. **Use consistent ring sizes**: Use the default ring size to blend in with other transactions
4. **Enable quantum-safe mode**: Use quantum-safe transactions for long-term privacy protection

### Privacy Limitations

- **Network-level privacy**: Botho doesn't provide IP-level privacy. Consider using Tor or I2P.
- **Metadata**: Transaction timing and frequency may leak information
- **Exchange interactions**: KYC exchanges can link your identity to addresses

## Comparison with Other Privacy Coins

| Feature | Botho | Monero | Zcash |
|---------|---------|--------|-------|
| Stealth addresses | Yes | Yes | Shielded only |
| Ring signatures | Yes (MLSAG/LION) | Yes (CLSAG) | No |
| Ring size | 7 | 16 | N/A |
| Confidential amounts | Planned | Yes | Shielded only |
| Encrypted memos | Yes | No | Shielded only |
| Post-quantum crypto | Yes (LION) | No | No |
| Privacy by default | Yes | Yes | No (opt-in) |
| Open (transparent) tx | Optional | No | Yes |
| Consensus | SCP (Federated) | PoW (RandomX) | PoW (Equihash) |

## Technical References

- [CryptoNote Whitepaper](https://cryptonote.org/whitepaper.pdf) - Original stealth address specification
- [Zero to Monero](https://web.getmonero.org/library/Zero-to-Monero-2-0-0.pdf) - Detailed ring signature construction
- [Bulletproofs Paper](https://eprint.iacr.org/2017/1066.pdf) - Range proof system
- [LION Ring Signatures](https://link.springer.com/chapter/10.1007/978-981-95-3540-8_17) - Lattice-based linkable ring signatures
- [Module-LWE](https://eprint.iacr.org/2017/1066.pdf) - Underlying lattice problem for LION security
