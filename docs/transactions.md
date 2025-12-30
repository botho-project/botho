# Transaction Types

Botho supports three transaction types, each designed for specific use cases with different privacy and size trade-offs. All transaction types use post-quantum cryptography throughout.

## Overview

| Property | Minting | Standard | Private |
|----------|---------|----------|---------|
| **Purpose** | Block rewards | Normal transfers | Maximum privacy |
| **Recipient Privacy** | Hidden (stealth) | Hidden (stealth) | Hidden (stealth) |
| **Amount Privacy** | Public | Hidden | Hidden |
| **Sender Privacy** | Known (minter) | Visible | Hidden (ring) |
| **Stealth Address** | ML-KEM | ML-KEM | ML-KEM |
| **Amount Encoding** | Plaintext | Pedersen + Bulletproofs | Pedersen + Bulletproofs |
| **Authorization** | ML-DSA | ML-DSA | LION (ring size 7) |
| **Cluster Tags** | Creates new cluster | Inherited from inputs | Inherited from inputs |
| **Approx. Size** | ~1.5 KB | ~3-4 KB | ~22 KB |

## Cryptographic Primitives

All transaction types share a common post-quantum foundation:

| Primitive | Algorithm | Purpose | Size |
|-----------|-----------|---------|------|
| Stealth addresses | ML-KEM-768 | Recipient unlinkability | 1088 B ciphertext |
| Amount commitments | Pedersen | Hide transaction amounts | 32 B |
| Range proofs | Bulletproofs | Prove amounts are valid | ~700 B |
| Single-signer auth | ML-DSA-65 | Authorize spending | 3309 B |
| Ring signatures | LION | Hide sender in ring | ~17.5 KB |
| Key images | LION-derived | Prevent double-spending | 1312 B |

---

## Minting Transactions

Minting transactions create new coins as block rewards. They have no inputs (coins come from the protocol itself).

### Properties

- **Inputs**: None (coinbase)
- **Outputs**: Stealth addresses with plaintext amounts
- **Authorization**: ML-DSA signature from the minter
- **Cluster**: Creates a new cluster origin

### Structure

```
MintingTx {
    block_height: u64,
    minter_proof: MinterProof,      // PoW solution + ML-DSA signature
    outputs: Vec<MintingOutput>,
    cluster_id: ClusterId,          // New cluster created by this mint
}

MintingOutput {
    target_key: PqStealthAddress,   // ML-KEM one-time destination
    public_key: MlKem768Ciphertext, // Ephemeral key for recipient
    amount: u64,                    // Plaintext (auditable)
    cluster_tag: ClusterTag,        // Initial tag weight = 1.0
}
```

### Why Amounts Are Public

Minting amounts must be publicly verifiable to:
- Audit total coin supply
- Verify emission schedule compliance
- Detect inflation bugs

Recipient privacy is still preserved via stealth addresses.

### Cluster Initialization

Each minting transaction creates a new cluster:
- Cluster ID derived from: `H(block_height || minter_pubkey || output_index)`
- Initial cluster tag weight: 1.0 (100%)
- All descendant coins inherit this cluster attribution

---

## Standard Transactions

Standard transactions transfer value with hidden amounts but visible sender identity. This is the most efficient transaction type for most use cases.

### Properties

- **Inputs**: References to previous outputs + ML-DSA signatures
- **Outputs**: Stealth addresses with committed amounts
- **Authorization**: ML-DSA signature per input (sender is identifiable)
- **Amount Privacy**: Hidden via Pedersen commitments + Bulletproofs

### Structure

```
StandardTx {
    inputs: Vec<StandardInput>,
    outputs: Vec<StandardOutput>,
    fee: CommittedAmount,           // Fee as Pedersen commitment
    bulletproofs: AggregatedProof,  // Range proofs for all outputs
}

StandardInput {
    tx_ref: TxOutRef,               // Previous output reference
    signature: MlDsa65Signature,    // Authorizes spending
}

StandardOutput {
    target_key: PqStealthAddress,   // ML-KEM one-time destination
    public_key: MlKem768Ciphertext, // Ephemeral key for recipient
    commitment: PedersenCommitment, // C = v*H + b*G
    encrypted_amount: [u8; 32],     // For recipient decryption
    cluster_tags: ClusterTagVector, // Inherited/mixed from inputs
    memo: Option<EncryptedMemo>,    // Optional 66-byte encrypted memo
}
```

### Sender Visibility

In Standard transactions, the sender is identifiable because:
- Each input directly references a specific previous output
- The ML-DSA signature proves ownership of that specific output
- No decoys or ring structure obscures the true spender

### Use Cases

