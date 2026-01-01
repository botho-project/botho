import type {
  Address,
  Balance,
  Block,
  BlockHeight,
  CryptoType,
  NetworkStats,
  NodeInfo,
  Transaction,
  TxHash,
} from '@botho/core'
import type {
  BlockFetchOptions,
  NodeAdapter,
  RemoteNodeConfig,
  TxHistoryOptions,
  TxSubmitResult,
} from './types'

const DEFAULT_CONFIG: Required<RemoteNodeConfig> = {
  seedNodes: ['https://seed.botho.io'],
  networkId: 'botho-mainnet',
  timeout: 10000,
  useWebSocket: true,
}

/** JSON-RPC 2.0 request */
interface JsonRpcRequest {
  jsonrpc: '2.0'
  method: string
  params: Record<string, unknown>
  id: number
}

/** JSON-RPC 2.0 response */
interface JsonRpcResponse<T = unknown> {
  jsonrpc: '2.0'
  result?: T
  error?: { code: number; message: string }
  id: number
}

/**
 * Remote node adapter for connecting to Botho seed nodes
 * Uses JSON-RPC 2.0 protocol over HTTPS
 */
export class RemoteNodeAdapter implements NodeAdapter {
  private config: Required<RemoteNodeConfig>
  private connected = false
  private currentNode: NodeInfo | null = null
  private currentSeedUrl: string | null = null
  private ws: WebSocket | null = null
  private blockCallbacks: Set<(block: Block) => void> = new Set()
  private txCallbacks: Map<string, Set<(tx: Transaction) => void>> = new Map()
  private rpcId = 0

  constructor(config: Partial<RemoteNodeConfig> = {}) {
    this.config = { ...DEFAULT_CONFIG, ...config }
  }

  async connect(): Promise<void> {
    // Try each seed node until one works
    for (const seedUrl of this.config.seedNodes) {
      try {
        const result = await this.rpcCall<{
          version: string
          network: string
          chainHeight: number
          peerCount: number
        }>(seedUrl, 'node_getStatus', {})

        if (result) {
          this.currentSeedUrl = seedUrl
          this.currentNode = {
            id: seedUrl,
            host: new URL(seedUrl).hostname,
            port: 443,
            version: result.version,
            blockHeight: result.chainHeight,
            networkId: result.network || this.config.networkId,
            status: 'online',
          }
          this.connected = true

          // Set up WebSocket for real-time updates (if supported)
          if (this.config.useWebSocket) {
            this.setupWebSocket(seedUrl)
          }

          return
        }
      } catch {
        // Try next node
        continue
      }
    }

    throw new Error('Failed to connect to any seed node')
  }

  disconnect(): void {
    this.connected = false
    this.currentNode = null
    this.currentSeedUrl = null
    if (this.ws) {
      this.ws.close()
      this.ws = null
    }
    this.blockCallbacks.clear()
    this.txCallbacks.clear()
  }

  isConnected(): boolean {
    return this.connected
  }

  getNodeInfo(): NodeInfo | null {
    return this.currentNode
  }

  // =========================================================================
  // Blockchain Queries
  // =========================================================================

  async getBlockHeight(): Promise<BlockHeight> {
    const result = await this.call<{ chainHeight: number }>('node_getStatus', {})
    return result.chainHeight
  }

  async getNetworkStats(): Promise<NetworkStats> {
    const [status, chain] = await Promise.all([
      this.call<{
        chainHeight: number
        peerCount: number
        mempoolSize: number
      }>('node_getStatus', {}),
      this.call<{
        difficulty: number
      }>('getChainInfo', {}),
    ])

    return {
      blockHeight: status.chainHeight,
      difficulty: BigInt(chain.difficulty || 0),
      hashRate: '0', // Not provided by RPC
      connectedPeers: status.peerCount,
      mempoolSize: status.mempoolSize,
    }
  }

  async getBlock(heightOrHash: BlockHeight | string): Promise<Block | null> {
    try {
      if (typeof heightOrHash === 'number') {
        const result = await this.call<{
          height: number
          hash: string
          prevHash: string
          timestamp: number
          difficulty: number
          txCount: number
          mintingReward: number
        }>('getBlockByHeight', { height: heightOrHash })

        return {
          hash: result.hash,
          height: result.height,
          timestamp: result.timestamp,
          previousHash: result.prevHash,
          transactionCount: result.txCount,
          size: 0, // Not provided
          reward: BigInt(result.mintingReward || 0),
          difficulty: BigInt(result.difficulty || 0),
        }
      } else {
        // Hash lookup not directly supported, would need additional RPC method
        return null
      }
    } catch {
      return null
    }
  }

