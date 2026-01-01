# Cryptographic Architecture

This document describes Botho's cryptographic primitives, key hierarchy, and signature schemes for external security auditors.

## Table of Contents

1. [Key Hierarchy](#key-hierarchy)
2. [Stealth Addresses](#stealth-addresses)
3. [Ring Signatures (CLSAG)](#ring-signatures-clsag)
4. [Post-Quantum Extensions (LION)](#post-quantum-extensions-lion)
5. [Pedersen Commitments](#pedersen-commitments)
6. [Range Proofs (Bulletproofs)](#range-proofs-bulletproofs)
7. [Domain Separators](#domain-separators)

---

## Key Hierarchy

### Overview

Botho derives all keys from a single BIP39 mnemonic using SLIP-10 hardened derivation.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           KEY DERIVATION HIERARCHY                           │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐   │
│  │                     BIP39 Mnemonic (24 words)                         │   │
│  │                         entropy: 256 bits                             │   │
│  └───────────────────────────────┬──────────────────────────────────────┘   │
│                                  │                                          │
│                                  ▼ PBKDF2-SHA512(mnemonic, "TREZOR", 2048)  │
│                          ┌──────────────┐                                   │
│                          │  512-bit     │                                   │
│                          │  Seed        │                                   │
│                          └──────┬───────┘                                   │
│                                 │                                           │
│              ┌──────────────────┴──────────────────┐                       │
│              │       SLIP-10 Derivation            │                       │
│              │       Path: m/44'/866'/idx'         │                       │
│              └──────────────────┬──────────────────┘                       │
│                                 │                                           │
│         ┌───────────────────────┴───────────────────────┐                  │
│         │                                               │                  │
│         ▼                                               ▼                  │
│  ┌─────────────────┐                           ┌─────────────────┐         │
│  │  Slip10Key      │                           │  Slip10Key      │         │
│  │  (View)         │                           │  (Spend)        │         │
│  └────────┬────────┘                           └────────┬────────┘         │
│           │                                             │                  │
│           ▼ HKDF-SHA512                                 ▼ HKDF-SHA512      │
│           "botho-ristretto255-view"                     "botho-ristretto255-spend"
│           │                                             │                  │
│           ▼                                             ▼                  │
│  ┌─────────────────┐                           ┌─────────────────┐         │
│  │ RootViewPrivate │                           │RootSpendPrivate │         │
│  │  (scalar a)     │                           │  (scalar b)     │         │
│  │  A = a*G        │                           │  B = b*G        │         │
│  └─────────────────┘                           └─────────────────┘         │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Key Components

| Component | Type | Purpose | Location |
|-----------|------|---------|----------|
| `Slip10Key` | `[u8; 32]` with Zeroizing | SLIP-10 derived key material | `core/src/slip10/mod.rs` |
| `RootViewPrivate` | Ristretto scalar | View key for scanning outputs | `account-keys/src/account_keys.rs` |
| `RootSpendPrivate` | Ristretto scalar | Spend key for signing transactions | `account-keys/src/account_keys.rs` |
| `PublicAddress` | (C, D) point pair | Subaddress for receiving payments | `account-keys/src/account_keys.rs` |

### Subaddress Derivation

Subaddresses allow a wallet to generate unlimited unlinkable addresses from a single master key pair.

```
Subaddress Index: i

View subkey:   c_i = a + Hs("SubAddr" || a || i)
Spend subkey:  d_i = b + Hs("SubAddr" || a || i)

Public Address:
  C_i = c_i * G
  D_i = d_i * G
```

**Security Properties:**
- Subaddresses are unlinkable without the view key
- The view key reveals which subaddress received a payment
- Only the spend key can authorize spending

---

## Stealth Addresses

Stealth addresses (one-time keys) ensure that no public address ever appears on-chain.

### Protocol

```
┌────────────────────────────────────────────────────────────────────────────┐
│                        STEALTH ADDRESS PROTOCOL                             │
│                                                                             │
│  SENDER (knows recipient's public address (C, D)):                         │
│  ──────────────────────────────────────────────────                        │
│  1. Generate random ephemeral scalar: r                                    │
│  2. Compute ephemeral public key: R = r * D                                │
│  3. Compute shared secret: shared = r * C                                  │
│  4. Derive one-time public key: P = Hs(shared) * G + D                     │
│  5. Include R in transaction output                                        │
│                                                                             │
│  RECIPIENT (has private keys (a, d)):                                      │
│  ─────────────────────────────────────                                     │
│  1. Compute shared secret: shared = a * R                                  │
│  2. Derive one-time public key: P' = Hs(shared) * G + D                    │
│  3. If P' matches output, this is our payment                              │
│  4. Compute one-time private key: x = Hs(shared) + d                       │
│                                                                             │
│  VERIFICATION:                                                             │
│  ─────────────                                                             │
│  x * G = (Hs(shared) + d) * G = Hs(shared) * G + D = P  ✓                 │
│                                                                             │
└────────────────────────────────────────────────────────────────────────────┘
```

### Key Formulas

| Operation | Formula | Notes |
|-----------|---------|-------|
| Ephemeral key | `R = r * D` | Random r per output |
| Shared secret | `shared = r * C = a * R` | ECDH key exchange |
| One-time public | `P = Hs(shared) * G + D` | Unlinkable to (C, D) |
| One-time private | `x = Hs(shared) + d` | Recoverable by recipient |

**Code Reference:** `crypto/ring-signature/src/onetime_keys.rs`

---

## Ring Signatures (CLSAG)

CLSAG (Concise Linkable Spontaneous Anonymous Group) signatures hide the real input among decoys while preventing double-spending.

### Structure

```
┌────────────────────────────────────────────────────────────────────────────┐
│                          CLSAG SIGNATURE                                    │
│                                                                             │
│  Ring: [P_0, P_1, ..., P_{n-1}]  (n = 20 public keys)                      │
│  Real index: π (secret)                                                    │
│  Message: m                                                                │
│                                                                             │
│  Signature Components:                                                     │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │  c_0        : Initial challenge (32 bytes)                          │  │
│  │  s[0..n]    : Responses, one per ring member (32 bytes each)        │  │
│  │  I          : Key image (32 bytes compressed point)                 │  │
│  │  D          : Auxiliary key image for commitment (32 bytes)         │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
│                                                                             │
│  Total size: ~700 bytes for ring size 20                                   │
│                                                                             │
└────────────────────────────────────────────────────────────────────────────┘
```

### Signing Algorithm

```
CLSAG.Sign(x, P[], real_index, message):
  // x = private key at real_index
  // P[] = ring of public keys

  // 1. Compute key image (prevents double-spend)
  I = x * Hp(P[real_index])

  // 2. Compute aggregation coefficients
  mu_P = H("CLSAG_agg_0" || P[] || I || commitment_key_image)
  mu_C = H("CLSAG_agg_1" || P[] || I || commitment_key_image)

  // 3. Generate random starting point
  alpha = random_scalar()
  L[real_index] = alpha * G
  R[real_index] = alpha * Hp(P[real_index])

  // 4. Complete the ring (challenge chain)
  for i in (real_index+1 .. real_index) mod n:
    c[i] = H("CLSAG_round" || P[] || L[i-1] || R[i-1] || message)

    // Aggregated public key
    W[i] = mu_P * P[i] + mu_C * Z[i]  // Z = commitment difference

    s[i] = random_scalar()
    L[i] = s[i] * G + c[i] * W[i]
    R[i] = s[i] * Hp(P[i]) + c[i] * (mu_P * I + mu_C * D)

  // 5. Close the loop
  c[real_index] = H("CLSAG_round" || P[] || L[n-1] || R[n-1] || message)
  s[real_index] = alpha - c[real_index] * (mu_P * x + mu_C * z)

  return (c_0, s[], I, D)
```

### Verification

```
CLSAG.Verify(signature, P[], message):
  (c_0, s[], I, D) = signature

  // Recompute aggregation coefficients
  mu_P = H("CLSAG_agg_0" || P[] || I || D)
  mu_C = H("CLSAG_agg_1" || P[] || I || D)

  c = c_0
  for i in 0..n:
    W[i] = mu_P * P[i] + mu_C * Z[i]
    L[i] = s[i] * G + c * W[i]
    R[i] = s[i] * Hp(P[i]) + c * (mu_P * I + mu_C * D)
    c = H("CLSAG_round" || P[] || L[i] || R[i] || message)

  return c == c_0  // Loop closure check
```

### Key Image

The key image `I = x * Hp(P)` is:
- **Unique** per output (deterministic from private key)
- **Unlinkable** to the public key (one-way function)
- **Trackable** to prevent double-spending

**Code Reference:** `crypto/ring-signature/src/ring_signature/clsag.rs`

---

## Post-Quantum Extensions (LION)

LION provides lattice-based ring signatures for quantum resistance.

### Parameters

| Parameter | Value | Description |
|-----------|-------|-------------|
| N | 256 | Ring dimension |
| Q | 8,380,417 | Prime modulus |
| K, L | 4 | Module dimensions |
| Ring Size | 7-11 | Decoys + real |

### Size Comparison

| Signature Type | Ring Size | Signature Size | Privacy Bits |
|----------------|-----------|----------------|--------------|
| CLSAG | 20 | ~700 bytes | 4.32 |
| LION | 11 | ~36 KB | 3.30 |

### Security Assumptions

- **Module-LWE**: Learning With Errors over module lattices
- **Module-SIS**: Short Integer Solution over module lattices
- **Target Security**: 128-bit post-quantum

### Hybrid Mode

When post-quantum security is enabled:

```
Transaction Signature = CLSAG + LION

Both signatures must verify for the transaction to be valid.
Classical-only clients can verify CLSAG.
PQ-aware clients verify both.
```

**Code Reference:** `crypto/lion/src/`

---

## Pedersen Commitments

Amount hiding uses Pedersen commitments with dual generators.

### Formula

```
Commitment: V = v * H + r * G

Where:
  v = amount (hidden)
  r = blinding factor (random scalar)
  G = generator point (standard Ristretto basepoint)
  H = secondary generator (nothing-up-my-sleeve derivation)
```

### Properties

| Property | Guarantee |
|----------|-----------|
| **Hiding** | Cannot determine v from V without r |
| **Binding** | Cannot find (v', r') ≠ (v, r) such that V = v'*H + r'*G |
| **Additive Homomorphism** | V₁ + V₂ = (v₁ + v₂)*H + (r₁ + r₂)*G |

### Value Conservation

Transaction validity requires:

```
sum(input_commitments) == sum(output_commitments) + fee_commitment

This proves: sum(inputs) == sum(outputs) + fee
Without revealing individual amounts.
```

**Code Reference:** `crypto/ring-signature/src/amount/commitment.rs`

---

## Range Proofs (Bulletproofs)

Bulletproofs prove that committed amounts are in valid range without revealing values.

### Properties

```
Range Proof proves: 0 <= amount < 2^64

Size: O(log n) where n = number of outputs
Aggregated verification for multiple outputs
```

### Verification

Each output commitment must have a valid Bulletproof proving non-negative amount.

**Code Reference:** `transaction/core/src/ring_ct/rct_bulletproofs.rs`

---

## Domain Separators

All hash operations use unique domain separators to prevent cross-protocol attacks.

### Key Derivation Separators

| Separator | Usage | Location |
|-----------|-------|----------|
| `botho-ristretto255-view` | View key from SLIP-10 | `core/src/slip10/mod.rs` |
| `botho-ristretto255-spend` | Spend key from SLIP-10 | `core/src/slip10/mod.rs` |
| `BOTHO_PQ_DOMAIN` | Post-quantum key derivation | `crypto/pq/src/lib.rs` |

### Signature Separators

| Separator | Usage | Location |
|-----------|-------|----------|
| `CLSAG_agg_0` | P-key aggregation coefficient | `clsag.rs` |
| `CLSAG_agg_1` | C-key aggregation coefficient | `clsag.rs` |
| `CLSAG_round` | Round hash | `clsag.rs` |

### Burn Address

The burn address uses a nothing-up-my-sleeve construction:

```
View private key: constant [1u8; 32]
Spend public key: hash-to-curve("botho-burn-address-v1")

Properties:
- Anyone can send to this address
- No one can spend (no corresponding private key exists)
- Fully auditable with view key
```

**Code Reference:** `account-keys/src/burn_address.rs`

---

## Security Audit Checklist

### Key Derivation
- [ ] BIP39 uses correct salt ("TREZOR")
- [ ] SLIP-10 implements hardened derivation correctly
- [ ] Domain separators are unique and collision-free
- [ ] Keys are zeroized after use

### Ring Signatures
- [ ] Key image computation is deterministic
- [ ] Ring size enforcement (minimum 20)
- [ ] Challenge chain closure verified
- [ ] Aggregation coefficients prevent malleability

### Commitments
- [ ] Generator H independent of G
- [ ] Blinding factors are random per commitment
- [ ] Value conservation enforced at validation

### Post-Quantum
- [ ] LION parameters provide 128-bit security
- [ ] Rejection sampling is uniform
- [ ] Hybrid signatures both verify