- **Most everyday transfers**: When sender privacy isn't critical
- **Business payments**: When you need audit trails
- **Lower fees**: Smaller transaction size means lower fees
- **Exchange deposits**: Exchanges may require identifiable senders

---

## Private Transactions

Private transactions provide maximum privacy by hiding the sender within a ring of possible signers using LION post-quantum ring signatures.

### Properties

- **Inputs**: Ring of 7 possible outputs + LION ring signature
- **Outputs**: Stealth addresses with committed amounts
- **Authorization**: LION ring signature (sender hidden among 7 members)
- **Amount Privacy**: Hidden via Pedersen commitments + Bulletproofs
- **Sender Privacy**: Hidden via ring signature (1-in-7 anonymity)

### Structure

```
PrivateTx {
    inputs: Vec<PrivateInput>,
    outputs: Vec<PrivateOutput>,    // Same as StandardOutput
    fee: CommittedAmount,
    bulletproofs: AggregatedProof,
}

PrivateInput {
    ring: [TxOutRef; 7],            // 7 possible source outputs
    key_image: LionKeyImage,        // Prevents double-spending
    signature: LionRingSignature,   // Proves ownership of ONE member
}

PrivateOutput {
    // Same structure as StandardOutput
    target_key: PqStealthAddress,
    public_key: MlKem768Ciphertext,
    commitment: PedersenCommitment,
    encrypted_amount: [u8; 32],
    cluster_tags: ClusterTagVector,
    memo: Option<EncryptedMemo>,
}
```

### LION Ring Signatures

LION (Lattice-based lInkable ring signatures fOr aNonymity) provides:

| Property | Description |
|----------|-------------|
| **Sender anonymity** | Signature proves ownership of 1-of-7 outputs without revealing which |
| **Linkability** | Key images prevent double-spending without revealing the signer |
| **Post-quantum security** | Based on Module-LWE, ~128-bit PQ security level |
| **Ring size** | Fixed at 7 members |

### Key Images

Key images are deterministic values derived from the secret key:

```
key_image = H(secret_key) * G_lattice
```

Properties:
- Same secret key always produces same key image
- Different secret keys produce different key images
- Cannot reverse-engineer secret key from key image
- Ledger maintains set of all spent key images

If a key image appears twice, the transaction is rejected as a double-spend.

### Decoy Selection (OSPEAD)

Botho uses OSPEAD (Optimal Selection Probability to Evade Analysis of Decoys):

- **Gamma distribution**: Matches decoy ages to real spending patterns
- **Age-weighted selection**: Newer outputs more likely to be selected
- **Effective anonymity**: At least 2 ring members appear equally likely to be the spender

### Use Cases

- **High-value transfers**: When privacy is worth the extra fee
- **Sensitive payments**: Medical, legal, personal matters
- **Long-term privacy**: Protection against "harvest now, decrypt later"
- **Whistleblowing**: When anonymity is critical

---

## Stealth Addresses (All Types)

All transaction types use ML-KEM-768 stealth addresses for recipient privacy.

### Protocol

**Sender (creating output):**
1. Recipient publishes: ML-KEM public key `K`, spend public key `S`
2. Sender encapsulates shared secret: `(ciphertext, ss) = ML-KEM.Encapsulate(K)`
3. Sender derives scalar: `Hs = H(ss || output_index)`
4. Sender computes one-time destination: `target = Hs * G + S`
5. Output contains: `(target_key, ciphertext)`

**Recipient (scanning):**
1. For each output, decapsulate: `ss = ML-KEM.Decapsulate(ciphertext, kem_secret_key)`
2. Derive scalar: `Hs = H(ss || output_index)`
3. Compute expected target: `target' = Hs * G + S`
4. If `target' == target_key`, output belongs to recipient
5. Spending key: `x = Hs + spend_secret_key`

### Properties

- **Unlinkability**: Each output has unique one-time address
- **Post-quantum**: ML-KEM-768 provides ~192-bit PQ security
- **Scan efficiency**: Only view key needed to scan, spend key stays cold

---

## Amount Privacy (Standard & Private)

Standard and Private transactions hide amounts using Pedersen commitments and Bulletproofs.

### Pedersen Commitments

Each output amount is encoded as:
```
C = v*H + b*G
```

Where:
- `v` = amount value
- `b` = random blinding factor
- `H` = value generator point
- `G` = blinding generator point

**Properties:**
- **Hiding**: Cannot determine `v` from `C` (information-theoretic)
- **Binding**: Cannot find different `(v', b')` with same `C` (computational)
- **Homomorphic**: `C1 + C2 = (v1+v2)*H + (b1+b2)*G`

### Balance Verification

