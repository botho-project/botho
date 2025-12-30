# Why Botho?

A detailed comparison of Botho with other cryptocurrencies.

## Quick Comparison

| Feature | Botho | Bitcoin | Monero | Zcash |
|---------|-------|---------|--------|-------|
| Privacy | Default | None | Default | Opt-in |
| Finality | ~5 sec | ~60 min | ~20 min | ~75 min |
| Quantum-safe | Yes (hybrid) | No | No | No |
| Anti-hoarding | Yes (cluster fees) | No | No | No |
| Tail emission | Yes (2%) | No | Yes (0.6/block) | No |
| Fee destination | Burned | To miners | To miners | To miners |
| Pre-mine | None | None | None | 10% founder |

---

## Botho vs Bitcoin

### What Bitcoin Does Well

Bitcoin pioneered decentralized digital money and remains the most secure and widely recognized cryptocurrency. Its simplicity and conservative approach have made it extremely robust.

### Where Botho Differs

**Privacy**

Bitcoin transactions are fully transparent. Anyone can see:
- Sender address
- Recipient address
- Amount transferred
- Full transaction history

Botho transactions reveal none of this. Every transaction uses stealth addresses (hiding recipients) and ring signatures (hiding senders).

**Finality**

Bitcoin uses probabilistic finality — you wait for confirmations because blocks can be reorganized. The standard "6 confirmations" takes about 60 minutes.

Botho uses SCP consensus, providing deterministic finality in seconds. Once a transaction is confirmed, it cannot be reversed.

**Economics**

Bitcoin's security budget will eventually depend entirely on transaction fees (after ~2140). If fees are insufficient, security degrades.

Botho has perpetual tail emission (~2% annually), ensuring miners are always incentivized regardless of transaction volume.

**Wealth Distribution**

Bitcoin rewards early adopters and large holders with no mechanism to encourage circulation.

Botho's progressive cluster fees make hoarding expensive, encouraging coins to circulate through the economy.

---

## Botho vs Monero

### What Monero Does Well

Monero is the gold standard for cryptocurrency privacy. Its mandatory privacy, mature ring signature implementation, and strong community have made it the most trusted privacy coin.

### Where Botho Differs

**Consensus & Finality**

Monero uses traditional Nakamoto consensus with RandomX proof-of-work. Transactions need ~10 confirmations (~20 minutes) for reasonable security.

Botho's SCP hybrid provides final consensus in seconds. There are no reorgs and no need to wait for confirmations.

**Fee Economics**

Monero fees go to miners, creating a typical fee market.

Botho fees are burned, creating deflationary pressure that offsets tail emission. This simplifies monetary policy and prevents fee-based miner centralization.

**Wealth Distribution**

Monero has no mechanism to discourage wealth concentration.

Botho's cluster-based progressive fees make hoarding expensive without compromising privacy or enabling Sybil attacks.

**Quantum Resistance**

Monero uses classical cryptography vulnerable to future quantum computers.

Botho uses LION lattice-based ring signatures, providing both privacy AND quantum resistance in a single unified primitive, protecting against "harvest now, decrypt later" attacks.

**Block Selection**

In Monero, the first miner to propagate a valid block wins, favoring miners with better network connectivity.

In Botho, SCP consensus determines the winner, making network latency irrelevant for block selection.

---

## Botho vs Zcash

### What Zcash Does Well

Zcash pioneered zero-knowledge proofs for cryptocurrency privacy. Its "shielded" transactions provide strong cryptographic privacy guarantees.

### Where Botho Differs

**Privacy Model**

Zcash privacy is opt-in. Most transactions are transparent, and even shielded transactions reveal the shielded/transparent boundary.

Botho privacy is mandatory and uniform. All transactions look the same, providing a larger anonymity set.

**Finality**

Zcash uses Nakamoto consensus with ~75 minute finality (assuming 24 confirmations for shielded transactions).

Botho provides seconds-long finality via SCP.

**Trusted Setup**

Zcash's original privacy (Sprout) required a trusted setup ceremony. Later upgrades (Sapling, Orchard) reduced but didn't eliminate trust requirements.

Botho uses CryptoNote-style privacy with no trusted setup required.

