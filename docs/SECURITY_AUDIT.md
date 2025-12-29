# Security Audit Report

**Date:** 2025-12-29
**Scope:** botho/, botho-wallet/ directories
**Status:** In Development
**Last Updated:** 2025-12-29

## Executive Summary

This security audit identified **2 critical**, **2 high**, and **4 medium** severity issues.

**Current Status:**
- Critical: 2/2 fixed
- High: 1/2 fixed
- Medium: 0/4 fixed

## Critical Issues

### 1. Signature Verification Not Implemented in Consensus Validator - FIXED

**Location:** `botho/src/mempool.rs`, `botho/src/transaction.rs`

**Issue:** Transfer transaction signature verification was not implemented.

**Fix Applied:**
- Added `TxInput::verify_signature()` method in `transaction.rs`
- Mempool now verifies Schnorrkel signatures against UTXO `target_key`
- Uses domain separator "botho-tx-v1" matching the signing code

**Status:** ✅ Fixed

---

### 2. Weak RNG in Wallet Stealth Output Generation - FIXED

**Location:** `botho-wallet/src/transaction.rs:70`

**Issue:** Used `rand::random()` instead of a cryptographically secure RNG.

**Fix Applied:**
```rust
use rand::{rngs::OsRng, RngCore};
let mut random_bytes = [0u8; 32];
OsRng.fill_bytes(&mut random_bytes);
hasher.update(random_bytes);
```

**Status:** ✅ Fixed

---

## High Severity Issues

### 3. No File Permission Restrictions on Windows

**Location:** `botho-wallet/src/storage.rs:150-153`

**Issue:** Non-Unix platforms fall back to `fs::write()` with no permission restrictions.

```rust
#[cfg(not(unix))]
{
    fs::write(path, json)?;
}
```

**Impact:** Windows users' wallet files could be readable by other users on the system.

**Recommendation:** Use Windows ACL APIs to restrict file access on Windows platforms.

---

### 4. Float Precision Loss in Amount Conversion - FIXED

**Location:** `botho/src/commands/send.rs`

**Issue:** Float-to-u64 conversion could lose precision or overflow.

**Fix Applied:**
- Added maximum amount limit (18,000 credits)
- Uses explicit `.round()` for precision
- Validates conversion result before casting

**Status:** ✅ Fixed

---

## Medium Severity Issues

### 5. Timestamp Monotonicity Not Enforced

**Location:** `botho/src/consensus/validation.rs:165-167`

**Issue:** Block timestamps are not validated to be after the parent block's timestamp.

```rust
// Note: We don't check if timestamp is before parent here because
// that requires looking up the parent block. The consensus layer
// should handle this during block construction.
```

**Impact:** Could allow timestamp manipulation attacks affecting difficulty adjustment.

**Recommendation:** Add parent timestamp validation during block validation.

---

### 6. Abundant unwrap() Calls in Hot Paths

**Locations:**
- `botho/src/rpc/mod.rs`: 21+ unwrap() calls
- `botho/src/node/mod.rs`: 10+ unwrap() calls
- `botho/src/commands/run.rs`: 8+ unwrap() calls

**Impact:** Panics could cause denial of service if invariants are violated.

**Recommendation:** Replace `unwrap()` with proper error handling, especially for:
- RwLock acquisitions (can be poisoned)
- Header parsing for CORS
- Serialization operations

---

### 7. No Maximum Transaction Size Validation

**Location:** Multiple RPC and network handlers

**Issue:** Transactions can be arbitrarily large before deserialization.

**Impact:** Memory exhaustion attacks through large transaction payloads.

**Recommendation:** Add transaction size limit check before deserialization.

---

### 8. Mempool Signature Verification Missing

**Location:** `botho/src/mempool.rs:57-122`

**Issue:** The mempool validates UTXO existence and amounts but does not verify transaction signatures.

**Impact:** Combined with Issue #1, unsigned transactions can enter the mempool.

**Recommendation:** Add signature verification in `add_tx()`.

---

## Low Severity Issues

### 9. Discovery State Parsing Assumption

**Location:** `botho-wallet/src/storage.rs:214`

**Issue:** Uses `split(':')` to parse nonce:ciphertext, assuming no colons in hex string.

**Impact:** Unlikely to cause issues with hex encoding, but inelegant.

**Recommendation:** Use a more robust parsing method or a structured format.

---

### 10. No Rate Limiting on Password Attempts

**Location:** `botho-wallet/src/storage.rs`

**Issue:** No rate limiting on decryption attempts.

**Impact:** Offline brute-force attacks on wallet files.

**Recommendation:** Consider adding attempt limiting or increasing Argon2 work factor.

---

## Positive Security Findings

### Cryptography
- Uses `OsRng` (CSPRNG) in main node for key generation
- ChaCha20-Poly1305 authenticated encryption for wallet storage
- Argon2id key derivation with reasonable parameters (64MB, 3 iterations)
- Proper domain separator for transaction signing ("botho-tx-v1")

### Network Security
- Request size limit: 1KB max
- Response size limit: 10MB max
- Per-peer rate limiting: 60 requests/minute
- Gossipsub validation mode set to "Strict"
- Peer reputation tracking with ban threshold

### Storage Security
- Unix file permissions (0600) for wallet files
- No plaintext secrets in memory (secure zeroing attempted)

### Transaction Safety
- Overflow protection using `checked_add()` and `saturating_add()`
- Double-spend detection in mempool
- Fee validation: `output_sum + fee <= input_sum`

### No Unsafe Code
- No `unsafe` blocks found in `botho/` directory

---

## Summary by Severity

| Severity | Count | Fixed | Status |
|----------|-------|-------|--------|
| Critical | 2     | 2     | ✅ All fixed |
| High     | 2     | 1     | 1 remaining (Windows permissions) |
| Medium   | 4     | 0     | Should fix |
| Low      | 2     | 0     | Nice to fix |

---

## Recommendations Priority

1. ~~**Immediate:** Implement signature verification (Issues #1, #8)~~ ✅ Done
2. ~~**Immediate:** Fix weak RNG in wallet (Issue #2)~~ ✅ Done
3. **Before Release:** Fix Windows file permissions (Issue #3)
4. ~~**Before Release:** Fix amount conversion (Issue #4)~~ ✅ Done
5. **Before Release:** Add timestamp validation (Issue #5)
6. **Ongoing:** Replace unwrap() calls with proper error handling (Issue #6)
