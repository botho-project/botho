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

### Lower Priority

5. **Dependency Modernization**
   - [ ] `slog` → `tracing` (botho/ migrated, inherited crates remain)
   - [x] `lmdb-rkv` removed (unused inherited crates deleted, botho uses `heed`)

6. **Fee Estimation API** ✓ COMPLETE
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

### Tier 1: Clarity & Onboarding

- [ ] Create `docs/FAQ.md` — Top 10 newcomer questions
- [ ] Create `docs/comparison.md` — Why Botho vs Monero, Zcash, Bitcoin
- [ ] Create `docs/glossary.md` — Define technical terms (stealth address, ring signature, SCP, cluster tags, etc.)

### Tier 2: Developer Experience

- [ ] Create `docs/developer-guide.md` — Build your first app on Botho
- [x] Add RPC examples to `docs/api.md` — curl examples included
- [ ] Create `docs/testing.md` — How to run and write tests

### Tier 3: Operations & Security

- [ ] Create `docs/deployment.md` — systemd, Docker, monitoring setup
- [ ] Create `docs/security.md` — Key management, threat model, best practices
- [ ] Create `docs/backup.md` — Wallet backup and recovery procedures

### Tier 4: Ecosystem Growth

- [ ] Exchange integration guide
- [ ] Merchant acceptance guide

### Structural Improvements

- [x] Docs landing page on botho.io with navigation
- [ ] Add "Concepts" section — visual explainers for stealth addresses, ring sigs, SCP
- [ ] Add diagrams — transaction flow, fee calculation, consensus visualization
- [ ] Version docs to match releases

---

## Post-Quantum: Future Phases

### Phase 8: Lattice Ring Signatures (Research)

No mature post-quantum ring signature scheme exists. Options under evaluation:

| Scheme | Size per member | Status |
|--------|-----------------|--------|
| [Raptor](https://github.com/zhenfeizhang/raptor) | ~1.3 KB | Research |
| [Lion](https://eprint.iacr.org/2024/553) | ~1.07 KB | Research |
| [MatRiCT+](https://eprint.iacr.org/2019/1287) | Full RingCT | Research |

### Phase 9: Full PQ Privacy (Future)

| Feature | Classical | PQ Status |
|---------|-----------|-----------|
| Stealth addresses | ECDH | ML-KEM (done) |
| Spend signatures | Schnorr | ML-DSA (done) |
| Ring signatures | MLSAG | Research needed |
| Amount hiding | Pedersen | Lattice commits (research) |
| Key images | Curve hash | Tied to ring sigs |

### Open Questions

1. **PQ Ring Signatures**: How do lattice-based ring sigs integrate with existing MLSAG?
2. **PQ Amount Hiding**: Pedersen commitments are ECDLP-based. Need lattice commitment scheme.
3. **Address Size**: PQ addresses are ~4.4KB. Options: QR codes, address registry, hybrid derivation.
4. **Signature Aggregation**: Can Dilithium signatures be aggregated to reduce overhead?

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

## Testing: Future Work

- [ ] Fuzz testing with cargo-fuzz for deserialization
- [ ] Property-based testing for crypto operations
- [ ] External security audit (required before v1.0)
- [ ] Cross-implementation compatibility tests (botho vs transaction/core PQ types)
