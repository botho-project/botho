# Threat Model

This document describes Botho's threat model for external security auditors. It covers protected assets, adversary profiles, trust assumptions, attack surfaces, security properties, and mitigations.

## Table of Contents

- [Changes Since Last Revision](#changes-since-last-revision)
- [Assets](#assets)
- [Adversaries](#adversaries)
- [Trust Assumptions](#trust-assumptions)
- [Attack Surface](#attack-surface)
- [Security Properties](#security-properties)
- [Economic Mechanism Threats](#economic-mechanism-threats)
- [Mitigations](#mitigations)
- [Threat Matrix](#threat-matrix)

---

## Changes Since Last Revision

**Revision date: 2026-07-04.** This refresh brings the threat model up to
post-cycle-6 reality. The prior revision predated (a) the cluster-tilted
economic redistribution mechanism (tilted lottery + emission routing +
spend-time demurrage) and (b) the cycle-6 consensus block-acceptance
hardening. Every claim about current behavior below has been verified against
the merged code or the referenced PR; open residuals are called out explicitly
with their tracking issue.

**New / expanded threat coverage:**

- **Consensus block-acceptance path** — a new attack-surface section. Cycle 6
  ([audit](../../audits/2026-06-11-cycle6.md)) found that blocks arriving via
  gossip/sync bypassed most economic consensus rules (five Criticals: C1–C5).
  The `Ledger::add_block` path now enforces expected difficulty (C1),
  recomputed emission reward + timestamp bounds (C2), ring-member UTXO
  resolution (C3), `tx_root` recomputation (C4), a single integer difficulty
  controller (C5, #553), a cluster-tag inflation guard (C6, #576/#580), and a
  deterministic consensus fee floor with spend-time demurrage (C7,
  #578/#602). Fee-sum overflow now rejects (#601); consensus double-spend and
  wealth reads fail closed on DB error (#600/#564); block + emission state
  commit in one LMDB txn (#560); rebuild vs incremental cluster-wealth
  arithmetic is now parity-matched (M3, #604/#607).
- **Economic mechanism threats** — a new section: gaming the tilted lottery,
  demurrage parking, cluster-tag inflation via spender-chosen decoys, and
  fee-market manipulation.
- **Bootstrap / seed-discovery attack surface** — DNS TXT seed discovery and
  the multi-region testnet seed topology (#613).
- **Wallet ring-age fingerprinting** — the OSPEAD age-similar decoy policy
  (#596) and the degenerate-band DoS it introduced and then fixed
  (#611 → #618); the CLI thin wallet now builds real CLSAG rings (#620).

**Documented open residuals (NOT mitigated — tracked):**

- **#581** — cluster-tag inflation via decoy-sourced input mass. The C6 guard
  bounds output tag-mass by the mass of the *resolved ring members*, which
  include spender-chosen decoys, so a spender who selects high-cluster-mass
  decoys can inflate the permitted output mass above what the real inputs
  supply. Accepted residual (never false-rejects a valid block; tightening
  needs a decoy-independent input set).
- **Lottery / demurrage payout privacy** — lottery winners are visible
  on-chain and cluster-tilted selection statistically leaks winner factor
  distribution (design doc Open Question 4). Open, unquantified.
- **"Everyone parks" drift** — spend-time demurrage charges accrual at spend,
  so a permanently parked coin pays nothing until it moves (nominal share
  drifts ~+0.5pp/5yr, offset by unbooked accrued liability). The designed
  countermeasure (eligibility decay on the payout weight) is not implemented;
  see #314 / design Open Question 0.

---

## Assets

The following assets require protection in the Botho system:

### 1. User Private Keys

| Key Type | Purpose | Storage Location | Compromise Impact |
|----------|---------|------------------|-------------------|
| Spend Private Key | Authorizes fund transfers | Encrypted wallet file | Complete fund loss |
| View Private Key | Scans incoming transactions | Encrypted wallet file | Privacy loss (incoming) |
| ML-DSA Private Key | Minting transaction signing | Encrypted wallet file | Minting capability loss |
| Mnemonic (24 words) | Master seed for all keys | User responsibility | Complete fund loss |

**File references:**
- Key derivation: `botho/src/wallet.rs:42-76`
- Encrypted storage: `botho-wallet/src/storage.rs:164-383`

### 2. Transaction Privacy

| Privacy Goal | Technique | Protected Data |
|--------------|-----------|----------------|
| Recipient hiding | ML-KEM-768 stealth addresses | Recipient identity |
| Sender hiding | CLSAG ring signatures (ring=20) | Sender identity |
| Amount hiding | Pedersen commitments + Bulletproofs | Transaction values |
| Memo privacy | AES-256-CTR encryption | Payment metadata |

**File references:**
- Stealth addresses: `crypto/ring-signature/src/pq_onetime_keys.rs`
- Ring signatures: `crypto/ring-signature/src/ring_signature/clsag.rs`
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

### 5. Economic Redistribution Integrity

The cluster-tilted redistribution mechanism introduces consensus-critical
economic state whose integrity is now an asset in its own right. See the
[design doc](../design/cluster-tilted-redistribution.md) and
[cycle-6 audit](../../audits/2026-06-11-cycle6.md).

| Component | Threat | Protection |
|-----------|--------|------------|
| Emission reward / total supply | Inflation via forged reward | `add_block` recomputes `calculate_block_reward` and rejects mismatch (C2) |
| Lottery pool + carryover | Payout suppression / seed-grinding | Validator re-derives the deterministic draw; per-block payout cap ≤ 1 reward with carryover; seed-rotated candidate window (#573) |
| Per-cluster wealth state (`cluster_wealth_db`) | Fee-rate evasion by splitting | Fees key off *global* cluster wealth (split-invariant); rebuild/incremental parity via `saturating_add` (M3, #607) |
| Demurrage accrual | Evasion of the stock-level levy | Consensus fee floor recomputes `base_minimum_fee + demurrage_charge` at block validation (C7, #602) |
| Cluster-tag mass | Tag inflation to lower factor | Conservation-of-mass guard at block validation (C6, #580); residual #581 |

**File references:**
- Block-acceptance gates: `botho/src/ledger/store.rs` (`add_block_inner`, C1–C7)
- Consensus fee floor + demurrage: `botho/src/ledger/store.rs` (`consensus_fee_floor`, `verify_consensus_fee_floor`)
- Lottery draw: `botho/src/consensus/lottery.rs`, `cluster-tax/src/lottery.rs`
- Demurrage math: `cluster-tax/src/demurrage.rs`, `bth_cluster_tax::ring_elapsed_quantile`

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
- Break ML-DSA minting signatures
- Reverse hash functions

**Note:** CLSAG ring signatures are classical and vulnerable to quantum attacks. However, sender privacy is ephemeral—its value degrades over time as economic context becomes historical. See [ADR-0001](../decisions/0001-deprecate-lion-ring-signatures.md) for the rationale.

**Mitigation level:** High (PQ cryptography for critical paths)

### 7. Economic / Strategic Adversary

**Capability:** A well-capitalized holder (a "whale") who transacts within the
protocol's rules to defeat the redistribution mechanism. Does not need to break
cryptography or control consensus — only to shape its own transactions and
holdings.

**Goals:**
- Capture the cluster-tilted lottery payout stream
- Avoid the progressive cluster-factor fee (evade attribution)
- Avoid spend-time demurrage (park wealth, or dilute the ring age clock)
- Inflate output cluster-tag mass beyond what real inputs supply

**Cannot:**
- Gain lottery weight by splitting UTXOs (weight is value-weighted, Sybil/split-invariant by construction)
- Lower its fee rate by splitting funds across accounts (fees key off *global* cluster wealth)
- Dilute the consensus demurrage clock with fresh decoys (the floor uses `ring_elapsed_quantile@max`, an unweighted order statistic — B1)
- Escape a wealthy ring's demurrage with background-tagged outputs (factor floored at the ring-centroid-implied value — B2)

**Residual levers (open):**
- Shedding cluster attribution via wash trading (rate-bounded by tag decay, but the single remaining designed lever)
- Decoy-sourced tag-mass inflation against the C6 guard (**#581**, accepted residual)
- Permanent parking to defer demurrage (**#314** / "everyone parks" watch item)
- Payout-privacy leakage from on-chain lottery winners (design Open Question 4)

**Mitigation level:** Medium — the core split/Sybil vectors are closed by
construction and validated by simulation ([design doc](../design/cluster-tilted-redistribution.md));
the residuals above are explicitly tracked, not mitigated.

| Component | Algorithm | Quantum Safety |
|-----------|-----------|----------------|
| Stealth addresses | ML-KEM-768 | Full |
| Minting signatures | ML-DSA-65 | Full |
| Private sender | CLSAG | Classical (ephemeral privacy) |
| Commitments (hiding) | Pedersen | Information-theoretic |
| Commitments (binding) | Pedersen | Classical only |

---

## Trust Assumptions

### Cryptographic Hardness

| Assumption | Algorithm | Implication if Broken |
|------------|-----------|----------------------|
| Discrete Log (DLOG) | curve25519 | CLSAG signatures forgeable |
| Decisional Diffie-Hellman (DDH) | curve25519 | Pedersen binding broken |
| Learning With Errors (LWE) | ML-KEM, ML-DSA | PQ protections broken |
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

### 5. Consensus Block-Acceptance Path

**Path:** `Ledger::add_block` / `add_block_inner` — the code every node runs on
a block received via gossip or sync. Cycle 6 found this path enforced only
height, prev-hash, self-declared PoW, lottery accounting, key images, and ring
signatures; the following economic gates were subsequently added (all verified
in `botho/src/ledger/store.rs`).

| Gate | Rule enforced at block acceptance | Reference | Status |
|------|-----------------------------------|-----------|--------|
| C1: Difficulty | `header.difficulty == chain-expected difficulty`, then `is_valid_pow` | `store.rs` C1 (`#553` controller) | Mitigated |
| C2: Reward | `minting_tx.reward == calculate_block_reward(...)`; reject mismatch | `store.rs` (expected_reward) | Mitigated |
| C2: Timestamp | minting/header timestamps agree; `>= parent`; `<= now + MAX_FUTURE_TIMESTAMP_SECS` | `store.rs` | Mitigated |
| C3: Ring members | Every ring member resolved against the UTXO set (`verify_ring_members`) | `store.rs` | Mitigated |
| C4: tx_root | `header.tx_root == Block::compute_tx_root(&transactions)` | `store.rs` | Mitigated |
| C5: Difficulty controller | Single integer (u128 bps) controller; f64 eliminated from live path | `#553` | Mitigated |
| C6: Tag inheritance | Output cluster-tag mass ≤ resolved-ring-member input mass (conservation) | `check_cluster_tag_inheritance` (#576/#580) | Mitigated (residual #581) |
| C7: Fee floor | `tx.fee >= base_minimum_fee + demurrage_charge` (congestion-free, integer) | `consensus_fee_floor` (#578/#602) | Mitigated |
| Fee-sum overflow | `checked_block_fees` → `LedgerError::FeeOverflow` instead of wrapping | `#601` | Mitigated |
| Emission-state atomicity | Block + difficulty/reward/epoch state commit in one LMDB txn | `#560` | Mitigated |
| DB-error handling | `is_key_image_spent` / `get_cluster_wealth` fail **closed** on error | `#600` / `#564` | Mitigated |

**Consensus determinism discipline (why these are fork-safe):** the fee floor
uses a fixed `CONSENSUS_FEE_BASE` (not the mempool's f64 congestion EMA), the
demurrage clock uses an unweighted order statistic (`ring_elapsed_quantile@max`)
over public ring-member `(value, created_at)`, cluster wealth is read from
committed pre-block state, and all accumulation is integer + `BTreeMap`. The
mempool's dynamic congestion base can only raise a node's local admission
threshold *above* this floor, never below (Bitcoin min-relay vs.
consensus-validity split).

**Open items from cycle 6 (not consensus-exploitable today, tracked):**
- Cumulative cluster wealth never decrements (M2) — an economics/design
  question, deterministic.
- Fee accounting books 100% of fees as burned while 80% is redistributed (M4)
  — no consensus effect today (difficulty ignores burns).
- No explicit fork-choice/reorg handling in `add_block` beyond exact
  next-height; interaction with SCP finalization needs documented verification
  (I6).

### 6. Bootstrap / Seed-Discovery Attack Surface

**Components:** DNS TXT seed discovery (`botho/src/network/dns_seeds.rs`),
hardcoded fallback seed list (`botho/src/network/seeds.rs`), protocol-version
peer gating (`botho/src/network/discovery.rs`).

**Bootstrap order:** (1) explicit `bootstrap_peers` in `config.toml`, (2) DNS
TXT discovery (`seeds.botho.io` / `seeds.testnet.botho.io`), (3) hardcoded
fallback list. The testnet fallback list ships the live regional seeds by
default (#613): a primary (`seed.botho.io`) plus regional hosts
`us`/`eu`/`ap.seed.botho.io`. Mainnet regional hostnames are scaffolding, gated
behind `BOTHO_REGIONAL_SEEDS`.

| Attack | Mitigation | Status |
|--------|------------|--------|
| DNS poisoning / spoofed TXT seeds | Falls back to hardcoded seeds on DNS failure; block validation rejects any peer's invalid chain regardless of how it was discovered | Partial — DNS records are **not** cryptographically pinned (no DNSSEC dependency); a poisoned resolver can bias peer selection / attempt eclipse |
| Eclipse via seed-list control | Multi-region seed diversity (#613); per-IP connection caps | Partial |
| Malicious primary seed | Mainnet primary seed multiaddr pins a peer ID; testnet primary resolves peer ID dynamically (host re-key without client release) | Partial (testnet unpinned by design) |
| Protocol downgrade / incompatible peers | Major-version mismatch disconnects (`PROTOCOL_VERSION` 3.0.0, `consensus_incompatibility`, #608); minor/patch differences warn only | Mitigated (upgrade hygiene, not a security boundary — see I1) |
| Redial storm after version disconnect | Per-IP caps bound cost; no ban/backoff window yet (L1) | Partial |

**Note:** protocol versions are self-reported via libp2p identify. The
disconnect is upgrade hygiene, not a trust boundary — a lying peer is still
excluded only by block validation (cycle-6 I1).

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
- Ring signatures (CLSAG ring=20)
- Cluster-aware decoy selection (≥70% cosine similarity)
- Age-similar decoy selection: decoys drawn from ±10% of the real input's age
  (`AGE_SIMILARITY_SPREAD_BPS`), floored at `MIN_DECOY_AGE_BLOCKS`
  confirmations, CSPRNG-shuffled (not a deterministic first-N height slice).
  The node and the CLI thin wallet share this policy
  (`botho/src/decoy_selection.rs`, `botho-wallet/src/decoy_selection.rs`).

**Limitations:**
- Network-level correlation possible without Tor
- Cluster tag fingerprinting reduces effective ring size
- Exchange KYC can link identities
- **Ring-age fingerprinting (mitigated, residual):** an earlier wallet decoy
  policy used a wide (~2×) age band; a chain-analysis adversary could
  fingerprint wallet-built rings by their unusually broad ring-age spread. The
  ±10% age-similarity band (#596) collapses that distinction and is `#314`-safe
  under `ring_elapsed_quantile@max`. The band is degenerate for very young
  inputs (below the confirmation floor `min_age > max_age`); spending such
  inputs is guarded with a clean error rather than a panic (fixed #611 → #618).
  Age-similarity is a heuristic-resistance measure, not a proof of
  indistinguishability.

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

### 6. Economic Redistribution Integrity

**Property:** The supply schedule, lottery payout, and progressive
fee/demurrage terms are computed identically by the block proposer and every
validator, from committed chain state — so they cannot be forged, evaded by
UTXO structure, or made to fork the chain.

**Informal statement:**
```
For a block B applied at height H by any honest node:
- reward(B) == calculate_block_reward(H, total_mined)   (no inflation)
- for every transfer tx T in B: fee(T) >= base_minimum_fee(T)
                                          + demurrage_charge(T, H)   (no evasion)
- lottery winners == deterministic draw over the seed-rotated candidate window
- output cluster-tag mass <= resolved-ring-member input mass          (C6)
```

**Achieved via:**
- Emission reward recomputation and timestamp bounds at acceptance (C2)
- Deterministic integer consensus fee floor with `ring_elapsed_quantile@max`
  demurrage clock and ring-centroid-floored factor (C7, #602)
- Value-weighted, split-invariant lottery weight; per-block payout cap with
  carryover; validator re-derivation of the draw
- Global (not per-UTXO) cluster wealth as the fee-rate signal

**Limitations (open residuals):**
- Decoy-sourced tag-mass inflation vs. the C6 guard (**#581**)
- Spend-time (not continuous) demurrage → "everyone parks" drift (**#314**)
- Lottery payout privacy: winners visible on-chain, factor distribution leaks
- Cluster wealth is cumulative and never decrements (M2)

---

## Economic Mechanism Threats

The cluster-tilted redistribution mechanism is designed so that the primary
gaming vectors are closed *by construction* (value-weighting, global cluster
wealth) rather than by policy — see the
[design doc](../design/cluster-tilted-redistribution.md), empirically validated
by 5-year honest-and-gamed simulation. This section enumerates the specific
threats and their current status against merged code.

### 1. Gaming the tilted lottery (payout capture)

**Threat:** A whale splits/churns UTXOs to capture the redistribution stream
(the failure mode of uniform/Hybrid-α payouts, which *inverted* Gini under
gaming — whale 5%→24% in simulation).

**Status:** Mitigated by construction. Lottery weight is
`value × (max_factor − cluster_factor + 1) / max_factor` — value-weighted, so
splitting a position never increases total weight. The consensus lottery
defaults to `ClusterWeighted` (not the grindable `Hybrid { alpha }`). Because
churn/split fees feed the pool, attacking the mechanism *funds* it (gamed
simulation redistributes marginally more than honest). Payout cap ≤ 1 reward
with carryover, plus a seed-rotated candidate window (#573), bounds
seed-grinding gain below the PoW cost of a regrind.

**Residual:** payout privacy — on-chain winners statistically reveal the winner
factor distribution (design Open Question 4). Open.

### 2. Demurrage evasion

**Threat:** Avoid the spend-time demurrage levy, the load-bearing stock-level
term of the validated Gini mechanism, by (a) selecting fresh high-value decoys
to dilute the ring age clock, (b) tagging outputs as background to zero the
factor, or (c) never spending.

**Status:**
- (a) **Mitigated:** the consensus floor's demurrage clock is
  `ring_elapsed_quantile@max` — an *unweighted* order statistic over ring-member
  ages, value-independent, so fresh high-value decoys cannot pull it to zero
  (H2/B1).
- (b) **Mitigated:** the factor is floored at the ring-centroid-implied factor
  (`ring_centroid_floored_factor`, B2), so background-tagged outputs cannot
  escape a wealthy ring's demurrage.
- (c) **Open residual (#314):** demurrage accrues at *spend*, not continuously,
  so a permanently parked coin pays nothing until it moves. Simulation bounds
  the "everyone parks" drift to ~+0.5pp/5yr (offset by unbooked accrued
  liability); the designed countermeasure (eligibility decay on payout weight)
  is not yet implemented.

### 3. Cluster-tag inflation via decoys (#581)

**Threat:** Inflate output cluster-tag mass beyond what the real inputs supply,
lowering effective factors / diluting the wealth signal.

**Status:** Partially mitigated. The C6 conservation-of-mass guard (#576/#580)
rejects blocks where output tag-mass exceeds input tag-mass per cluster. But
the "input" mass is summed over the *resolved ring members*, which include
spender-chosen decoys — so a spender who picks high-cluster-mass decoys can
raise the permitted output mass above what the real inputs actually supply.
This is an **accepted residual tracked in #581**: the current bound never
false-rejects a valid block (ring mass is an upper bound on true input mass),
and tightening it requires a decoy-independent input set. The wallet-side
cluster-factor ceiling on decoys (`decoy_factor <= real_factor × ratio`)
limits, but does not eliminate, the lever for honest wallets.

### 4. Fee-market manipulation

**Threat:** A miner includes their own transactions fee-free, or a wealthy
cluster self-mines / pays miners out-of-band, to evade the progressive fee and
demurrage (cycle-6 H1: these were mempool-only policy).

**Status:** Mitigated. The consensus fee floor (C7, #602) recomputes
`base_minimum_fee + demurrage_charge` per transfer tx at block validation from
committed chain state, so under-fee transactions are rejected by every
validator regardless of who mined them. The floor is congestion-free
(deterministic), so it cannot be gamed via node-local congestion state, and the
mempool's dynamic base can only make a node *stricter*.

---

## Mitigations

### By Threat Category

#### 1. Cryptographic Attacks

| Threat | Mitigation | Implementation |
|--------|------------|----------------|
| Key compromise | Argon2id + ChaCha20-Poly1305 | `storage.rs:189-220` |
| Signature forgery | Ed25519, CLSAG, ML-DSA verification | `transaction.rs:1335-1357` |
| Quantum harvest-now-decrypt-later | ML-KEM-768 for stealth addresses | `pq_onetime_keys.rs` |
| Replay attacks | Key images (CLSAG) | `ledger/store.rs:800-845` |

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

#### 6. Consensus / Economic Integrity (cycle 6)

| Threat | Mitigation | Implementation |
|--------|------------|----------------|
| Self-declared low difficulty | Expected-difficulty gate (C1) | `store.rs` add_block |
| Reward inflation | Recompute `calculate_block_reward` (C2) | `store.rs` add_block |
| Ring counterfeit inputs | UTXO-set ring resolution (C3) | `verify_ring_members` |
| tx-list substitution | `tx_root` recomputation (C4) | `store.rs` add_block |
| Platform-divergent difficulty | Single integer controller (C5) | `#553` |
| Cluster-tag inflation | Conservation-of-mass guard (C6) | `check_cluster_tag_inheritance` (#580) |
| Fee/demurrage evasion | Deterministic consensus fee floor (C7) | `consensus_fee_floor` (#602) |
| Fee-sum overflow | `checked_block_fees` → typed error | `#601` |
| Consensus DB-error fail-open | Fail closed on read error | `#600` / `#564` |
| Non-atomic emission state | Single-txn commit | `#560` |
| Rebuild/incremental drift | `saturating_add` parity | `#607` |

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
| Inflation via forged reward | Tampering | Supply | `add_block` reward recomputation (C2) | ✓ |
| Block-list substitution | Tampering | Blockchain | `tx_root` recomputation (C4) | ✓ |
| Zero-PoW chain injection | Spoofing | Consensus | Expected-difficulty gate (C1) | ✓ |
| Fee/demurrage evasion | Repudiation | Economics | Consensus fee floor (C7) | ✓ |
| Tag-mass inflation | Tampering | Economics | C6 guard; residual #581 | ◐ |
| DNS seed poisoning | Spoofing | Bootstrap | Hardcoded fallback; block validation | ◐ |

### Risk Matrix

| Threat | Likelihood | Impact | Risk | Status |
|--------|------------|--------|------|--------|
| Quantum attack on CLSAG | Low (5-15 years) | High | Medium | PQ option available |
| Sybil attack | Medium | Medium | Medium | Mitigated |
| Wallet password brute-force | Medium | Critical | High | Argon2id mitigated |
| RPC DoS | High | Low | Medium | Rate limited |
| Eclipse attack | Low | High | Medium | Partial mitigation |
| Cluster fingerprinting | Medium | Medium | Medium | OSPEAD mitigated |
| Inflation via forged block reward | Low | Critical | High | Mitigated (C2, #602) |
| Block-acceptance economic bypass (C1–C7) | Low | Critical | High | Mitigated (cycle 6) |
| Lottery payout capture (split/churn) | Medium | Medium | Medium | Mitigated by construction |
| Demurrage parking ("everyone parks") | Low | Medium | Low-Medium | Open residual (#314) |
| Tag-mass inflation via decoys | Medium | Medium | Medium | Partial (residual #581) |
| Fee-market manipulation (self-mining) | Medium | Medium | Medium | Mitigated (C7) |
| DNS seed poisoning / eclipse | Low | High | Medium | Partial (no seed pinning) |
| Ring-age fingerprinting | Medium | Medium | Medium | Mitigated (#596) |
| Lottery/demurrage payout privacy | Medium | Low | Low-Medium | Open (unquantified) |

---

## References

### Internal Documentation
- [Architecture](../concepts/architecture.md) - System component overview
- [Security Guide](../concepts/security.md) - Operational security practices
- [Privacy Features](../concepts/privacy.md) - Cryptographic privacy mechanisms
- [Transactions](../concepts/transactions.md) - Transaction types and structure
- [Cluster-Tilted Redistribution](../design/cluster-tilted-redistribution.md) - Economic mechanism design (validated)
- [Cycle-6 Internal Audit](../../audits/2026-06-11-cycle6.md) - Consensus-economics findings (C1–C7, H/M/L)

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
