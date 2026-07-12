## Privacy Features

Privacy is not just a feature in Botho—it's a fundamental design principle. Every aspect of the protocol is designed to protect your financial privacy while maintaining the auditability properties needed for a sound monetary system.

### Why Privacy Matters

Financial privacy is essential for:

- **Personal security** - Public wealth makes you a target for criminals
- **Business confidentiality** - Competitors shouldn't see your supplier payments or revenue
- **Fungibility** - Money should be interchangeable; tainted coins create a two-tier system
- **Human dignity** - Your financial life is nobody's business but your own
- **Censorship resistance** - When all transactions look identical, there's no basis for blocking particular payments. Bitcoin works hard to solve this problem with various techniques, but privacy-by-default solves it elegantly: validators cannot discriminate because they cannot distinguish

### Stealth Addresses

Stealth addresses are the foundation of Botho's privacy model. Here's how they work:

**The Problem:** In Bitcoin, if you publish an address to receive donations, anyone can see every donation you've ever received by looking at that address on the blockchain.

**The Solution:** In Botho, your public address is not where funds are actually sent. Instead, each sender uses your public address to mathematically derive a unique one-time address. Only you can detect and spend from these derived addresses.

**Technical Details:**

1. Your wallet has a **view keypair** and a **spend keypair**
2. The sender generates a random value and combines it with your public keys
3. This produces a one-time address that appears random to everyone else
4. Your wallet uses your private view key to scan for payments addressed to you
5. To spend, you use your private spend key to sign the transaction

The result: Even if you publish your address publicly, no one watching the blockchain can determine how many payments you've received, when you received them, or how much they were for.

### Ring Signatures (Private Transactions)

When you send a **Private transaction**, Botho uses **CLSAG ring signatures** to hide which specific coins you're spending. Your transaction references 20 possible inputs (a "ring"), and the signature proves you own one of them without revealing which one.

CLSAG (Concise Linkable Spontaneous Anonymous Group) is an efficient ring signature scheme that provides strong sender privacy with compact signatures (~700 bytes per input).

This breaks the transaction graph that would otherwise allow tracing funds through the blockchain. An observer sees that *someone* in the ring spent *some* coins, but cannot determine which participant or which specific coins.

> **Note:** All value transfers use ring signatures. Minting transactions (block rewards) use ML-DSA signatures since the minter is public.

### Confidential Amounts

In all **Private transactions** (which includes all value transfers), amounts are hidden using **Pedersen commitments** with **Bulletproofs** range proofs. These cryptographic constructs allow the network to verify that transactions balance (inputs equal outputs plus fees) without revealing the actual amounts.

Validators can confirm:
- No new money is created from thin air
- The sender has sufficient funds
- The fee is at least the minimum required
- All amounts are positive (via Bulletproofs)

But they cannot determine:
- How much is being transferred
- The sender's total balance
- The recipient's total balance

> **Note:** Minting transactions (block rewards) have public amounts for supply auditability, but recipients are still hidden via stealth addresses.

### Post-Quantum Cryptography

Quantum computers pose a future threat to the cryptographic algorithms that secure most cryptocurrencies today. Botho uses a **hybrid post-quantum architecture** that protects the most critical data while keeping transactions efficient.

**Algorithms Used:**

- **ML-KEM-768** (FIPS 203) - Post-quantum stealth addresses (recipient privacy is permanent)
- **ML-DSA-65** (FIPS 204) - Post-quantum signatures for minting transactions
- **CLSAG** - Classical ring signatures for private transactions (sender privacy is ephemeral)
- **Pedersen + Bulletproofs** - Information-theoretic amount hiding (quantum-safe)

**Why This Architecture?**

Recipient identity is recorded on-chain forever—a quantum attacker in 2045 could link recipients from 2025 transactions. ML-KEM protects against this "harvest now, decrypt later" threat. Sender privacy, however, is ephemeral—its value degrades over time as economic context becomes historical. Using classical CLSAG keeps transactions small (~4 KB vs ~65 KB for post-quantum alternatives).

**Transaction Types:**

| Type | Recipient | Amount | Sender | Use Case |
|------|-----------|--------|--------|----------|
| Minting | Hidden (ML-KEM) | Public | Known (ML-DSA) | Block rewards |
| Private | Hidden (ML-KEM) | Hidden | Hidden (CLSAG ring=20) | All transfers (~4 KB) |

Recipients and amounts are protected against quantum computers. Sender privacy uses efficient classical signatures.

### Privacy Best Practices

To maximize your privacy when using Botho:

1. **Run your own node** - This prevents revealing your addresses to third-party servers
2. **Use a new address for each context** - While stealth addresses protect received funds, using separate addresses for work vs personal adds another layer
3. **Be mindful of metadata** - Privacy on-chain doesn't help if you reveal information off-chain
