# Privacy Features

Botho provides strong transaction privacy through a combination of cryptographic techniques inherited from the CryptoNote protocol.

## Overview

| Privacy Goal | Technique | Status |
|--------------|-----------|--------|
| Hide recipient | Stealth addresses (one-time keys) | Implemented |
| Hide sender | Ring signatures | Planned |
| Hide amounts | RingCT (confidential transactions) | Planned |

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

1. **Clusters**: Each coin-creation event (mining reward) spawns a new "cluster" identity
2. **Tag Vectors**: Every account carries a sparse vector of weights indicating what fraction of its coins trace back to each cluster origin
3. **Cluster Wealth**: The total value in the system tagged to a given cluster (`W = Σ balance × tag_weight`)
4. **Progressive Fees**: Fee rate increases with cluster wealth via a sigmoid curve

```
Fee Rate = sigmoid(cluster_wealth) → ranges from 0.05% to 30%
```

### Why It's Sybil-Resistant

Splitting transactions or creating multiple accounts doesn't reduce fees because:

- Fee rate depends on **cluster wealth**, not transaction size or account count
- All accounts holding coins from the same mining origin pay the same rate
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

## Ring Signatures (Planned)

Ring signatures will hide the true sender among a group of possible signers.

### How Ring Signatures Work

1. **Decoy Selection**: When spending, the sender selects N-1 decoy outputs from the blockchain
2. **Ring Construction**: Creates a signature that proves ownership of ONE of the N outputs, without revealing which
3. **Verification**: Anyone can verify the signature is valid, but cannot determine the true signer

### Benefits

- **Sender Unlinkability**: Observers cannot determine which input is being spent
- **Plausible Deniability**: Any of the ring members could be the true sender
- **No Coordination**: Decoys don't know they're being used

### Current Status

Currently uses plain Ed25519 signatures. Ring signature implementation is planned for a future release.

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

## Privacy Best Practices

### For Users

1. **Use fresh addresses**: Generate new subaddresses for each payment request
2. **Allow time between transactions**: Spacing transactions makes timing analysis harder
3. **Use consistent ring sizes**: When ring signatures are implemented, use the default ring size

### Privacy Limitations

- **Network-level privacy**: Botho doesn't provide IP-level privacy. Consider using Tor or I2P.
- **Metadata**: Transaction timing and frequency may leak information
- **Exchange interactions**: KYC exchanges can link your identity to addresses

## Comparison with Other Privacy Coins

| Feature | Botho | Monero | Zcash |
|---------|---------|--------|-------|
| Stealth addresses | Yes | Yes | Shielded only |
| Ring signatures | Planned | Yes | No |
| Confidential amounts | Planned | Yes | Shielded only |
| Privacy by default | Yes | Yes | No (opt-in) |
| Proof of work | SHA-256 | RandomX | Equihash |

## Technical References

- [CryptoNote Whitepaper](https://cryptonote.org/whitepaper.pdf) - Original stealth address specification
- [Ring Signatures Paper](https://web.getmonero.org/library/Zero-to-Monero-2-0-0.pdf) - Detailed ring signature construction
- [Bulletproofs Paper](https://eprint.iacr.org/2017/1066.pdf) - Range proof system
