# Glossary

Technical terms used in Botho documentation.

---

## A

### Address
A public identifier for receiving funds. In Botho, addresses are derived from your view and spend public keys. Each transaction creates a unique stealth address, so your main address is never revealed on-chain.

---

## B

### BIP39
**Bitcoin Improvement Proposal 39** — A standard for generating wallet seed phrases. Botho uses 24-word BIP39 mnemonics for wallet recovery.

### Block
A collection of transactions bundled together and added to the blockchain. Each block references the previous block, forming a chain.

### Block Reward
The amount of BTH created when a new block is mined. Botho starts at 50 BTH per block, halving every ~2 years until reaching tail emission.

### BTH
The native currency unit of Botho. BTH uses a two-tier precision system:
- **For individual transactions**: 1 BTH = 1,000,000,000,000 picocredits (10^12, internal precision)
- **For supply tracking/display**: 1 BTH = 1,000,000,000 nanoBTH (10^9, user-facing)

The conversion factor is: 1 nanoBTH = 1,000 picocredits. See [Unit System](tokenomics.md#unit-system) for details.

### Bulletproofs
A type of zero-knowledge proof used for range proofs. Ensures transaction amounts are positive without revealing the actual values. Used in Private transactions to hide amounts.

### Byzantine Fault Tolerance (BFT)
The ability of a system to continue operating correctly even if some participants are malicious or faulty. SCP provides BFT for Botho consensus.

---

## C

### Cluster
In Botho's progressive fee system, a cluster represents a minting origin. All coins trace back to the block reward that created them. Cluster wealth determines fee rates.

### Cluster Fee
A progressive transaction fee based on coin ancestry. Coins from wealthy clusters (those that haven't circulated) pay higher fees. See [Tokenomics](tokenomics.md#cluster-based-progressive-fees).

### Consensus
The process by which network nodes agree on the current state of the blockchain. Botho uses SCP for consensus.

### CryptoNote
A protocol for private cryptocurrency transactions, originally developed for Bytecoin and used by Monero. Botho implements CryptoNote-style stealth addresses.

### CLSAG
**Concise Linkable Spontaneous Anonymous Group** signature — An efficient ring signature scheme used for Private transactions. CLSAG is 45% smaller than MLSAG through response aggregation. Provides ~128-bit classical security with ring size 20. Sender privacy degrades over time (ephemeral), which is acceptable since economic context becomes historical.

---

## D

### Decoy
In ring signatures, a decoy is a transaction output included to hide the true sender. Decoys are selected using OSPEAD criteria: age distribution, cluster tag similarity, and amount plausibility.

### Difficulty
A measure of how hard it is to find a valid proof-of-work. Difficulty adjusts to maintain target block times.

### Dilithium
A lattice-based signature scheme now standardized as ML-DSA. Botho uses ML-DSA-65 for minting transaction signatures.

---

## E

### ECDH
**Elliptic Curve Diffie-Hellman** — A key exchange protocol for establishing shared secrets. Used in stealth address derivation.

### Emission
The creation of new coins through mining. See [Tokenomics](tokenomics.md#emission-schedule).

---

## F

### Fee Burn
When transaction fees are destroyed rather than paid to miners. Botho burns all fees, creating deflationary pressure.

### Finality
The point at which a transaction cannot be reversed. Botho provides immediate finality via SCP consensus.

### Fork
When the blockchain splits into two competing chains. Botho's SCP consensus prevents forks.

---

## G

### Genesis Block
The first block in a blockchain. Contains no transactions and establishes initial parameters.

### Gossipsub
A pub/sub protocol used by libp2p for peer-to-peer message propagation. Botho uses gossipsub for broadcasting transactions and blocks.

---

## H

### Halving
A scheduled reduction in block rewards, typically by 50%. Botho halves every ~2 years for 10 years, then transitions to tail emission.

### Hash
A fixed-size output produced by a hash function. Used for block identification, proof-of-work, and data integrity.

### Hashrate
The speed at which a miner computes hashes, typically measured in H/s (hashes per second).

### Hybrid Cryptography
Using both classical and post-quantum algorithms together. Botho uses a strategic hybrid approach: ML-KEM-768 (post-quantum) for all stealth addresses (permanent recipient privacy), ML-DSA-65 (post-quantum) for minting signatures, and CLSAG (classical) for ring signatures (ephemeral sender privacy).

---

## K

### Key Image
A cryptographic value derived from a spent output that prevents double-spending without revealing which output was spent. Essential for ring signature systems.

### Kyber
A lattice-based key encapsulation mechanism now standardized as ML-KEM. Botho uses ML-KEM-768 for post-quantum stealth addresses.

---

## L

### Ledger
The complete record of all transactions, stored as a blockchain. Botho uses LMDB for ledger storage.

### LION (Deprecated)
**Lattice-based lInkable ring signatures fOr aNonymity** — A post-quantum ring signature scheme that was considered for Botho but deprecated due to size concerns (~50x larger than CLSAG). See [ADR-0001](decisions/0001-deprecate-lion-ring-signatures.md) for details.

### libp2p
A modular networking stack used by Botho for peer-to-peer communication.

### LMDB
**Lightning Memory-Mapped Database** — A high-performance key-value store used by Botho for blockchain storage.

---

## M

### Mempool
The pool of unconfirmed transactions waiting to be included in a block. Transactions are prioritized by fee.

### Minting
Botho's term for mining — the process of creating new blocks and earning block rewards.

### Minting Transaction
A transaction type that creates new coins as block rewards. Minting transactions have no inputs, use ML-DSA signatures, and create new cluster origins. Amounts are public for supply auditability, but recipients are hidden via stealth addresses.

### ML-DSA (Dilithium)
**Module Lattice Digital Signature Algorithm** — A post-quantum signature scheme standardized by NIST (FIPS 204). Botho uses ML-DSA-65 for Minting transaction authorization.

### ML-KEM (Kyber)
**Module Lattice Key Encapsulation Mechanism** — A post-quantum key exchange scheme standardized by NIST (FIPS 203). Botho uses ML-KEM-768 for post-quantum stealth addresses.

### MLSAG
**Multilayered Linkable Spontaneous Anonymous Group** signature — A classical ring signature scheme. Botho uses CLSAG, a more efficient variant that is 45% smaller.

### Mnemonic
A sequence of words (typically 24) that encodes your wallet's master seed. Used for backup and recovery.

---

## N

### nanoBTH
A display-friendly unit of BTH used for fee calculations and user interfaces. 1 BTH = 1,000,000,000 nanoBTH (10^9). 1 nanoBTH = 1,000 picocredits. NanoBTH is preferred for user-facing amounts because the numbers are more manageable.

### Node
A computer running Botho software that participates in the network. Nodes relay transactions, validate blocks, and optionally mine.

### Nonce
A number that miners vary to find a valid proof-of-work hash.

---

## O

### One-Time Key
A unique key generated for each transaction output. Part of the stealth address system that prevents linking payments to recipients.

### Output
The destination of funds in a transaction. Each output specifies an amount and a one-time public key.

---

## P

### Peer
Another node connected to your node in the P2P network.

### Pedersen Commitment
A cryptographic commitment that hides a value while allowing mathematical operations. Used in confidential transactions.

### Picocredits
The smallest internal unit of BTH, used for transaction amounts and accounting precision. 1 BTH = 1,000,000,000,000 picocredits (10^12). This provides higher precision than nanoBTH for individual transaction calculations. The bridge contracts and core transaction system use picocredits internally, while user interfaces typically display amounts in nanoBTH or BTH for readability.

### Post-Quantum Cryptography
Cryptographic algorithms believed to be secure against quantum computer attacks. Botho uses ML-KEM-768 for all stealth addresses (permanent recipient privacy) and ML-DSA-65 for minting transaction signatures. Ring signatures use classical CLSAG for efficiency—sender privacy is ephemeral and degrades over time.

### Private Key
A secret value that controls your funds. Never share your private keys or mnemonic.

### Private Transaction
The standard transaction type for all value transfers. Uses CLSAG ring signatures (sender hidden among 20 decoys), Pedersen commitments with Bulletproofs (amounts hidden), and ML-KEM stealth addresses (recipient hidden with post-quantum security). Size-based fees (~4 KB typical).

### Proof-of-Work (PoW)
A consensus mechanism where miners prove they've done computational work. Botho uses SHA-256 PoW.

### Public Key
A value derived from your private key that can be shared publicly. Used to receive funds and verify signatures.

---

## Q

### Quantum Computer
A type of computer using quantum mechanics that could break classical cryptography. Botho's hybrid cryptography protects against this threat.

### Quorum
The set of nodes that must agree for SCP consensus to proceed. Configured via `quorum` settings.

### Quorum Slice
In SCP, the subset of nodes that a particular node trusts. The intersection of quorum slices determines consensus.

---

## R

### Range Proof
A zero-knowledge proof that a hidden value falls within a valid range (e.g., is non-negative). Prevents creating coins from nothing.

### Ring Signature
A cryptographic signature that proves one member of a group signed a message, without revealing which one. Used to hide transaction senders.

### RingCT
**Ring Confidential Transactions** — Combines ring signatures with confidential amounts. Botho implements this via CLSAG ring signatures (for sender privacy) combined with Pedersen commitments and Bulletproofs (for amount privacy).

### RPC
**Remote Procedure Call** — A protocol for making requests to a node. Botho provides a JSON-RPC API on port 7101.

---

## S

### SCP
**Stellar Consensus Protocol** — A Byzantine fault-tolerant consensus protocol. Provides fast finality without the possibility of forks.

### Schnorr Signature
A digital signature scheme known for simplicity and efficiency. Botho uses Schnorr signatures (via Ed25519).

### Seed Node
A well-known node used for initial peer discovery. Botho's seed node is `seed.botho.io`.

### SHA-256
**Secure Hash Algorithm 256-bit** — A cryptographic hash function used for Botho's proof-of-work.

### Spend Key
The private key required to spend funds. Part of the view/spend key pair.

### Standard Transaction
*See* **Private Transaction**. All value transfers in Botho use private transactions with CLSAG ring signatures.

### Stealth Address
A privacy technique where each transaction creates a unique one-time address. Prevents linking payments to recipients.

### Subaddress
An additional receiving address derived from your main keys. Used for organization and enhanced privacy.

### Sync
The process of downloading and validating the blockchain from other nodes.

---

## T

### Tag Vector
In cluster fee calculation, a sparse vector tracking what fraction of a UTXO traces back to each minting origin.

### Tail Emission
Perpetual coin creation after the halving period ends. Botho targets ~2% annual tail emission to ensure sustainable mining incentives.

### Transaction
A signed instruction to transfer funds from inputs to outputs.

### Tombstone
A block height after which a transaction expires and becomes invalid. Prevents old transactions from being replayed.

---

## U

### UTXO
**Unspent Transaction Output** — The fundamental unit of value in Botho. Your balance is the sum of all UTXOs you can spend.

---

## V

### View Key
The private key used to scan the blockchain for incoming transactions. Can be shared to allow watch-only access to your wallet.

---

## W

### Wallet
Software that manages your keys and interacts with the blockchain. Botho provides CLI, web, and desktop wallets.

### WebSocket
A protocol for real-time bidirectional communication. Botho's RPC server supports WebSocket connections for event streaming.

---

## Z

### Zero-Knowledge Proof
A cryptographic technique to prove something is true without revealing the underlying information. Used in range proofs and confidential transactions.
