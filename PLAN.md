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

4. **Wallet PQ Integration** (requires network protocol support)
   - [ ] Full PQ transaction building (UTXO selection, signing)
   - [ ] Scan both classical and PQ outputs
   - [ ] Show transaction type in history

### Lower Priority

5. **Dependency Modernization**
   - [ ] `slog` → `tracing` (botho/ migrated, inherited crates remain)
   - [ ] `lmdb-rkv` → `heed` or `redb` (still works with patch)

6. **Fee Estimation API**
   - [ ] Add `estimateFee` method to JSON-RPC
   - [ ] Update `botho send` to show memo fee breakdown

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