  async getRecentBlocks(options?: BlockFetchOptions): Promise<Block[]> {
    const blocks: Block[] = []
    const limit = options?.limit || 10
    const startHeight = options?.startHeight || 0

    // Fetch blocks sequentially (could optimize with batch RPC)
    for (let i = 0; i < limit; i++) {
      const block = await this.getBlock(startHeight + i)
      if (block) {
        blocks.push(block)
      }
    }

    return blocks
  }

  // =========================================================================
  // Wallet Queries
  // =========================================================================

  async getBalance(_addresses: Address[]): Promise<Balance> {
    // The RPC's wallet_getBalance doesn't take addresses (server-side wallet)
    // For thin wallet, balance is computed client-side from outputs
    const result = await this.call<{
      confirmed: number
      pending: number
      total: number
    }>('wallet_getBalance', {})

    return {
      available: BigInt(result.confirmed || 0),
      pending: BigInt(result.pending || 0),
      total: BigInt(result.total || 0),
    }
  }

  async getTransactionHistory(_addresses: Address[], options?: TxHistoryOptions): Promise<Transaction[]> {
    // Fetch outputs for the height range and filter client-side
    const result = await this.call<Array<{
      height: number
      outputs: Array<{
        txHash: string
        outputIndex: number
        targetKey: string
        publicKey: string
        amountCommitment: string
      }>
    }>>('chain_getOutputs', {
      start_height: options?.startHeight || 0,
      end_height: options?.endHeight || (options?.startHeight || 0) + 100,
    })

    // For now, return empty - client should process outputs locally
    // Full implementation would scan outputs for matching addresses
    return result.flatMap((block) =>
      block.outputs.map((output) => ({
        id: output.txHash,
        type: 'receive' as const,
        amount: BigInt(0), // Would need to decrypt
        fee: BigInt(0),
        privacyLevel: 'private' as const, // Ring signatures for privacy
        cryptoType: 'clsag' as CryptoType, // Default to classical, actual type from RPC
        status: 'confirmed' as const,
        timestamp: Date.now(),
        blockHeight: block.height,
        confirmations: 0,
      }))
    )
  }

  async getTransaction(_txHash: TxHash): Promise<Transaction | null> {
    // Not directly supported by current RPC
    // Would need to scan blocks or add a dedicated method
    return null
  }

  // =========================================================================
  // Transaction Submission
  // =========================================================================

  async submitTransaction(signedTx: Uint8Array): Promise<TxSubmitResult> {
    try {
      const txHex = Array.from(signedTx)
        .map((b) => b.toString(16).padStart(2, '0'))
        .join('')

      const result = await this.call<{ txHash: string }>('tx_submit', {
        tx_hex: txHex,
      })

      return {
        success: true,
        txHash: result.txHash,
      }
    } catch (err) {
      return {
        success: false,
        error: err instanceof Error ? err.message : 'Unknown error',
      }
    }
  }

  async estimateFee(_sizeBytes: number, _clusterWealth?: bigint): Promise<bigint> {
    const result = await this.call<{
      minimumFee: number
      recommendedFee: number
    }>('estimateFee', {
      amount: 0,
      private: true,
      memos: 0,
    })

    return BigInt(result.recommendedFee || result.minimumFee || 0)
  }

  // =========================================================================
  // Events
  // =========================================================================

  onNewBlock(callback: (block: Block) => void): () => void {
    this.blockCallbacks.add(callback)
    return () => this.blockCallbacks.delete(callback)
  }

  onTransaction(addresses: Address[], callback: (tx: Transaction) => void): () => void {
    const key = addresses.sort().join(',')
    if (!this.txCallbacks.has(key)) {
      this.txCallbacks.set(key, new Set())
      this.sendWsMessage({ type: 'subscribe', addresses })
    }
    this.txCallbacks.get(key)!.add(callback)

    return () => {
      const callbacks = this.txCallbacks.get(key)
      if (callbacks) {
        callbacks.delete(callback)
        if (callbacks.size === 0) {
          this.txCallbacks.delete(key)
          this.sendWsMessage({ type: 'unsubscribe', addresses })
        }
      }
    }
  }

