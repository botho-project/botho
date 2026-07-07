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
export type PrivacyLevel = 'standard' | 'private' // standard = Minting (ML-DSA), private = CLSAG ring signatures

/** Cryptographic signature type used in transaction */
export type CryptoType = 'clsag' | 'mldsa' | 'hybrid'

export interface Transaction {
  id: TxHash
  type: TransactionType
  amount: Amount
  fee: Amount
  privacyLevel: PrivacyLevel
  /** Signature type: clsag (ring signatures for private tx), mldsa (minting tx), or hybrid */
  cryptoType: CryptoType
  status: TransactionStatus
  timestamp: Timestamp
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
  hashRate: string
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
  size: number
  minter?: Address
  reward: Amount
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
