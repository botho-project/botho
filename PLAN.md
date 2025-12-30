# Botho - Work In Progress & Roadmap

## Current Status: v0.1.0-beta Ready

Core functionality complete. See README.md for features and usage.

---

## Remaining Work

### Medium Priority

1. **Web Dashboard Polish**
   - [x] Wallet page: Balance, transaction history, send modal
   - [x] Minting page: Minting controls with SCP visualization
   - [x] Network page: Interactive SCP consensus visualization
   - [x] Ledger page: Block explorer (desktop wallet)
   - [x] BIP39 mnemonic generation with optional password encryption
   - [ ] Real-time updates via WebSocket/SSE
   - [ ] Mobile responsiveness improvements

2. **Transaction Size Limits** ✓ COMPLETE
   - MAX_TRANSACTION_SIZE (100KB), MAX_BLOCK_SIZE (20MB), MAX_SCP_MESSAGE_SIZE (1MB)
   - Size checked before deserialization in gossip handler
   - gossipsub max_transmit_size configured at libp2p level

3. **Add Memo Field to TxOutput** ✓ COMPLETE
   - Botho's TxOutput now supports encrypted memos
   - Memo fee system is wired into mempool validation
   - Tasks:
     - [x] Add `e_memo: Option<EncryptedMemo>` to `TxOutput` in `botho/src/transaction.rs`
     - [x] Define `EncryptedMemo` type (66 bytes: 2-byte type + 64-byte encrypted payload)
     - [x] Update serialization/deserialization (bincode-compatible)
     - [x] Add memo creation helpers (`MemoPayload::destination()`, `encrypt()`)
     - [x] Wire memo counting into mempool validation
     - [x] Add `--memo` flag to `botho send` CLI
   - Implementation notes:
     - Uses AES-256-CTR encryption with HKDF-SHA512 key derivation
     - Shared secret: `create_shared_secret(recipient_view_key, tx_private_key)`
     - Recipient decrypts using: `create_shared_secret(output_public_key, view_private_key)`
     - Compatible with existing fee system in `bth-cluster-tax`

4. **Wallet PQ Integration** ✓ COMPLETE
   - [x] Full PQ transaction building (UTXO selection, dual signing)
   - [x] Stealth address output scanning with proper key recovery
   - [x] `pq_tx_submit` RPC endpoint for quantum-private transactions
   - [x] Extended OwnedUtxo with target_key, public_key, subaddress_index
   - [ ] Show transaction type in history (future enhancement)
   - Implementation notes:
     - `TransactionBuilder::build_pq_transfer()` creates QuantumPrivateTransaction
     - Dual signatures: Schnorr + ML-DSA-65 for each input
     - Bridge mode: Classical UTXOs derive PQ secrets via HKDF
     - `WalletScanner` uses proper stealth address detection

5. **PQ Security Hardening** ✓ COMPLETE
   - [x] Unified Schnorr implementations (consistent `b"botho-tx-v1"` domain separator)
   - [x] Classical/PQ layer binding (ephemeral key derived from random + shared_secret)
   - [x] BIP39 passphrase support for PQ key derivation (PBKDF2-HMAC-SHA512)
   - Implementation notes:
     - `quantum_private_validate.rs` now uses `verify_schnorrkel` instead of custom verification
     - `QuantumPrivateTxOutput::new()` binds layers via HKDF: `k = HKDF(random || pq_shared_secret)`
     - `QuantumSafeAccountKey::from_mnemonic_with_passphrase()` uses proper 64-byte BIP39 seed

### Lower Priority

6. **Dependency Modernization**
   - [ ] `slog` → `tracing` (botho/ migrated, inherited crates remain)
   - [x] `lmdb-rkv` removed (unused inherited crates deleted, botho uses `heed`)

7. **Fee Estimation API** ✓ COMPLETE
   - [x] `estimateFee` / `tx_estimateFee` RPC method implemented
   - [x] Returns: minimumFee, feeRateBps, recommendedFee, highPriorityFee
   - [x] `botho send` shows fee breakdown (type, rate, memo surcharge)

---

## Documentation

### Critical: Parameter Inconsistencies ✓ FIXED

