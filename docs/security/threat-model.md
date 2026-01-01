# Threat Model

This document describes Botho's threat model for external security auditors. It covers protected assets, adversary profiles, trust assumptions, attack surfaces, security properties, and mitigations.

## Table of Contents

- [Assets](#assets)
- [Adversaries](#adversaries)
- [Trust Assumptions](#trust-assumptions)
- [Attack Surface](#attack-surface)
- [Security Properties](#security-properties)
- [Mitigations](#mitigations)
- [Threat Matrix](#threat-matrix)

---

## Assets

The following assets require protection in the Botho system:

### 1. User Private Keys

| Key Type | Purpose | Storage Location | Compromise Impact |
|----------|---------|------------------|-------------------|
| Spend Private Key | Authorizes fund transfers | Encrypted wallet file | Complete fund loss |
| View Private Key | Scans incoming transactions | Encrypted wallet file | Privacy loss (incoming) |
| LION Private Key | PQ-Private transaction signing | Encrypted wallet file | Future PQ fund loss |
| Mnemonic (24 words) | Master seed for all keys | User responsibility | Complete fund loss |

**File references:**
- Key derivation: `botho/src/wallet.rs:42-76`
- Encrypted storage: `botho-wallet/src/storage.rs:164-383`

### 2. Transaction Privacy

| Privacy Goal | Technique | Protected Data |
|--------------|-----------|----------------|
| Recipient hiding | ML-KEM-768 stealth addresses | Recipient identity |
| Sender hiding | CLSAG/LION ring signatures | Sender identity |
| Amount hiding | Pedersen commitments + Bulletproofs | Transaction values |
| Memo privacy | AES-256-CTR encryption | Payment metadata |

**File references:**
- Stealth addresses: `crypto/ring-signature/src/pq_onetime_keys.rs`
- Ring signatures: `crypto/ring-signature/src/ring_signature/clsag.rs`, `crypto/lion/src/ring_signature/`
- Confidential amounts: `crypto/ring-signature/src/amount/commitment.rs`

### 3. Wallet Files

| Component | Protection | Location |
|-----------|------------|----------|
| Mnemonic | ChaCha20-Poly1305 + Argon2id | `~/.botho-wallet/wallet.enc` |
| Cache data | SQLite (unencrypted) | `~/.botho-wallet/cache.db` |
| Node config | TOML (may contain mnemonic) | `~/.botho/config.toml` |

**File references:**
- Wallet encryption: `botho-wallet/src/storage.rs:189-220`
- Argon2id parameters: `botho-wallet/src/storage.rs:26-29`

### 4. Network Integrity

| Component | Threat | Protection |
|-----------|--------|------------|
| Blockchain state | Fork attacks | SCP consensus |
| Transaction propagation | Censorship | Gossipsub flooding |
| Peer connections | Sybil attacks | Connection limiting |
| RPC endpoints | DoS attacks | Rate limiting |

---

## Adversaries

### 1. Passive Network Observer

**Capability:** Observes network traffic between nodes.

**Goals:**
- Link transactions to IP addresses
- Correlate transaction timing
- Build transaction graphs

**Cannot:**
- Decrypt transaction contents
- Determine sender/recipient from on-chain data
- Break ring signature anonymity

**Mitigation level:** Medium (network-level privacy requires Tor/VPN)

### 2. Active Network Attacker (MITM)

**Capability:** Intercepts and modifies network traffic.

**Goals:**
- Inject malicious transactions
- Eclipse target nodes
- Delay transaction propagation

**Cannot:**
- Forge valid transactions (no private keys)
- Break libp2p encryption
- Override consensus without quorum control

**Mitigation level:** High (cryptographic authentication)

### 3. Malicious Peer Node

**Capability:** Operates one or more network nodes.

**Goals:**
- Spam invalid transactions
- Waste target node resources
- Attempt Sybil attacks
- Selectively censor transactions

**Cannot:**
- Exceed per-IP connection limits (10 connections)
- Bypass rate limiting (60 requests/min/peer)
- Forge consensus messages without quorum

**Mitigation level:** High (rate limiting, connection limits)

### 4. Compromised RPC Node

**Capability:** Controls an RPC server that thin wallets connect to.

**Goals:**
- Return false balance information
- Withhold incoming transactions
- Track wallet queries
- Delay transaction broadcast

**Cannot:**
- Steal funds (no private keys)
- Forge transactions
- Break transaction privacy

**Mitigation level:** Medium (users should run own nodes for critical use)

### 5. Local Attacker with Filesystem Access

**Capability:** Read/write access to the user's filesystem.

**Goals:**
- Extract encrypted wallet file
- Brute-force wallet password
- Install keyloggers
- Modify node software

**Cannot (with proper security):**
- Decrypt wallet without password (Argon2id: 64MB, 3 iterations)
- Recover zeroized keys from memory
- Bypass exponential backoff (5 attempts, then lockout)

**Mitigation level:** Medium (depends on password strength)

### 6. Quantum Adversary (Future)

**Capability:** Access to cryptographically relevant quantum computer.

**Goals:**
- Break classical cryptography (ECDH, ECDSA)
- Harvest-now-decrypt-later attacks
- Forge classical ring signatures

**Cannot:**
- Break ML-KEM-768 stealth addresses
- Break LION ring signatures
- Break ML-DSA minting signatures
- Reverse hash functions

**Mitigation level:** High (PQ cryptography for critical paths)

| Component | Algorithm | Quantum Safety |
|-----------|-----------|----------------|
| Stealth addresses | ML-KEM-768 | Full |
| Minting signatures | ML-DSA-65 | Full |
| Standard-Private sender | CLSAG | Classical only |
| PQ-Private sender | LION | Full |
| Commitments (hiding) | Pedersen | Information-theoretic |
| Commitments (binding) | Pedersen | Classical only |

---

## Trust Assumptions

### Cryptographic Hardness

| Assumption | Algorithm | Implication if Broken |
|------------|-----------|----------------------|
| Discrete Log (DLOG) | curve25519 | CLSAG signatures forgeable |
| Decisional Diffie-Hellman (DDH) | curve25519 | Pedersen binding broken |
| Learning With Errors (LWE) | ML-KEM, ML-DSA, LION | PQ protections broken |
| Collision Resistance | SHA-256, Blake2b | PoW and hashing compromised |

### Consensus Assumptions

**SCP Quorum Honesty:**
- At least 2/3 of quorum members are honest (Byzantine threshold)
- Quorum configuration reflects actual network trust
- Quorum intersection property holds

**File reference:** `consensus/scp/src/quorum_set_ext.rs`

### Hardware/Environment

| Assumption | Risk if Violated |
|------------|------------------|
| Hardware RNG quality | Key generation predictable |
| No side-channel leakage | Timing attacks on signatures |
| Memory isolation | Key extraction from RAM |
| Secure boot chain | Compromised node software |

### Operational Assumptions

- Users protect their mnemonic phrases
- Users choose strong wallet passwords
- Node operators keep software updated
- RPC endpoints are properly firewalled

---

## Attack Surface

### 1. RPC API Attack Surface

**Endpoint:** HTTP JSON-RPC server (default port 7101)

| Entry Point | Input | Validation | Reference |
|-------------|-------|------------|-----------|
| `tx_submit` | Transaction hex | Structure, signatures, fees | `botho/src/rpc/mod.rs:507` |
| `pq_tx_submit` | PQ Transaction hex | PQ structure, LION signatures | `botho/src/rpc/mod.rs:508` |
| `exchange_registerViewKey` | View key hex | 64-char hex, valid curve point | `botho/src/rpc/mod.rs:527` |
| `getBlocks` | Block range | Bounds checking | `botho/src/rpc/mod.rs:493` |
| `cluster_getWealth` | Cluster ID | Numeric validation | `botho/src/rpc/mod.rs:534` |

**Protections:**
- HMAC-SHA256 authentication: `botho/src/rpc/auth.rs:144-271`
- Timestamp validation (±5 min): `botho/src/rpc/auth.rs:203-218`
- Per-key rate limiting: `botho/src/rpc/rate_limit.rs:22-33`
- CORS origin whitelist: `botho/src/rpc/mod.rs:258-273`

**Attack vectors:**
| Attack | Mitigation | Status |
|--------|------------|--------|
| Brute-force API keys | Rate limiting (100-10000/min by tier) | Mitigated |
| Replay attacks | Timestamp validation (5 min window) | Mitigated |
| CSRF | CORS origin checking | Mitigated |
| Resource exhaustion | Request size limits, rate limits | Mitigated |

### 2. P2P Protocol Attack Surface

**Protocol:** libp2p gossipsub over TCP

| Entry Point | Input | Validation | Reference |
|-------------|-------|------------|-----------|
| Block gossip | Block data | Full block validation | `botho/src/network/sync.rs` |
| Transaction gossip | Transaction data | Mempool validation | `botho/src/mempool.rs:325-432` |
| SCP messages | Consensus data | Ballot validation | `consensus/scp/src/msg.rs` |
| Sync requests | Height range | Rate limiting, size limits | `botho/src/network/sync.rs:26-49` |

**Protections:**
- Per-IP connection limit (10): `botho/src/network/connection_limiter.rs:21`
- Per-peer rate limit (60 req/min): `botho/src/network/sync.rs:37`
- Message size limits (1KB req, 10MB resp): `botho/src/network/sync.rs:31-34`
- Peer reputation tracking: `botho/src/network/mod.rs`

**Attack vectors:**
| Attack | Mitigation | Status |
|--------|------------|--------|
| Sybil (many fake peers) | Per-IP connection limits | Mitigated |
| Eclipse (isolate node) | Quorum diversity, peer rotation | Partial |
| Flood (spam messages) | Rate limiting per peer | Mitigated |
| Memory exhaustion | Message size limits | Mitigated |

### 3. Wallet File Attack Surface

**Files:** `~/.botho-wallet/wallet.enc`, `~/.botho/config.toml`

| Entry Point | Input | Validation | Reference |
|-------------|-------|------------|-----------|
| Wallet decryption | Password | Argon2id + rate limiting | `botho-wallet/src/storage.rs:278-325` |
| Config parsing | TOML file | Standard parsing | `botho/src/config.rs` |

**Protections:**
- Argon2id KDF (64MB, 3 iter, 4 parallel): `botho-wallet/src/storage.rs:26-29`
- ChaCha20-Poly1305 encryption: `botho-wallet/src/storage.rs:189-220`
- Exponential backoff (5 attempts → lockout): `botho-wallet/src/storage.rs:104-112`
- File permissions (600): `botho-wallet/src/storage.rs:340-346`
- Zeroizing memory wrapper: `botho/src/wallet.rs:42-43`

**Attack vectors:**
| Attack | Mitigation | Status |
|--------|------------|--------|
| Offline brute-force | Argon2id (64MB memory-hard) | Mitigated |
| Online brute-force | Rate limiting + lockout | Mitigated |
| Memory dump | Zeroizing wrapper | Mitigated |
| File permission bypass | OS-level (chmod 600) | Mitigated |

### 4. Browser/Desktop App Attack Surface

**Component:** Tauri desktop wallet

| Entry Point | Input | Validation | Reference |
|-------------|-------|------------|-----------|
| IPC commands | User actions | Tauri command validation | `web/packages/desktop/src-tauri/` |
| WebView content | HTML/JS | CSP, sandboxing | Tauri defaults |

**Attack vectors:**
| Attack | Mitigation | Status |
|--------|------------|--------|
| XSS | CSP, Tauri sandboxing | Mitigated |
| IPC injection | Command validation | Mitigated |
| Update MITM | Signed updates | Planned |

---

## Security Properties

### 1. Transaction Unlinkability

**Property:** An observer cannot determine if two transactions involve the same party.

**Formal statement:**
```
For transactions T1, T2 with parties (S1, R1) and (S2, R2):
Pr[adversary correctly determines S1 = S2 | blockchain] ≤ 1/ring_size + ε
Pr[adversary correctly determines R1 = R2 | blockchain] ≤ negl(λ)
```

**Achieved via:**
- Stealth addresses (one-time destination keys)
- Ring signatures (CLSAG ring=20, LION ring=11)
- Cluster-aware decoy selection (≥70% cosine similarity)

**Limitations:**
- Network-level correlation possible without Tor
- Cluster tag fingerprinting reduces effective ring size
- Exchange KYC can link identities

### 2. Amount Confidentiality

**Property:** Transaction amounts are hidden from all observers except sender and recipient.

**Formal statement:**
```
For commitment C = v*H + b*G:
Given C, adversary cannot determine v with probability > negl(λ)
Given C1, C2, adversary cannot determine if v1 = v2 with probability > 1/2 + negl(λ)
```

**Achieved via:**
- Pedersen commitments (information-theoretic hiding)
- Bulletproofs range proofs
- Homomorphic balance verification

**Limitations:**
- Minting transaction amounts are public (emission transparency)
- Pedersen binding is classically secure (quantum vulnerability)

### 3. Key Image Uniqueness (Double-Spend Prevention)

**Property:** Each transaction output can only be spent once.

**Formal statement:**
```
For output O spent in transaction T with key image K:
Any transaction T' spending O must use the same key image K
K is deterministically computed: K = x * Hp(P) where x is spend key, P is output key
```

**Achieved via:**
- Key image derivation: `crypto/ring-signature/src/ring_signature/key_image.rs`
- Ledger key image set: `botho/src/ledger/store.rs:800-845`
- Mempool key image tracking: `botho/src/mempool.rs:404-421`

**Validation order:**
1. Check key image not in mempool
2. Check key image not in ledger
3. Verify ring signature

### 4. Consensus Safety

**Property:** All honest nodes agree on the same transaction history.

**Formal statement:**
```
If honest node N1 accepts block B at height H:
All honest nodes eventually accept B at height H
No honest node accepts B' ≠ B at height H
```

**Achieved via:**
- SCP (Stellar Consensus Protocol)
- Quorum intersection requirement
- Byzantine fault tolerance (tolerates < 1/3 malicious)

**File reference:** `consensus/scp/src/slot.rs`

### 5. Consensus Liveness

**Property:** Valid transactions are eventually included in blocks.

**Formal statement:**
```
If transaction T is valid and broadcast at time t:
T is included in a block by time t + Δ with high probability
where Δ depends on network conditions and fee priority
```

**Achieved via:**
- Gossipsub message propagation
- Fee-priority mempool ordering
- Dynamic block timing (5-40s based on load)

---

## Mitigations

### By Threat Category

#### 1. Cryptographic Attacks

| Threat | Mitigation | Implementation |
|--------|------------|----------------|
| Key compromise | Argon2id + ChaCha20-Poly1305 | `storage.rs:189-220` |
| Signature forgery | Ed25519, CLSAG, LION verification | `transaction.rs:1335-1357` |
| Quantum harvest-now-decrypt-later | ML-KEM-768 for stealth addresses | `pq_onetime_keys.rs` |
| Replay attacks | Key images (CLSAG/LION) | `ledger/store.rs:800-845` |

#### 2. Network Attacks

| Threat | Mitigation | Implementation |
|--------|------------|----------------|
| Sybil | Per-IP connection limits (10) | `connection_limiter.rs:21` |
| DoS/Flood | Rate limiting (60 req/min/peer) | `sync.rs:37` |
| Eclipse | Quorum diversity, peer rotation | SCP quorum config |
| MITM | libp2p noise encryption | libp2p defaults |

#### 3. Resource Exhaustion

| Threat | Mitigation | Implementation |
|--------|------------|----------------|
| Memory exhaustion | Message size limits (10MB) | `sync.rs:34` |
| CPU exhaustion | PoW before expensive validation | `validation.rs:210-213` |
| Disk exhaustion | Mempool size limit (10K tx) | `mempool.rs:334-336` |
| Connection exhaustion | Per-IP limits | `connection_limiter.rs` |

#### 4. Privacy Attacks

| Threat | Mitigation | Implementation |
|--------|------------|----------------|
| Transaction graph analysis | Ring signatures (size 20/11) | `clsag.rs`, `lion/` |
| Amount analysis | Pedersen commitments | `commitment.rs` |
| Cluster fingerprinting | Cluster-aware decoy selection | OSPEAD algorithm |
| IP correlation | (User responsibility: Tor/VPN) | Documentation |

#### 5. Authentication/Authorization

| Threat | Mitigation | Implementation |
|--------|------------|----------------|
| API key brute-force | Rate limiting by tier | `rate_limit.rs:22-33` |
| Timestamp replay | ±5 min window validation | `auth.rs:203-218` |
| HMAC forgery | Constant-time comparison | `auth.rs:273-283` |
| Wallet password brute-force | Exponential backoff + lockout | `storage.rs:104-112` |

---

## Threat Matrix

### STRIDE Analysis

| Threat | Category | Asset | Mitigation | Status |
|--------|----------|-------|------------|--------|
| Key extraction | Spoofing | Private keys | Encryption, zeroizing | ✓ |
| Transaction forgery | Tampering | Blockchain | Signature verification | ✓ |
| Transaction censorship | Repudiation | Network | Gossipsub, SCP | ✓ |
| Privacy breach | Information Disclosure | Transaction data | Ring sigs, commitments | ✓ |
| Node DoS | Denial of Service | Network | Rate limiting | ✓ |
| Privilege escalation | Elevation of Privilege | RPC | API key tiers | ✓ |

### Risk Matrix

| Threat | Likelihood | Impact | Risk | Status |
|--------|------------|--------|------|--------|
| Quantum attack on CLSAG | Low (5-15 years) | High | Medium | PQ option available |
| Sybil attack | Medium | Medium | Medium | Mitigated |
| Wallet password brute-force | Medium | Critical | High | Argon2id mitigated |
| RPC DoS | High | Low | Medium | Rate limited |
| Eclipse attack | Low | High | Medium | Partial mitigation |
| Cluster fingerprinting | Medium | Medium | Medium | OSPEAD mitigated |

---

## References

### Internal Documentation
- [Architecture](../architecture.md) - System component overview
- [Security Guide](../security.md) - Operational security practices
- [Privacy Features](../privacy.md) - Cryptographic privacy mechanisms
- [Transactions](../transactions.md) - Transaction types and structure

### External Standards
- [NIST FIPS 203](https://csrc.nist.gov/pubs/fips/203/final) - ML-KEM specification
- [NIST FIPS 204](https://csrc.nist.gov/pubs/fips/204/final) - ML-DSA specification
- [RFC 9180](https://datatracker.ietf.org/doc/rfc9180/) - HPKE specification
- [OWASP Top 10](https://owasp.org/www-project-top-ten/) - Web security risks
- [CryptoNote](https://cryptonote.org/whitepaper.pdf) - Stealth address protocol

### Research Papers
- [CLSAG](https://eprint.iacr.org/2019/654.pdf) - Concise Linkable Ring Signatures
- [Bulletproofs](https://eprint.iacr.org/2017/1066.pdf) - Range proof system
- [LION](https://link.springer.com/chapter/10.1007/978-981-95-3540-8_17) - Lattice-based linkable ring signatures
- [SCP](https://www.stellar.org/papers/stellar-consensus-protocol.pdf) - Stellar Consensus Protocol