  // =========================================================================
  // Private Helpers
  // =========================================================================

  private async rpcCall<T>(
    baseUrl: string,
    method: string,
    params: Record<string, unknown>
  ): Promise<T> {
    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), this.config.timeout)

    const request: JsonRpcRequest = {
      jsonrpc: '2.0',
      method,
      params,
      id: ++this.rpcId,
    }

    try {
      const response = await fetch(baseUrl, {
        method: 'POST',
        signal: controller.signal,
        headers: {
          'Content-Type': 'application/json',
        },
        body: JSON.stringify(request),
      })

      const json: JsonRpcResponse<T> = await response.json()

      if (json.error) {
        throw new Error(json.error.message)
      }

      return json.result as T
    } finally {
      clearTimeout(timeoutId)
    }
  }

  private async call<T>(method: string, params: Record<string, unknown>): Promise<T> {
    if (!this.connected || !this.currentSeedUrl) {
      throw new Error('Not connected to any node')
    }
    return this.rpcCall<T>(this.currentSeedUrl, method, params)
  }

  private setupWebSocket(seedUrl: string): void {
    // Connect to node WebSocket endpoint for real-time events
    const wsUrl = seedUrl.replace(/^http/, 'ws') + '/ws'

    try {
      this.ws = new WebSocket(wsUrl)

      this.ws.onopen = () => {
        // Subscribe to block events by default
        this.sendWsMessage({ type: 'subscribe', events: ['blocks', 'transactions'] })
      }

      this.ws.onmessage = (event) => {
        try {
          const msg = JSON.parse(event.data)
          if (msg.type === 'event') {
            // New event format: { type: "event", event: "block", data: {...} }
            if (msg.event === 'block') {
              const block = this.parseBlockEvent(msg.data)
              this.blockCallbacks.forEach((cb) => cb(block))
            } else if (msg.event === 'transaction') {
              const tx = this.parseTransactionEvent(msg.data)
              this.txCallbacks.forEach((callbacks) => {
                callbacks.forEach((cb) => cb(tx))
              })
            }
          } else if (msg.type === 'subscribed') {
            // Subscription confirmed
            console.debug('WebSocket subscribed to:', msg.events)
          }
        } catch {
          // Ignore malformed messages
        }
      }

      this.ws.onclose = () => {
        if (this.connected) {
          // Exponential backoff reconnection
          setTimeout(() => {
            if (this.connected) {
              this.setupWebSocket(seedUrl)
            }
          }, 5000)
        }
      }

      this.ws.onerror = () => {
        // WebSocket not supported, fall back to polling
        this.ws = null
      }
    } catch {
      // WebSocket connection failed, continue without real-time updates
      this.ws = null
    }
  }

  private sendWsMessage(msg: unknown): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg))
    }
  }

  /** Parse block from WebSocket event */
  private parseBlockEvent(data: Record<string, unknown>): Block {
    return {
      hash: data.hash as string,
      height: data.height as number,
      timestamp: data.timestamp as number,
      previousHash: '', // Not included in WS event
      transactionCount: data.tx_count as number,
      size: 0, // Not included in WS event
      reward: BigInt(0), // Not included in WS event
      difficulty: BigInt((data.difficulty as number) || 0),
    }
  }

  /** Parse transaction from WebSocket event */
  private parseTransactionEvent(data: Record<string, unknown>): Transaction {
    // Map RPC type field to CryptoType
    const rpcType = data.type as string | undefined
    let cryptoType: CryptoType = 'clsag' // default
    if (rpcType === 'lion') {
      cryptoType = 'lion'
    } else if (rpcType === 'hybrid') {
      cryptoType = 'hybrid'
    }

    return {
      id: data.hash as string,
      type: 'receive' as const,
      amount: BigInt(0), // Private - not visible
      fee: BigInt((data.fee as number) || 0),
      privacyLevel: 'private' as const,
      cryptoType,
      status: data.in_block ? 'confirmed' as const : 'pending' as const,
      timestamp: Date.now(),
      blockHeight: data.in_block as number | undefined,
      confirmations: data.in_block ? 1 : 0,
    }
  }
}