- [x] **Sync minting.md with current monetary policy**
  - Fixed block time (20s → 60s)
  - Fixed total supply (~18M → ~100M BTH)
  - Fixed tail emission (0.6 → ~4.76 BTH/block)
  - Fixed halving period (4y → 2y)
  - Fixed fee destination (to minter → burned)
- [x] Fixed desktop wallet minting page (20s → 60s block time)
- [x] Fixed getting-started.md (Rust version 1.70 → 1.83, correct git URL)
- [x] Fixed configuration.md (added RPC port, CORS, correct ports)

### New Documentation Created

- [x] `docs/api.md` — Complete JSON-RPC and WebSocket API reference
- [x] `docs/troubleshooting.md` — Common issues and solutions
- [x] Updated `docs/README.md` and main `README.md` with new doc links

### Tier 1: Clarity & Onboarding ✓ COMPLETE

- [x] Create `docs/FAQ.md` — Frequently asked questions for newcomers
- [x] Create `docs/comparison.md` — Why Botho vs Monero, Zcash, Bitcoin
- [x] Create `docs/glossary.md` — Define technical terms (stealth address, ring signature, SCP, cluster tags, etc.)

### Tier 2: Developer Experience ✓ COMPLETE

- [x] Create `docs/developer-guide.md` — Build your first app on Botho (JS, Python, Rust examples)
- [x] Add RPC examples to `docs/api.md` — curl examples included
- [x] Create `docs/testing.md` — How to run and write tests

### Tier 3: Operations & Security ✓ COMPLETE

- [x] Create `docs/deployment.md` — systemd, Docker, nginx, monitoring, HA setup
- [x] Create `docs/security.md` — Key management, threat model, best practices
- [x] Create `docs/backup.md` — Wallet backup and recovery procedures

### Tier 4: Ecosystem Growth ✓ COMPLETE

- [x] Create `docs/exchange-integration.md` — Exchange listing guide with deposit/withdrawal handling
- [x] Create `docs/merchant-guide.md` — Merchant acceptance guide for e-commerce and POS

### Structural Improvements

- [x] Docs landing page on botho.io with navigation
- [ ] Add "Concepts" section — visual explainers for stealth addresses, ring sigs, SCP
- [ ] Add diagrams — transaction flow, fee calculation, consensus visualization
- [ ] Version docs to match releases

---

## Post-Quantum: Future Phases

### Phase 8: Lattice Ring Signatures (Research) — FEASIBILITY CONFIRMED

**Status:** Research confirms PQ ring signatures are viable for Botho. Awaiting mature implementations.

#### Feasibility Analysis (Dec 2024)

We analyzed the impact of integrating lattice-based ring signatures. Key findings:

**Transaction Size Comparison:**

| Transaction Type | Output | Input (ring-11) | Typical Tx (2-in, 2-out) |
|-----------------|--------|-----------------|--------------------------|
| Classical (current) | 72 B | ~1 KB | ~2.1 KB |
| PQ no-ring (current) | 1,160 B | 3,409 B | ~9.1 KB |
| PQ + Lion ring-11 | 1,160 B | ~12 KB | **~26 KB** |

**Blockchain Growth at Bitcoin-Scale (50% PQ mix):**

| Year | Tx/Day | Chain Size | Storage Cost |
|------|--------|------------|--------------|
| Y5 | 50K | 285 GB | $1.48/year |
| Y10 | 400K | 3.1 TB | $7.13/year |
| Y15 | 500K | 9.3 TB | $10.23/year |

**Verdict:** 15x larger than Bitcoin but economically negligible. Storage costs ~$10/year.

**SCP Consensus Impact:** None. SCP messages contain 41-byte tx hashes, not full transactions.
Validation overhead is 3x slower but uses only 13% of 5-second slot budget.

