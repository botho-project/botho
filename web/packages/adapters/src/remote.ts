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
  MempoolUpdate,
  NodeAdapter,
  PeerStatus,
  RemoteNodeConfig,
  TxHistoryOptions,
  TxSubmitResult,
  WsConnectionStatus,
} from './types'

const DEFAULT_CONFIG: Required<RemoteNodeConfig> = {
  seedNodes: ['https://faucet.botho.io/rpc'],
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
  private wsStatus: WsConnectionStatus = 'disconnected'
  private wsReconnectAttempt = 0
  private wsReconnectTimer: ReturnType<typeof setTimeout> | null = null
  private blockCallbacks: Set<(block: Block) => void> = new Set()
  private txCallbacks: Map<string, Set<(tx: Transaction) => void>> = new Map()
  private mempoolCallbacks: Set<(update: MempoolUpdate) => void> = new Set()
  private peerCallbacks: Set<(status: PeerStatus) => void> = new Set()
  private wsStatusCallbacks: Set<(status: WsConnectionStatus) => void> = new Set()
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
    if (this.wsReconnectTimer) {
      clearTimeout(this.wsReconnectTimer)
      this.wsReconnectTimer = null
    }
    if (this.ws) {
      this.ws.close()
      this.ws = null
    }
    this.setWsStatus('disconnected')
    this.blockCallbacks.clear()
    this.txCallbacks.clear()
    this.mempoolCallbacks.clear()
    this.peerCallbacks.clear()
    this.wsStatusCallbacks.clear()
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

    // If no startHeight specified, fetch from the chain tip (most recent)
    let startHeight = options?.startHeight
    if (startHeight === undefined) {
      const chainHeight = await this.getBlockHeight()
      startHeight = Math.max(0, chainHeight - limit + 1)
    }

    // Fetch blocks and return in descending order (newest first)
    for (let i = limit - 1; i >= 0; i--) {
      const height = startHeight + i
      if (height < 0) continue
      const block = await this.getBlock(height)
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

  onMempoolUpdate(callback: (update: MempoolUpdate) => void): () => void {
    this.mempoolCallbacks.add(callback)
    return () => this.mempoolCallbacks.delete(callback)
  }

  onPeerStatus(callback: (status: PeerStatus) => void): () => void {
    this.peerCallbacks.add(callback)
    return () => this.peerCallbacks.delete(callback)
  }

  // =========================================================================
  // WebSocket Status
  // =========================================================================

  getWsStatus(): WsConnectionStatus {
    return this.wsStatus
  }

  onWsStatusChange(callback: (status: WsConnectionStatus) => void): () => void {
    this.wsStatusCallbacks.add(callback)
    return () => this.wsStatusCallbacks.delete(callback)
  }

  // =========================================================================
  // Private Helpers
  // =========================================================================

  private setWsStatus(status: WsConnectionStatus): void {
    if (this.wsStatus !== status) {
      this.wsStatus = status
      this.wsStatusCallbacks.forEach((cb) => cb(status))
    }
  }

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

    this.setWsStatus(this.wsReconnectAttempt > 0 ? 'reconnecting' : 'connecting')

    try {
      this.ws = new WebSocket(wsUrl)

      this.ws.onopen = () => {
        // Reset reconnection state on successful connection
        this.wsReconnectAttempt = 0
        this.setWsStatus('connected')

        // Subscribe to all event types
        this.sendWsMessage({
          type: 'subscribe',
          events: ['blocks', 'transactions', 'mempool', 'peers']
        })
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
            } else if (msg.event === 'mempool') {
              const update = this.parseMempoolEvent(msg.data)
              this.mempoolCallbacks.forEach((cb) => cb(update))
            } else if (msg.event === 'peers') {
              const status = this.parsePeerEvent(msg.data)
              this.peerCallbacks.forEach((cb) => cb(status))
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
        this.ws = null
        if (this.connected) {
          this.setWsStatus('reconnecting')
          // Exponential backoff with jitter for reconnection
          // Base delay: 1s, max delay: 30s
          const baseDelay = 1000
          const maxDelay = 30000
          const delay = Math.min(baseDelay * Math.pow(2, this.wsReconnectAttempt), maxDelay)
          // Add jitter (±25%)
          const jitter = delay * 0.25 * (Math.random() * 2 - 1)
          const finalDelay = Math.round(delay + jitter)

          this.wsReconnectAttempt++
          this.wsReconnectTimer = setTimeout(() => {
            this.wsReconnectTimer = null
            if (this.connected) {
              this.setupWebSocket(seedUrl)
            }
          }, finalDelay)
        } else {
          this.setWsStatus('disconnected')
        }
      }

      this.ws.onerror = () => {
        // WebSocket error - onclose will be called next, which handles reconnection
        // Don't set ws to null here as onclose handles cleanup
      }
    } catch {
      // WebSocket connection failed, continue without real-time updates
      this.ws = null
      this.setWsStatus('disconnected')
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
    if (rpcType === 'mldsa') {
      cryptoType = 'mldsa'
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

  /** Parse mempool update from WebSocket event */
  private parseMempoolEvent(data: Record<string, unknown>): MempoolUpdate {
    return {
      size: (data.size as number) || 0,
      totalFees: BigInt((data.total_fees as number) || 0),
    }
  }

  /** Parse peer status from WebSocket event */
  private parsePeerEvent(data: Record<string, unknown>): PeerStatus {
    const eventData = data.event as Record<string, unknown> | undefined
    let event: PeerStatus['event'] = 'count_changed'
    let peerId: string | undefined

    if (eventData) {
      if ('Connected' in eventData) {
        event = 'connected'
        peerId = (eventData.Connected as Record<string, unknown>)?.peer_id as string
      } else if ('Disconnected' in eventData) {
        event = 'disconnected'
        peerId = (eventData.Disconnected as Record<string, unknown>)?.peer_id as string
      }
    }

    return {
      peerCount: (data.peer_count as number) || 0,
      event,
      peerId,
    }
  }
}
