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
The native currency unit of Botho. 1 BTH = 1,000,000,000 nanoBTH.

### Bulletproofs
A type of zero-knowledge proof used for range proofs. Ensures transaction amounts are positive without revealing the actual values. (Planned for Botho)

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

---

## D

### Decoy
In ring signatures, a decoy is a transaction output included to hide the true sender. Decoys are randomly selected from the blockchain.

### Difficulty
A measure of how hard it is to find a valid proof-of-work. Difficulty adjusts to maintain target block times.

### Dilithium
A lattice-based signature scheme (now standardized as ML-DSA). LION uses similar parameters for consistency.

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
Using both classical and post-quantum algorithms together. Both must verify for security. Provides defense-in-depth against quantum attacks.

---

## K

### Key Image
A cryptographic value derived from a spent output that prevents double-spending without revealing which output was spent. Essential for ring signature systems.

### Kyber
A lattice-based key encapsulation mechanism (now standardized as ML-KEM). Botho uses LION ring signatures instead for unified privacy + PQ security.

---

## L

### Ledger
The complete record of all transactions, stored as a blockchain. Botho uses LMDB for ledger storage.

### LION
**Lattice-based lInkable ring signatures fOr aNonymity** — Botho's post-quantum ring signature scheme. Provides both sender privacy AND quantum resistance in a single unified algorithm. Uses Module-LWE for ~128-bit post-quantum security.

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

### ML-DSA (Dilithium)
**Module Lattice Digital Signature Algorithm** — A post-quantum signature scheme standardized by NIST (FIPS 204). LION uses similar lattice parameters.

### ML-KEM (Kyber)
**Module Lattice Key Encapsulation Mechanism** — A post-quantum key exchange scheme standardized by NIST (FIPS 203). Botho uses LION ring signatures instead for unified privacy + quantum security.

### MLSAG
**Multilayered Linkable Spontaneous Anonymous Group** signature — A ring signature scheme that hides the signer among a group while preventing double-spending.

### Mnemonic
A sequence of words (typically 24) that encodes your wallet's master seed. Used for backup and recovery.

---

## N

### nanoBTH
The smallest unit of BTH. 1 BTH = 1,000,000,000 nanoBTH.

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
Internal unit for BTH amounts. 1 BTH = 1,000,000,000,000 picocredits (10^12).

### Post-Quantum Cryptography
Cryptographic algorithms believed to be secure against quantum computer attacks. Botho uses LION lattice-based ring signatures.

### Private Key
A secret value that controls your funds. Never share your private keys or mnemonic.

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
**Ring Confidential Transactions** — Combines ring signatures with confidential amounts. Planned for Botho.

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