**Bandwidth Considerations:**
- Daily sync: 7 GB/day (10 min at 100 Mbps) ✓
- Initial sync: 8 days at 100 Mbps (vs Bitcoin's 13 hours)
- Mitigation: UTXO snapshots, compact block relay

#### Candidate Schemes

| Scheme | Size/Member | Notes | Paper |
|--------|-------------|-------|-------|
| [Lion (2025)](https://link.springer.com/chapter/10.1007/978-981-95-3540-8_17) | ~1.07 KB | Generic lattices, best security | ICICS 2025 |
| [Raptor](https://link.springer.com/chapter/10.1007/978-3-030-21568-2_6) | ~1.3 KB | First practical impl, NTRU-based | ACNS 2019 |
| [MatRiCT+](https://eprint.iacr.org/2019/1287) | Full RingCT | Complete protocol, 23ms verify | IEEE S&P 2022 |
| [2024 Efficient LRS](https://eprint.iacr.org/2024/553.pdf) | 32B pubkeys | 50% smaller than MatRiCT | ePrint 2024 |

#### Alternative: Zcash-Style STARKs

| Approach | Proof Size | Anonymity Set | Quantum Safe |
|----------|------------|---------------|--------------|
| PQ Ring Sigs (Lion) | ~12 KB/input | Ring of 11 | ✓ (lattice) |
| zk-STARKs | 50-200 KB/tx | Entire pool | ✓ (hash-based) |

STARKs offer unlimited anonymity but require architectural overhaul. Ring signatures
fit our existing model with acceptable size increase.

#### Implementation Path

1. **Now:** Current PQ transactions (quantum-safe, no ring privacy)
2. **2025-2026:** Monitor Lion/MatRiCT+ implementations for maturity
3. **Prototype:** Integrate lattice ring sig library when stable
4. **Hybrid:** Support both PQ-no-ring and PQ+ring transaction types
5. **Infrastructure:** Add compact block relay and UTXO snapshots

### Phase 9: Full PQ Privacy (Future)

| Feature | Classical | PQ Status |
|---------|-----------|-----------|
| Stealth addresses | ECDH | ML-KEM ✓ (done) |
| Spend signatures | Schnorr | ML-DSA ✓ (done) |
| Ring signatures | MLSAG | Lion/MatRiCT+ (research) |
| Amount hiding | Pedersen | Lattice commits (research) |
| Key images | Curve hash | Tied to ring sigs |

### Open Questions

1. **PQ Ring Integration**: Lion appears most promising. Need reference implementation.
2. **PQ Amount Hiding**: Pedersen commitments are ECDLP-based. MatRiCT+ includes lattice commitments.
3. **Address Size**: PQ addresses are ~4.4KB. Options: QR codes, address registry, hybrid derivation.
4. **Compact Blocks**: Required for 26 KB transactions. Implement before PQ rings.

---

## Version Roadmap

```
v0.1.0-beta  ← Current
├── Core functionality complete
├── PQ crypto working
├── Single seed node (seed.botho.io)
└── CLI wallet operational

v0.1.x (patches)
├── Bug fixes from beta feedback
├── Encrypted memos
└── Wallet UX improvements

v0.2.0
├── Multiple seed nodes
├── Mobile wallet support
├── Fuzz testing
└── Dashboard improvements

v1.0.0 (production)
├── External security audit
├── 6+ months stable operation
├── Community governance
└── Full documentation
```

---

## Deployment: Pending Items

### Seed Node (seed.botho.io)

- [x] EC2 instance running (i-03f2b4b35fa7e86ce, t3.large, 98.95.2.200)
- [x] Peer ID: 12D3KooWBrjTYjNrEwi9MM3AKFenmymyWVXtXbQiSx7eDnDwv9qQ
- [x] Ports open: 7100 (gossip), 7101 (RPC)
- [ ] CloudWatch monitoring configured
- [ ] Verify discoverability from external network

### Web (botho.io)

- [x] Cloudflare Pages deployed (botho-6cu.pages.dev)
- [x] Custom domain configured
- [x] SSL active

### Future Scaling

Add regional seed nodes when network grows:
- `seed-us.botho.io` (US-East)
- `seed-eu.botho.io` (EU-West)
- `seed-ap.botho.io` (AP-Southeast)

---

## Testing: Hardening Complete

- [x] Fuzz testing with cargo-fuzz for deserialization
  - 5 fuzz targets: Transaction, PQ Transaction, Block, PQ Keys, Network Messages
  - Located in `fuzz/` directory with README instructions
- [x] Property-based testing for crypto operations
  - 15 proptest properties for ML-KEM, ML-DSA, and key derivation
  - Located in `crypto/pq/tests/proptest_pq.rs`
- [x] Cross-implementation compatibility tests (botho vs transaction/core PQ types)
  - 14 compatibility tests verifying type consistency
  - Located in `botho/tests/pq_compatibility.rs`
- [ ] External security audit (required before v1.0)
