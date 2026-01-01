# Crypto Crates Test Coverage Baseline

**Generated:** 2025-12-31
**Tool:** cargo-llvm-cov v0.6.22

This document establishes the baseline test coverage for all crypto crates in the Botho project,
as required for external audit compliance (target: >80% coverage).

## Summary

| Metric | Count | Covered | Percentage |
|--------|-------|---------|------------|
| **Lines** | 7,159 | 5,482 | **76.6%** |
| Functions | 955 | 662 | 69.3% |
| Regions | 11,888 | 9,518 | 80.1% |

**Current Status:** Below 80% target. Priority areas identified below.

## Coverage by Crate

### Excellent Coverage (>90%)

| Crate | Lines | Covered | Percentage |
|-------|-------|---------|------------|
| `bth-crypto-sig` | 107 | 107 | **100%** |
| `bth-crypto-multisig` | 495 | 481 | **97.2%** |
| `bth-crypto-secp256k1` | 200 | 185 | **92.5%** |
| `bth-crypto-lion` | 1,631 | 1,463 | **89.7%** |

### Good Coverage (70-90%)

| Crate | Lines | Covered | Percentage |
|-------|-------|---------|------------|
| `bth-crypto-box` | 468 | 446 | **95.3%** |
| `bth-crypto-pq` | 550 | 459 | **83.5%** |
| `bth-crypto-ring-signature` | 1,254 | 1,116 | **89.0%** |
| `bth-crypto-keys` | 1,027 | 642 | **62.5%** |
| `bth-crypto-digestible` | 597 | 326 | **54.6%** |
| `bth-crypto-hashes` | 20 | 13 | **65.0%** |

### Needs Improvement (<70%)

| Crate | Lines | Covered | Percentage |
|-------|-------|---------|------------|
| `bth-crypto-digestible-signature` | 17 | 0 | **0%** |
| `bth-crypto-ring-signature-signer` | 88 | 0 | **0%** |

## Detailed File Coverage

### crypto/box (95.3% - 446/468 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| fixed_buffer.rs | 170 | 170 | 100% |
| hkdf_box.rs | 49 | 49 | 100% |
| lib.rs | 54 | 54 | 100% |
| traits.rs | 110 | 109 | 99% |
| versioned.rs | 85 | 64 | 75% |

**Gap:** `versioned.rs` error handling paths not fully tested.

### crypto/digestible (54.6% - 326/597 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| lib.rs | 280 | 198 | 70% |
| derive/src/lib.rs | 317 | 128 | 40% |

**Gap:** Derive macro code paths need additional test coverage.

### crypto/digestible-signature (0% - 0/17 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| lib.rs | 17 | 0 | 0% |

**Priority:** This crate has zero test coverage and needs tests added.

### crypto/hashes (65.0% - 13/20 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| pseudomerlin.rs | 20 | 13 | 65% |

**Gap:** Error handling and edge cases in pseudomerlin need testing.

### crypto/keys (62.5% - 642/1027 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| ristretto.rs | 490 | 409 | 83% |
| ed25519.rs | 309 | 104 | 33% |
| x25519.rs | 188 | 101 | 53% |
| traits.rs | 40 | 28 | 70% |

**Priority:** `ed25519.rs` needs significant additional tests.

### crypto/lion (89.7% - 1,463/1,631 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| lib.rs | 80 | 80 | 100% |
| lattice/mod.rs | 249 | 246 | 98% |
| ring_signature/verifier.rs | 183 | 180 | 98% |
| polynomial.rs | 505 | 440 | 87% |
| ring_signature/signer.rs | 173 | 166 | 95% |
| ring_signature/mod.rs | 267 | 248 | 92% |
| lattice/commitment.rs | 125 | 78 | 62% |
| params.rs | 31 | 25 | 80% |
| error.rs | 18 | 0 | 0% |

**Gap:** `error.rs` has no coverage (error handling tests needed).

### crypto/multisig (97.2% - 481/495 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| lib.rs | 495 | 481 | 97% |

Excellent coverage, nearly complete.

### crypto/pq (83.5% - 459/550 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| derive.rs | 107 | 107 | 100% |
| lib.rs | 66 | 66 | 100% |
| sig.rs | 197 | 156 | 79% |
| kem.rs | 180 | 130 | 72% |

