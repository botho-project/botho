// ============================================================================
// Core Types for Botho Wallet
// ============================================================================

/** Cryptocurrency amounts in the smallest unit (like satoshis) */
export type Amount = bigint

/** A blockchain address */
export type Address = string

/** A transaction hash */
export type TxHash = string

/** A block hash */
export type BlockHash = string

/** Block height */
export type BlockHeight = number

/** Unix timestamp in seconds */
export type Timestamp = number

// ============================================================================
// Transaction Types
// ============================================================================

export type TransactionType = 'send' | 'receive' | 'minting'
export type TransactionStatus = 'pending' | 'confirmed' | 'failed'
export type PrivacyLevel = 'standard' | 'private' // standard = minting (PoW-bound attribution, no signature), private = CLSAG ring signatures

/** Cryptographic attribution type used in transaction (ADR 0006: minting is PoW-bound, signature-free) */
export type CryptoType = 'clsag' | 'minting' | 'hybrid'

export interface Transaction {
  id: TxHash
  /**
   * Direction/kind of the transaction. Present for wallet-owned history rows
   * (the wallet knows whether it sent or received). Omitted by the block
   * explorer's `getTransaction`, where the node exposes no per-tx direction —
   * consumers must not assert one (#913).
   */
  type?: TransactionType
  /**
   * Transaction amount in picocredits. Present only for wallet-owned history
   * rows, where the amount is recovered from the wallet's own view key. The
   * node never exposes per-tx amounts to third parties, and under confidential
   * transactions (ADR 0006 Decision 1) it never will — so the block explorer
   * leaves this undefined rather than fabricating a `0` (#913, deprecation D1
   * in docs/design/post-ct-analytics.md).
   */
  amount?: Amount
  fee: Amount
  privacyLevel: PrivacyLevel
  /** Attribution type: clsag (ring signatures for private tx), minting (PoW-bound, no signature), or hybrid */
  cryptoType: CryptoType
  status: TransactionStatus
  /**
   * Wall-clock time of the transaction (unix seconds). Present for wallet
   * history and blocks whose timestamp is known. Omitted by the block
   * explorer's `getTransaction` when the RPC does not expose a real timestamp —
   * consumers render "—" rather than fabricating `Date.now()` (#913).
   */
  timestamp?: Timestamp
  blockHeight?: BlockHeight
  confirmations: number
  counterparty?: Address
  memo?: string
}

// ============================================================================
// Wallet Types
// ============================================================================

export interface Balance {
  available: Amount
  pending: Amount
  total: Amount
}

export interface WalletInfo {
  address: Address
  balance: Balance
  transactionCount: number
  lastSyncHeight: BlockHeight
}

// ============================================================================
// Node/Network Types
// ============================================================================

export type NodeStatus = 'online' | 'connecting' | 'offline' | 'error'

export interface NodeInfo {
  id: string
  host: string
  port: number
  version?: string
  blockHeight?: BlockHeight
  networkId: string
  latency?: number
  status: NodeStatus
}

export interface NetworkStats {
  blockHeight: BlockHeight
  difficulty: bigint
  /**
   * Network hash rate as a decimal string. `null` when the data source does
   * not expose it (e.g. the seed-node read RPC), in which case consumers render
   * "n/a"/"—" rather than a fabricated "0" (#913, derive-or-n/a rule in
   * docs/design/block-explorer-network-stats.md).
   */
  hashRate: string | null
  connectedPeers: number
  mempoolSize: number
}

// ============================================================================
// Address Book Types
// ============================================================================

export interface Contact {
  id: string
  name: string
  address: Address
  notes?: string
  createdAt: Timestamp
  updatedAt: Timestamp
  /** Number of transactions with this contact */
  txCount: number
  /** Last transaction timestamp */
  lastTxAt?: Timestamp
}

// ============================================================================
// Block Types
// ============================================================================

/**
 * Privacy-safe per-transaction structure inside a block (#699/#700).
 * Structure only: hash, fee, ring size — never amounts, recipients, or
 * linkage data.
 */
export interface BlockTransactionSummary {
  hash: TxHash
  /** Transaction fee in picocredits. */
  fee: Amount
  /** Ring-member count of the tx's CLSAG inputs (all inputs share it). */
  ringSize: number
}

/**
 * On-chain lottery summary for a block (#699/#700). All amounts are
 * picocredits. Blocks without lottery activity carry explicit zeros.
 */
export interface BlockLotterySummary {
  totalFees: Amount
  poolDistributed: Amount
  amountBurned: Amount
  /** Hex-encoded lottery seed for this block. */
  lotterySeed: string
  payoutCount: number
  payoutTotal: Amount
}

export interface Block {
  hash: BlockHash
  height: BlockHeight
  timestamp: Timestamp
  previousHash: BlockHash
  transactionCount: number
  /**
   * Serialized block size in bytes. Present for blocks fetched via the enriched
   * RPC. Omitted for blocks delivered over the live WebSocket event, whose
   * payload does not carry a size — consumers render "—" rather than a
   * fabricated `0` (#924, #541-class fabrication).
   */
  size?: number
  minter?: Address
  /**
   * Block reward in picocredits. Present for blocks fetched via the enriched
   * RPC. Omitted for blocks delivered over the live WebSocket event, whose
   * payload does not carry a reward — consumers omit the reward rather than
   * flash a fabricated "+0" (#924, #541-class fabrication).
   */
  reward?: Amount
  difficulty: bigint
  /**
   * Enriched explorer fields (#700). Optional and additive — older nodes
   * omit them, and consumers must guard with undefined checks.
   */
  transactions?: BlockTransactionSummary[]
  /** Sum of all tx fees in the block, in picocredits. */
  totalFees?: Amount
  /** Lottery summary; present when the node serves the enriched RPC. */
  lottery?: BlockLotterySummary
}

// ============================================================================
// Minting Types
// ============================================================================

export type MintingStatus = 'idle' | 'minting' | 'paused'

export interface MintingStats {
  status: MintingStatus
  hashRate: number
  blocksFound: number
  totalRewards: Amount
  currentDifficulty: bigint
  estimatedTimeToBlock?: number
}
