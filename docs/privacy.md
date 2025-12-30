# Privacy Features

Botho provides strong transaction privacy through a combination of cryptographic techniques inherited from the CryptoNote protocol, enhanced with post-quantum cryptography for future-proof security.

## Overview

| Privacy Goal | Technique | Status |
|--------------|-----------|--------|
| Hide recipient | Stealth addresses (one-time keys) | Implemented |
| Hide sender | Ring signatures (MLSAG) | Implemented |
| Hide amounts | RingCT (confidential transactions) | Planned |
| Secure communication | Encrypted memos (AES-256-CTR) | Implemented |
| Quantum resistance | Hybrid classical + PQ (ML-KEM/ML-DSA) | Implemented |

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

Botho implements hybrid classical + post-quantum cryptography to protect against future quantum computer attacks, including "harvest now, decrypt later" threats.

### Why Post-Quantum?

Large-scale quantum computers could break classical elliptic curve cryptography. Adversaries may be recording encrypted transactions today to decrypt them later. Botho's hybrid approach provides:

- **Defense in depth**: Both classical AND post-quantum signatures must verify
- **Fallback security**: If either cryptosystem is broken, the other still protects you
- **Future-proof privacy**: Transactions remain private even against quantum adversaries

### Algorithms Used

| Component | Classical | Post-Quantum | Standard |
|-----------|-----------|--------------|----------|
| Key Exchange | ECDH (Ristretto) | ML-KEM-768 (Kyber) | NIST FIPS 203 |
| Signatures | Schnorr (Ed25519) | ML-DSA-65 (Dilithium) | NIST FIPS 204 |

### How It Works

**Quantum-Safe Stealth Addresses:**

1. Sender generates classical ephemeral key AND ML-KEM encapsulation
2. Transaction output contains both the one-time key `P` and the ML-KEM ciphertext (1088 bytes)
3. Recipient decapsulates using their ML-KEM private key to recover the shared secret
4. Both key exchanges must succeed for the transaction to be recognized

**Quantum-Safe Signatures:**

1. Transaction inputs require BOTH classical Schnorr AND ML-DSA-65 signatures
2. Validators verify both signatures independently
3. Transaction is only valid if both signatures verify

### Key Derivation

All quantum-safe keys derive deterministically from the same BIP39 mnemonic:

```
mnemonic → SLIP-10 → classical keys (view, spend)
                   → ML-KEM-768 keys (encapsulation)
                   → ML-DSA-65 keys (signing)
```

### Transaction Sizes

| Transaction Type | Classical | Quantum-Safe |
|-----------------|-----------|--------------|
| Output | ~100 bytes | ~1160 bytes |
| Input (signature) | ~64 bytes | ~2520 bytes |

The size increase is the cost of quantum resistance. As post-quantum algorithms mature, sizes may decrease.

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
| Ring signatures | Yes (MLSAG) | Yes (CLSAG) | No |
| Confidential amounts | Planned | Yes | Shielded only |
| Encrypted memos | Yes | No | Shielded only |
| Post-quantum crypto | Yes (hybrid) | No | No |
| Privacy by default | Yes | Yes | No (opt-in) |
| Consensus | SCP (Federated) | PoW (RandomX) | PoW (Equihash) |

## Technical References

- [CryptoNote Whitepaper](https://cryptonote.org/whitepaper.pdf) - Original stealth address specification
- [Zero to Monero](https://web.getmonero.org/library/Zero-to-Monero-2-0-0.pdf) - Detailed ring signature construction
- [Bulletproofs Paper](https://eprint.iacr.org/2017/1066.pdf) - Range proof system
- [NIST FIPS 203](https://csrc.nist.gov/pubs/fips/203/final) - ML-KEM (Kyber) specification
- [NIST FIPS 204](https://csrc.nist.gov/pubs/fips/204/final) - ML-DSA (Dilithium) specification
