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
export type PrivacyLevel = 'standard' | 'private' // standard = ML-DSA, private = LION ring signatures

/** Cryptographic signature type used in transaction */
export type CryptoType = 'clsag' | 'lion' | 'hybrid'

export interface Transaction {
  id: TxHash
  type: TransactionType
  amount: Amount
  fee: Amount
  privacyLevel: PrivacyLevel
  /** Ring signature type: clsag (classical), lion (quantum-safe), or hybrid */
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