**Economics**

Zcash had a 20% founder's reward (now Zcash Community Grants) and will transition to a fee-only model after 2024.

Botho has no pre-mine, no founder reward, and perpetual tail emission for sustainable security.

---

## Botho vs Other Privacy Coins

### vs Dash

Dash's "PrivateSend" is optional CoinJoin — weaker privacy than cryptographic solutions. Botho provides mandatory cryptographic privacy.

### vs Grin/Beam (MimbleWimble)

MimbleWimble provides good privacy but requires interactive transaction building. Botho transactions are non-interactive like traditional cryptocurrencies.

### vs Secret Network

Secret Network focuses on programmable privacy (smart contracts). Botho focuses on being excellent at one thing: private, fair money.

---

## When to Choose Botho

**Choose Botho if you value:**

- **Fast finality** — Transactions confirm in seconds, not minutes
- **Mandatory privacy** — No "forgot to enable privacy" mistakes
- **Quantum resistance** — Protection against future quantum computers
- **Fair economics** — Progressive fees discourage hoarding
- **Sustainable security** — Perpetual mining incentives

**Choose something else if you need:**

- **Maximum decentralization** — Bitcoin has the largest, most distributed network
- **Battle-tested privacy** — Monero has years of real-world privacy validation
- **Smart contracts** — Ethereum, Secret Network, or other programmable chains
- **Maximum liquidity** — Bitcoin and Ethereum have the deepest markets

---

## Technical Comparison

### Cryptography

| Component | Botho | Bitcoin | Monero | Zcash |
|-----------|-------|---------|--------|-------|
| Signatures | Schnorr / LION | ECDSA/Schnorr | CLSAG | RedDSA |
| Key exchange | ECDH | N/A | ECDH | DH |
| Stealth addresses | Yes | No | Yes | Shielded only |
| Ring signatures | Yes (MLSAG/LION) | No | Yes (CLSAG) | No |
| Zero-knowledge | Planned | No | Bulletproofs | Halo2 |
| Quantum-safe | Yes (LION) | No | No | No |

### Consensus

| Aspect | Botho | Bitcoin | Monero | Zcash |
|--------|-------|---------|--------|-------|
| Mechanism | PoW + SCP | PoW (SHA-256) | PoW (RandomX) | PoW (Equihash) |
| Block time | 20 sec | 600 sec | 120 sec | 75 sec |
| Finality | Immediate | Probabilistic | Probabilistic | Probabilistic |
| Fault tolerance | Byzantine | Crash | Crash | Crash |

### Economics

| Aspect | Botho | Bitcoin | Monero | Zcash |
|--------|-------|---------|--------|-------|
| Max supply | None (tail emission) | 21M | None (tail emission) | 21M |
| Pre-mine | None | None | None | 10% |
| Fee model | Burned + progressive | To miners | To miners | To miners |
| Tail emission | ~2% target | None | 0.6 XMR/block | None |

---

## Philosophy Comparison

### Bitcoin: Digital Gold

Bitcoin optimizes for scarcity and security. It's designed to be a store of value with predictable, capped supply.

### Monero: Digital Cash

Monero optimizes for privacy and fungibility. Every coin is identical and untraceable, like physical cash.

### Zcash: Compliant Privacy

Zcash optimizes for optional privacy within regulatory frameworks. Users choose their privacy level.

### Botho: Fair Money

Botho optimizes for privacy AND economic fairness. It's designed to serve communities rather than concentrate wealth.

The name "Botho" means "humanity" — reflecting the belief that money should connect people, not divide them.

---

## Migration Paths

### From Bitcoin

If you're coming from Bitcoin:
- Privacy is automatic — no extra steps needed
- Transactions confirm much faster
- The wallet model is similar (addresses, transactions)
- No UTXO management needed (handled automatically)

### From Monero

If you're coming from Monero:
- Privacy features will feel familiar
- Finality is immediate — no waiting for confirmations
- Ring signatures work similarly
- Wallet recovery uses standard BIP39 mnemonics

### From Zcash

If you're coming from Zcash:
- No need to choose transparent vs shielded
- All transactions are private by default
- No trusted setup concerns
- Simpler mental model