Transaction validity requires:
```
sum(input_commitments) = sum(output_commitments) + fee_commitment
```

The homomorphic property allows verification without revealing values.

### Bulletproofs Range Proofs

Bulletproofs prove each output amount is in range `[0, 2^64)`:

- **Prevents overflow**: Can't create negative amounts
- **Zero-knowledge**: Reveals nothing about actual value
- **Aggregatable**: Multiple proofs combine efficiently
- **Size**: ~700 bytes for single proof, sub-linear growth for batches

---

## Cluster Tags (All Types)

Cluster tags track coin ancestry for the progressive fee system.

### How Tags Work

- **Minting**: Creates new cluster with weight 1.0
- **Spending**: Tags are mixed proportionally from inputs
- **Decay**: ~5% decay per transaction hop

### Tag Mixing Example

```
Input A: 100 BTH, tags = {cluster_1: 0.8, cluster_2: 0.2}
Input B: 50 BTH, tags = {cluster_3: 1.0}

Output (150 BTH):
  cluster_1: (100/150) * 0.8 * 0.95 = 0.507
  cluster_2: (100/150) * 0.2 * 0.95 = 0.127
  cluster_3: (50/150) * 1.0 * 0.95 = 0.317
```

### Fee Calculation

Cluster wealth determines progressive fee rate:
```
cluster_wealth = sum(all_utxos * tag_weight)
fee_rate = sigmoid(cluster_wealth)  // 0.05% to 30%
```

---

## Transaction Fees

| Type | Base Component | Size Component | Cluster Component |
|------|----------------|----------------|-------------------|
| Minting | 0 | 0 | 0 (creates new cluster) |
| Standard | 400 µBTH | ~3-4 KB × rate | 0.05% - 30% of value |
| Private | 400 µBTH | ~22 KB × rate | 0.05% - 30% of value |

Private transactions cost more due to larger LION signatures, but provide stronger privacy guarantees.

---

## Choosing a Transaction Type

```
                    ┌─────────────────────┐
                    │ Creating new coins? │
                    └─────────┬───────────┘
                              │
                    Yes ──────┴────── No
                      │                │
                      ▼                ▼
               ┌──────────┐    ┌───────────────────┐
               │ MINTING  │    │ Need sender       │
               └──────────┘    │ privacy?          │
                               └─────────┬─────────┘
                                         │
                               Yes ──────┴────── No
                                 │                │
                                 ▼                ▼
                          ┌──────────┐    ┌──────────┐
                          │ PRIVATE  │    │ STANDARD │
                          └──────────┘    └──────────┘
```

### Summary

| If you need... | Use |
|----------------|-----|
| Block rewards | Minting |
| Hidden amounts, lower fees | Standard |
| Hidden amounts + hidden sender | Private |
| Audit trail for compliance | Standard |
| Maximum anonymity | Private |

---

## Security Considerations

### Post-Quantum Security

All transaction types are designed for post-quantum security:

| Component | Classical Threat | Quantum Threat | Botho Protection |
|-----------|------------------|----------------|------------------|
| Stealth addresses | ECDH broken | Shor's algorithm | ML-KEM-768 |
| Signatures | Schnorr broken | Shor's algorithm | ML-DSA / LION |
| Commitments | Binding holds | Binding breakable | Pedersen (hiding still holds) |
| Ring signatures | MLSAG broken | Shor's algorithm | LION lattice |

**Note on Pedersen commitments**: While quantum computers could break the binding property, this only allows creating invalid proofs—it does not reveal hidden amounts. The hiding property is information-theoretic and remains secure.

### Transaction Graph Analysis

| Attack | Standard | Private |
|--------|----------|---------|
| Sender identification | Vulnerable | Mitigated (1-in-7) |
| Amount correlation | Protected | Protected |
| Timing analysis | Partially vulnerable | Partially vulnerable |
| Recipient identification | Protected | Protected |

For maximum privacy, use Private transactions and follow [privacy best practices](privacy.md#privacy-best-practices).

---

## Technical References

- [ML-KEM (FIPS 203)](https://csrc.nist.gov/pubs/fips/203/final) - Post-quantum key encapsulation
- [ML-DSA (FIPS 204)](https://csrc.nist.gov/pubs/fips/204/final) - Post-quantum signatures
- [LION Ring Signatures](https://link.springer.com/chapter/10.1007/978-981-95-3540-8_17) - Lattice-based linkable ring signatures
- [Bulletproofs](https://eprint.iacr.org/2017/1066.pdf) - Range proofs
- [Pedersen Commitments](https://link.springer.com/content/pdf/10.1007/3-540-46766-1_9.pdf) - Commitment scheme