**Gap:** Error handling in `kem.rs` and `sig.rs` needs additional tests.

### crypto/ring-signature (89.0% - 1,116/1,254 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| onetime_keys.rs | 220 | 220 | 100% |
| ring_signature/mlsag.rs | 165 | 165 | 100% |
| ring_signature/error.rs | 3 | 3 | 100% |
| ring_signature/clsag.rs | 303 | 297 | 98% |
| ring_signature/key_image.rs | 97 | 94 | 96% |
| ring_signature/mlsag_verify.rs | 34 | 33 | 97% |
| ring_signature/mlsag_sign.rs | 143 | 131 | 91% |
| ring_signature/curve_scalar.rs | 66 | 57 | 86% |
| amount/compressed_commitment.rs | 44 | 26 | 59% |
| amount/commitment.rs | 39 | 23 | 58% |
| ring_signature/mod.rs | 121 | 67 | 55% |
| proptest_fixtures.rs | 13 | 0 | 0% |
| lib.rs | 6 | 0 | 0% |

**Gap:** Amount commitment modules and batch verification functions need tests.

### crypto/ring-signature-signer (0% - 0/88 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| local_signer.rs | 39 | 0 | 0% |
| no_keys_ring_signer.rs | 31 | 0 | 0% |
| traits.rs | 18 | 0 | 0% |

**Priority:** This entire crate has zero test coverage.

### crypto/secp256k1 (92.5% - 185/200 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| lib.rs | 200 | 185 | 92% |

Good coverage with minor gaps in error handling.

### crypto/sig (100% - 107/107 lines)

| File | Lines | Covered | % |
|------|-------|---------|---|
| lib.rs | 107 | 107 | 100% |

Complete coverage.

## Priority Areas for Test Improvement

### Critical (0% coverage)
1. **`bth-crypto-ring-signature-signer`** - Entire crate untested (88 lines)
2. **`bth-crypto-digestible-signature`** - Entire crate untested (17 lines)
3. **`crypto/lion/src/error.rs`** - Error types untested (18 lines)

### High Priority (<50% coverage)
1. **`crypto/keys/src/ed25519.rs`** - Only 33% covered (309 lines)
2. **`crypto/digestible/derive/src/lib.rs`** - Only 40% covered (317 lines)

### Medium Priority (50-70% coverage)
1. **`crypto/ring-signature/src/amount/*`** - ~58% covered
2. **`crypto/keys/src/x25519.rs`** - 53% covered
3. **`crypto/ring-signature/src/ring_signature/mod.rs`** - 55% covered
4. **`crypto/lion/src/lattice/commitment.rs`** - 62% covered

## Running Coverage Locally

```bash
# Install cargo-llvm-cov if needed
cargo install cargo-llvm-cov
rustup component add llvm-tools-preview

# Run coverage for all crypto crates
cargo llvm-cov --package bth-crypto-box \
  --package bth-crypto-digestible \
  --package bth-crypto-digestible-signature \
  --package bth-crypto-hashes \
  --package bth-crypto-keys \
  --package bth-crypto-lion \
  --package bth-crypto-multisig \
  --package bth-crypto-pq \
  --package bth-crypto-ring-signature \
  --package bth-crypto-ring-signature-signer \
  --package bth-crypto-secp256k1 \
  --package bth-crypto-sig \
  --html --output-dir coverage-report

# View HTML report
open coverage-report/html/index.html

# Get JSON summary
cargo llvm-cov --package bth-crypto-* --json --summary-only
```

## Recommended Next Steps

1. **Add tests for `bth-crypto-ring-signature-signer`** - Critical for signing operations
2. **Add tests for `bth-crypto-digestible-signature`** - Small crate, quick win
3. **Improve `ed25519.rs` coverage** - Important for key operations
4. **Add error handling tests for `crypto/lion`** - Improve robustness
5. **Test batch verification in ring-signature** - Currently untested utility functions

## CI Integration

Coverage is tracked automatically via the `.github/workflows/coverage.yml` workflow.
The workflow:
- Runs on every push and PR
- Generates coverage reports
- Uploads HTML report as artifact
- Can optionally enforce minimum coverage thresholds
