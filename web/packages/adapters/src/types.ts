import type {
  Address,
  Balance,
  Block,
  BlockHeight,
  NetworkStats,
  NodeInfo,
  Transaction,
  TxHash,
} from '@botho/core'

/**
 * WebSocket connection status
 */
export type WsConnectionStatus = 'connected' | 'connecting' | 'disconnected' | 'reconnecting'

/**
 * Mempool update event from WebSocket
 */
export interface MempoolUpdate {
  /** Number of transactions in mempool */
  size: number
  /** Total fees in mempool (picocredits) */
  totalFees: bigint
}

/**
 * Peer status event from WebSocket
 */
export interface PeerStatus {
  /** Current peer count */
  peerCount: number
  /** Event type */
  event: 'connected' | 'disconnected' | 'count_changed'
  /** Peer ID (if connect/disconnect) */
  peerId?: string
}

/**
 * Result of submitting a transaction
 */
export interface TxSubmitResult {
  success: boolean
  txHash?: TxHash
  error?: string
}

/**
 * Result of estimating a transaction fee.
 *
 * Carries both the fee amount and the node-computed cluster-factor display
 * string so the send UI can show *why* the fee is what it is (progressive fee,
 * #626/#628/#634). The node computes `clusterFactorDisplay` server-side from the
 * live log-domain curve — clients must never hardcode a factor table.
 */
export interface FeeEstimate {
  /** Recommended fee in picocredits. */
  fee: bigint
  /**
   * Node-computed display string for the cluster fee factor, e.g. "1.85x".
   * Always "1.00x" at the base rate (no cluster-wealth premium).
   */
  clusterFactorDisplay: string
}

/**
 * Options for fetching transaction history
 */
export interface TxHistoryOptions {
  limit?: number
  offset?: number
  startHeight?: BlockHeight
  endHeight?: BlockHeight
}

/**
 * Options for fetching blocks
 */
export interface BlockFetchOptions {
  limit?: number
  startHeight?: BlockHeight
}

/**
 * Abstract interface for connecting to Botho nodes
 *
 * This allows the same wallet UI to work with:
 * - Remote nodes (web wallet via botho.io)
 * - Local nodes (desktop app via localhost)
 */
export interface NodeAdapter {
  /**
   * Connect to the node(s)
   */
  connect(): Promise<void>

  /**
   * Disconnect from the node(s)
   */
  disconnect(): void

  /**
   * Check if connected
   */
  isConnected(): boolean

  /**
   * Get information about the connected node
   */
  getNodeInfo(): NodeInfo | null

  // =========================================================================
  // Blockchain Queries
  // =========================================================================

  /**
   * Get the current block height
   */
  getBlockHeight(): Promise<BlockHeight>

  /**
   * Get network statistics
   */
  getNetworkStats(): Promise<NetworkStats>

  /**
   * Get a specific block by height or hash
   */
  getBlock(heightOrHash: BlockHeight | string): Promise<Block | null>

  /**
   * Get recent blocks
   */
  getRecentBlocks(options?: BlockFetchOptions): Promise<Block[]>

  // =========================================================================
  // Wallet Queries
  // =========================================================================

  /**
   * Get the balance for an address or set of addresses
   */
  getBalance(addresses: Address[]): Promise<Balance>

  /**
   * Get transaction history for an address or set of addresses
   */
  getTransactionHistory(addresses: Address[], options?: TxHistoryOptions): Promise<Transaction[]>

  /**
   * Get a specific transaction by hash
   */
  getTransaction(txHash: TxHash): Promise<Transaction | null>

  // =========================================================================
  // Transaction Submission
  // =========================================================================

  /**
   * Submit a signed transaction to the network
   */
  submitTransaction(signedTx: Uint8Array): Promise<TxSubmitResult>

  /**
   * Estimate the fee for a transaction.
   *
   * Returns a {@link FeeEstimate} carrying both the fee and the node-computed
   * `clusterFactorDisplay` (e.g. "1.85x") so the send UI can explain the
   * progressive fee. Callers on older nodes that omit the factor see "1.00x".
   *
   * @param sizeBytes Estimated transaction size in bytes
   * @param clusterWealth Total wealth in the sender's cluster (for progressive fees)
   */
  estimateFee(sizeBytes: number, clusterWealth?: bigint): Promise<FeeEstimate>

  /**
   * Look up the sender's cluster wealth for a set of owned output target keys,
   * for use as the `clusterWealth` argument to {@link estimateFee} (progressive
   * fees, #626/#628/#634).
   *
   * The value is a string-encoded u128 on the wire and MUST be parsed via
   * `BigInt()` (never `Number()`) — cluster wealth can exceed
   * `Number.MAX_SAFE_INTEGER`. Returns `0n` for an empty target-key list.
   *
   * @param targetKeys Hex-encoded target keys of the wallet's owned outputs
   */
  getClusterWealth(targetKeys: string[]): Promise<bigint>

  // =========================================================================
  // Events
  // =========================================================================

  /**
   * Subscribe to new blocks
   */
  onNewBlock(callback: (block: Block) => void): () => void

  /**
   * Subscribe to new transactions for watched addresses
   */
  onTransaction(addresses: Address[], callback: (tx: Transaction) => void): () => void

  /**
   * Subscribe to mempool updates
   */
  onMempoolUpdate(callback: (update: MempoolUpdate) => void): () => void

  /**
   * Subscribe to peer status changes
   */
  onPeerStatus(callback: (status: PeerStatus) => void): () => void

  // =========================================================================
  // WebSocket Status
  // =========================================================================

  /**
   * Get current WebSocket connection status
   */
  getWsStatus(): WsConnectionStatus

  /**
   * Subscribe to WebSocket connection status changes
   */
  onWsStatusChange(callback: (status: WsConnectionStatus) => void): () => void
}

/**
 * Configuration for remote node adapter
 */
export interface RemoteNodeConfig {
  /** Seed node URLs (e.g., ["https://seed1.botho.io", "https://seed2.botho.io"]) */
  seedNodes: string[]
  /** Network ID for validation */
  networkId: string
  /** Connection timeout in ms */
  timeout?: number
  /** Whether to use WebSocket for real-time updates */
  useWebSocket?: boolean
}

/**
 * Configuration for local node adapter
 */
export interface LocalNodeConfig {
  /** Host (default: localhost) */
  host?: string
  /** Port to probe or connect to */
  port?: number
  /** Ports to scan if port not specified */
  scanPorts?: number[]
  /** Connection timeout in ms */
  timeout?: number
}
