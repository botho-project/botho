# Wallet Architecture

This document describes Botho's wallet security, key storage, and transaction signing for external security auditors.

## Table of Contents

1. [Wallet Overview](#wallet-overview)
2. [Key Storage](#key-storage)
3. [Memory Protection](#memory-protection)
4. [Transaction Signing](#transaction-signing)
5. [Output Scanning](#output-scanning)
6. [UTXO Management](#utxo-management)

---

## Wallet Overview

Botho wallets derive all keys from a single BIP39 mnemonic and support both classical and post-quantum cryptography.

### Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              WALLET                                          │
│                                                                              │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                      MNEMONIC (24 words)                              │  │
│  │                      Wrapped in Zeroizing<String>                     │  │
│  └──────────────────────────────┬────────────────────────────────────────┘  │
│                                 │                                           │
│              ┌──────────────────┴──────────────────┐                       │
│              │                                     │                       │
│              ▼                                     ▼                       │
│  ┌──────────────────────┐             ┌──────────────────────┐            │
│  │   CLASSICAL PATH     │             │   POST-QUANTUM PATH  │            │
│  │                      │             │                      │            │
│  │  SLIP-10 Derivation  │             │  HKDF Derivation     │            │
│  │  m/44'/866'/0'       │             │  "botho-pq-v1"       │            │
│  │                      │             │                      │            │
│  │  ┌────────────────┐  │             │  ┌────────────────┐  │            │
│  │  │ RootViewPrivate│  │             │  │ ML-KEM-768     │  │            │
│  │  │ RootSpendPriv  │  │             │  │ (Encapsulation)│  │            │
│  │  └────────────────┘  │             │  └────────────────┘  │            │
│  │                      │             │                      │            │
│  │  ┌────────────────┐  │             │  ┌────────────────┐  │            │
│  │  │ AccountKey     │  │             │  │ ML-DSA-65      │  │            │
│  │  │ (view+spend)   │  │             │  │ (Signatures)   │  │            │
│  │  └────────────────┘  │             │  └────────────────┘  │            │
│  │                      │             │                      │            │
│  └──────────────────────┘             └──────────────────────┘            │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Key Components

| Component | Type | Purpose | Location |
|-----------|------|---------|----------|
| `Wallet` | Struct | Main wallet interface | `botho/src/wallet.rs` |
| `AccountKey` | Struct | Classical view+spend key pair | `account-keys/src/account_keys.rs` |
| `MlKem768KeyPair` | Struct | Post-quantum key encapsulation | `crypto/pq/src/kem.rs` |
| `MlDsa65KeyPair` | Struct | Post-quantum signatures | `crypto/pq/src/sig.rs` |

**Code Reference:** `botho/src/wallet.rs`

---

## Key Storage

### Storage Hierarchy

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        KEY STORAGE HIERARCHY                                 │
│                                                                              │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                     BIP39 Mnemonic (24 words)                         │  │
│  │                     Entropy: 256 bits                                 │  │
│  │                     Wrapped: Zeroizing<String>                        │  │
│  └──────────────────────────────┬────────────────────────────────────────┘  │
│                                 │                                           │
│                                 ▼ PBKDF2(mnemonic, "TREZOR", 2048)          │
│                         ┌──────────────┐                                    │
│                         │  512-bit     │                                    │
│                         │  Master Seed │                                    │
│                         └──────┬───────┘                                    │
│                                │                                            │
│         ┌──────────────────────┼──────────────────────┐                    │
│         │                      │                      │                    │
│         ▼                      ▼                      ▼                    │
│  ┌──────────────┐      ┌──────────────┐      ┌──────────────┐             │
│  │  View Key    │      │  Spend Key   │      │  PQ Keys     │             │
│  │              │      │              │      │              │             │
│  │ SLIP-10 +    │      │ SLIP-10 +    │      │ HKDF +       │             │
│  │ HKDF-SHA512  │      │ HKDF-SHA512  │      │ ML-KEM/DSA   │             │
│  │ "...-view"   │      │ "...-spend"  │      │ "botho-pq"   │             │
│  └──────────────┘      └──────────────┘      └──────────────┘             │
│                                                                              │
│  Subaddress Derivation (from View + Spend):                                 │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  For index i:                                                         │  │
│  │    subkey_scalar = Hs("SubAddr" || view_private || i)                 │  │
│  │    C_i = (view_private + subkey_scalar) * G                           │  │
│  │    D_i = (spend_private + subkey_scalar) * G                          │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Key Sizes

| Key Type | Public Size | Secret Size | Purpose |
|----------|-------------|-------------|---------|
| Classical View | 32 bytes | 32 bytes | Scanning outputs |
| Classical Spend | 32 bytes | 32 bytes | Signing transactions |
| ML-KEM-768 | 1,184 bytes | 2,400 bytes | PQ key exchange |
| ML-DSA-65 | 1,952 bytes | 4,032 bytes | PQ signatures |

**Code Reference:** `account-keys/src/account_keys.rs`

---

## Memory Protection

### Zeroization

All sensitive key material uses the `zeroize` crate for secure memory clearing.

```rust
#[derive(Zeroize, ZeroizeOnDrop)]
struct Slip10Key([u8; 32]);

// Mnemonic wrapped in Zeroizing
pub struct Wallet {
    mnemonic: Zeroizing<String>,
    account_key: AccountKey,
    // ...
}
```

### Zeroization Points

| Data | Protection | Location |
|------|------------|----------|
| Mnemonic | `Zeroizing<String>` | `wallet.rs` |
| SLIP-10 Keys | `#[zeroize(drop)]` | `slip10/mod.rs` |
| Private Scalars | `Zeroize` derive | `account_keys.rs` |
| Ephemeral Keys | Explicit zeroize | `transaction.rs` |

### Memory Safety Rules

1. **Never log private keys** - All logging filters sensitive data
2. **Zeroize on drop** - All private keys implement `ZeroizeOnDrop`
3. **No plaintext storage** - Keys only exist decrypted in memory
4. **Minimize lifetime** - Keys decrypted only when needed

**Code Reference:** `account-keys/src/account_keys.rs:123-145`

---

## Transaction Signing

### Signing Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        TRANSACTION SIGNING FLOW                              │
│                                                                              │
│  1. SELECT INPUTS                                                            │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  - Query owned UTXOs from ledger                                      │  │
│  │  - Select outputs with sufficient value                               │  │
│  │  - Compute total input amount                                         │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│  2. SELECT DECOYS                       ▼                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  - Use GammaDecoySelector for each input                              │  │
│  │  - Select 19 decoys per real input (ring size 20)                     │  │
│  │  - Ensure cluster similarity >= 0.7                                   │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│  3. CONSTRUCT OUTPUTS                   ▼                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  For each output:                                                     │  │
│  │    - Generate ephemeral scalar r                                      │  │
│  │    - Compute stealth address P = Hs(r*C)*G + D                        │  │
│  │    - Create Pedersen commitment V = v*H + r'*G                        │  │
│  │    - Generate Bulletproof range proof                                 │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│  4. COMPUTE KEY IMAGES                  ▼                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  For each input:                                                      │  │
│  │    - Recover one-time private key x = Hs(a*R) + d                     │  │
│  │    - Compute key image I = x * Hp(P)                                  │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│  5. SIGN WITH CLSAG                     ▼                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  For each input:                                                      │  │
│  │    - Create ring [P_0, ..., P_19] with real at random index           │  │
│  │    - Generate CLSAG signature (c_0, s[], I, D)                        │  │
│  │    - Verify signature locally before broadcast                        │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│  6. (OPTIONAL) SIGN WITH LION           ▼                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  If PQ mode enabled:                                                  │  │
│  │    - Select 10 decoys (ring size 11 for LION)                         │  │
│  │    - Generate LION lattice ring signature                             │  │
│  │    - Attach as secondary signature                                    │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Ring Composition

| Parameter | CLSAG | LION |
|-----------|-------|------|
| Ring Size | 20 | 11 |
| Real Inputs | 1 | 1 |
| Decoys | 19 | 10 |
| Min Ring Size | 20 | 11 |

### Key Image Computation

```rust
fn compute_key_image(
    one_time_private_key: &Scalar,
    public_key: &RistrettoPoint,
) -> KeyImage {
    // I = x * Hp(P)
    let hp_p = hash_to_point(public_key);
    one_time_private_key * hp_p
}
```

**Code Reference:** `botho/src/transaction.rs`

---

## Output Scanning

### Scanning Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         OUTPUT SCANNING                                      │
│                                                                              │
│  For each transaction output:                                                │
│                                                                              │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  1. COMPUTE SHARED SECRET                                             │  │
│  │     shared = view_private * ephemeral_public (a * R)                  │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  2. DERIVE EXPECTED PUBLIC KEY                                        │  │
│  │     P' = Hs(shared) * G + spend_public                                │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  3. CHECK MATCH                                                       │  │
│  │     if P' == output.public_key:                                       │  │
│  │       → This output belongs to us                                     │  │
│  │       → Compute one-time private key for spending                     │  │
│  └──────────────────────────────────────┬────────────────────────────────┘  │
│                                         │                                    │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │  4. RECOVER AMOUNT (if match)                                         │  │
│  │     amount = decrypt(output.encrypted_amount, shared_secret)          │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Scanning Performance

| Approach | Computation per Output |
|----------|----------------------|
| View Key Scan | 1 scalar mult + 1 point add + 1 compare |
| Full Sync | ~1ms per output (single-threaded) |

### View Key Properties

- **Can detect**: Which outputs belong to wallet
- **Can decrypt**: Output amounts
- **Cannot**: Spend outputs (requires spend key)
- **Cannot**: See transaction graph (only incoming payments)

**Code Reference:** `botho/src/wallet.rs`

---

## UTXO Management

### Decoy Selection

Botho uses gamma distribution-based decoy selection for ring composition.

```rust
struct GammaDecoySelector {
    shape: f64,    // k = 19.28
    scale: f64,    // θ = 1.61 days
}

impl GammaDecoySelector {
    fn select_decoys(
        &self,
        real_output: &Output,
        available_outputs: &[Output],
        ring_size: usize,
    ) -> Vec<Output> {
        // Select outputs with age distribution matching observed spending
        // patterns, filtered by cluster similarity
    }
}
```

### Selection Criteria

| Criterion | Requirement |
|-----------|-------------|
| Age Distribution | Gamma(19.28, 1.61 days) |
| Cluster Similarity | >= 0.7 (cosine + dominant match) |
| Uniqueness | No duplicate outputs in ring |
| Availability | Output must be unspent in UTXO set |

### Cluster Wealth Tracking

```rust
fn compute_cluster_wealth(&self, cluster_tags: &ClusterTags) -> Amount {
    // Sum of all UTXOs with matching cluster profile
    // Used for progressive fee calculation
}
```

**Code Reference:** `botho/src/decoy_selection.rs`

---

## Fee Calculation

### Progressive Fee System

Fees are calculated based on transaction characteristics and wallet wealth:

```rust
fn calculate_fee(
    tx_type: TxType,
    memo_count: u32,
    cluster_wealth: Amount,
) -> Amount {
    let base_fee = match tx_type {
        TxType::Plain => 1_000_000,       // 0.001 BTH
        TxType::Hidden => 2_000_000,      // 0.002 BTH
        TxType::PqHidden => 5_000_000,    // 0.005 BTH
    };

    let memo_factor = 1.0 + (memo_count as f64 * 0.1);
    let wealth_factor = progressive_curve(cluster_wealth);

    (base_fee as f64 * memo_factor * wealth_factor) as Amount
}
```

### Cluster Tag Decay

Cluster tags decay over time to prevent wealth tracking:

```rust
const DEFAULT_DECAY_RATE: u64 = 50_000; // 5% per epoch

fn decay_cluster_tag(tag: &mut ClusterTag, epochs_passed: u64) {
    for _ in 0..epochs_passed {
        tag.weight = (tag.weight * (1_000_000 - DEFAULT_DECAY_RATE)) / 1_000_000;
    }
}
```

**Code Reference:** `cluster-tax/src/fee_curve.rs`

---

## Security Audit Checklist

### Key Storage
- [ ] Mnemonic wrapped in `Zeroizing<String>`
- [ ] All private keys implement `ZeroizeOnDrop`
- [ ] No plaintext keys in logs

### Memory Protection
- [ ] Keys zeroized after use
- [ ] No key material in error messages
- [ ] Secure random generation for all ephemeral keys

### Transaction Signing
- [ ] Ring size enforced (minimum 20)
- [ ] Key image computation is deterministic
- [ ] Signatures verified before broadcast

### Output Scanning
- [ ] View key isolation from spend key
- [ ] Amount decryption uses correct shared secret
- [ ] Subaddress matching works correctly

### Decoy Selection
- [ ] Gamma distribution parameters are appropriate
- [ ] Cluster similarity prevents fingerprinting
- [ ] No bias toward real output position

### Fee Calculation
- [ ] Progressive curve is correctly implemented
- [ ] Cluster wealth computed accurately
- [ ] Minimum fee enforced
