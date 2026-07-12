## Getting Started with Botho

Botho is a privacy-focused cryptocurrency designed for the post-quantum era. It combines **stealth addresses** for transaction privacy with the **Stellar Consensus Protocol (SCP)** for fast, energy-efficient consensus. Unlike proof-of-work cryptocurrencies, Botho achieves finality in seconds while maintaining strong privacy guarantees.

### What Makes Botho Different?

Traditional cryptocurrencies like Bitcoin have transparent blockchains where anyone can trace the flow of funds between addresses. Even "privacy coins" often rely on cryptographic assumptions that may be broken by future quantum computers.

Botho takes a different approach:

- **Stealth addresses** ensure that each payment you receive goes to a unique one-time address, making it impossible to link your transactions together by watching the blockchain
- **Post-quantum cryptography** protects your privacy against adversaries with quantum computers
- **Federated Byzantine Agreement** provides fast finality — consensus safety never depends on hashpower
- **Egalitarian issuance** distributes new coins through RandomX CPU mining that is deliberately decoupled from consensus: mining earns rewards but buys no say over which transactions confirm
- **Progressive economics** — fees scale 1×–6× with wealth concentration, 80% of every fee is redistributed by lottery, and 20% is burned

### Creating a Wallet

Getting started with Botho takes just a few steps:

1. **Visit the Wallet page** - Click "Launch Wallet" from the homepage or navigate directly to the wallet
2. **Choose "Create New Wallet"** - You can also import an existing wallet if you have a recovery phrase
3. **Secure your recovery phrase** - You'll be shown a 24-word mnemonic phrase. Write this down on paper and store it in a safe place. This phrase is the **only way** to recover your funds if you lose access to your device
4. **Optional: Set a password** - Add an encryption password for additional security. You'll need this password each time you open the wallet in this browser

**Important:** Never share your recovery phrase with anyone. Anyone with these words can access your funds. Never store it digitally (no screenshots, no cloud storage, no password managers).

### Understanding Your Wallet Address

Your wallet address looks like this: `botho://1/4nuKn2U5qsRk3vD...` (about 90 characters total)

This address format includes:
- **Protocol identifier** (`botho://` on mainnet, `tbotho://` on testnet) - Different prefixes prevent accidental cross-network sends
- **Address version** (`1/` for classical addresses, `1q/` for quantum-safe addresses)
- **Public keys** - Your view key and spend key, encoded together in base58

Quantum-safe addresses (`1q/`) additionally embed ML-KEM and ML-DSA public keys, which makes them much longer (~4,400 characters) — better suited to QR codes and files than manual copying.

You can safely share this address with anyone who wants to send you funds. Thanks to stealth addresses, each incoming transaction will be sent to a unique derived address that only you can spend from.

### Receiving Your First Payment

When someone sends you BTH:

1. They use your public address to derive a unique one-time address
2. The transaction is broadcast to the network and included in a block
3. Your wallet scans new blocks and detects payments addressed to you
4. The funds appear in your balance, usually within one block — block time adapts to network load, from 3 seconds under very heavy traffic up to 40 seconds when the network is idle

### Sending Payments

To send BTH to someone else:

1. Click the **Send** button in your wallet
2. Enter the recipient's Botho address
3. Enter the amount to send
4. Review the transaction details including the fee
5. Confirm the transaction

Transactions are final once confirmed—there are no chargebacks or reversals in Botho.

### Transaction Fees

Every Botho transaction requires a small fee. These fees serve three purposes:

1. **Spam prevention** - Fees make it expensive to flood the network with junk transactions
2. **Progressive taxation** - Fees scale 1× to 6× based on cluster wealth, discouraging concentration without enabling Sybil attacks
3. **Redistribution and deflation** - 80% of every fee is redistributed to holders through a lottery; the remaining 20% is permanently burned

Fees are size-based, not amount-based: `fee = per-byte rate × transaction size × cluster factor (1×–6×) × output penalty`. See the Cluster Tags and Tokenomics sections for details.

### Security Best Practices

- **Back up your recovery phrase** on paper, stored in a secure location
- **Use a password** to encrypt your wallet in the browser
- **Consider running your own node** for maximum privacy
- **Verify addresses carefully** before sending funds—transactions cannot be reversed
