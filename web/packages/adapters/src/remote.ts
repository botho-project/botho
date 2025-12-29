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

/**
 * Remote node adapter for connecting to Botho seed nodes
 * Used by the web wallet at botho.io
 */
export class RemoteNodeAdapter implements NodeAdapter {
  private config: Required<RemoteNodeConfig>
  private connected = false
  private currentNode: NodeInfo | null = null
  private ws: WebSocket | null = null
  private blockCallbacks: Set<(block: Block) => void> = new Set()
  private txCallbacks: Map<string, Set<(tx: Transaction) => void>> = new Map()

  constructor(config: Partial<RemoteNodeConfig> = {}) {
    this.config = { ...DEFAULT_CONFIG, ...config }
  }

  async connect(): Promise<void> {
    // Try each seed node until one works
    for (const seedUrl of this.config.seedNodes) {
      try {
        const response = await this.fetch(seedUrl, '/api/status')
        if (response.ok) {
          const data = await response.json()
          this.currentNode = {
            id: seedUrl,
            host: new URL(seedUrl).hostname,
            port: 443,
            version: data.version,
            blockHeight: data.blockHeight,
            networkId: data.networkId || this.config.networkId,
            status: 'online',
          }
          this.connected = true

          // Set up WebSocket for real-time updates
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
    const response = await this.fetchApi('/api/status')
    const data = await response.json()
    return data.blockHeight
  }

  async getNetworkStats(): Promise<NetworkStats> {
    const response = await this.fetchApi('/api/network/stats')
    const data = await response.json()
    return {
      blockHeight: data.blockHeight,
      difficulty: BigInt(data.difficulty),
      hashRate: data.hashRate,
      connectedPeers: data.connectedPeers,
      mempoolSize: data.mempoolSize,
    }
  }

  async getBlock(heightOrHash: BlockHeight | string): Promise<Block | null> {
    try {
      const endpoint = typeof heightOrHash === 'number'
        ? `/api/blocks/height/${heightOrHash}`
        : `/api/blocks/${heightOrHash}`
      const response = await this.fetchApi(endpoint)
      if (!response.ok) return null
      return this.parseBlock(await response.json())
    } catch {
      return null
    }
  }

  async getRecentBlocks(options?: BlockFetchOptions): Promise<Block[]> {
    const params = new URLSearchParams()
    if (options?.limit) params.set('limit', options.limit.toString())
    if (options?.startHeight) params.set('start', options.startHeight.toString())

    const response = await this.fetchApi(`/api/blocks?${params}`)
    const data = await response.json()
    return data.blocks.map((b: unknown) => this.parseBlock(b))
  }

  // =========================================================================
  // Wallet Queries
  // =========================================================================

  async getBalance(addresses: Address[]): Promise<Balance> {
    const response = await this.fetchApi('/api/wallet/balance', {
      method: 'POST',
      body: JSON.stringify({ addresses }),
    })
    const data = await response.json()
    return {
      available: BigInt(data.available),
      pending: BigInt(data.pending),
      total: BigInt(data.total),
    }
  }

  async getTransactionHistory(addresses: Address[], options?: TxHistoryOptions): Promise<Transaction[]> {
    const params = new URLSearchParams()
    if (options?.limit) params.set('limit', options.limit.toString())
    if (options?.offset) params.set('offset', options.offset.toString())
    if (options?.startHeight) params.set('startHeight', options.startHeight.toString())
    if (options?.endHeight) params.set('endHeight', options.endHeight.toString())

    const response = await this.fetchApi(`/api/wallet/transactions?${params}`, {
      method: 'POST',
      body: JSON.stringify({ addresses }),
    })
    const data = await response.json()
    return data.transactions.map((t: unknown) => this.parseTransaction(t))
  }

  async getTransaction(txHash: TxHash): Promise<Transaction | null> {
    try {
      const response = await this.fetchApi(`/api/transactions/${txHash}`)
      if (!response.ok) return null
      return this.parseTransaction(await response.json())
    } catch {
      return null
    }
  }

  // =========================================================================
  // Transaction Submission
  // =========================================================================

  async submitTransaction(signedTx: Uint8Array): Promise<TxSubmitResult> {
    try {
      const response = await this.fetchApi('/api/transactions', {
        method: 'POST',
        headers: { 'Content-Type': 'application/octet-stream' },
        body: signedTx,
      })
      const data = await response.json()
      return {
        success: response.ok,
        txHash: data.txHash,
        error: data.error,
      }
    } catch (err) {
      return {
        success: false,
        error: err instanceof Error ? err.message : 'Unknown error',
      }
    }
  }

  async estimateFee(sizeBytes: number, clusterWealth?: bigint): Promise<bigint> {
    const response = await this.fetchApi('/api/fees/estimate', {
      method: 'POST',
      body: JSON.stringify({
        sizeBytes,
        clusterWealth: clusterWealth?.toString(),
      }),
    })
    const data = await response.json()
    return BigInt(data.fee)
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
      // Subscribe via WebSocket
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

  private async fetch(baseUrl: string, path: string, options?: RequestInit): Promise<Response> {
    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), this.config.timeout)

    try {
      return await fetch(`${baseUrl}${path}`, {
        ...options,
        signal: controller.signal,
        headers: {
          'Content-Type': 'application/json',
          ...options?.headers,
        },
      })
    } finally {
      clearTimeout(timeoutId)
    }
  }

  private async fetchApi(path: string, options?: RequestInit): Promise<Response> {
    if (!this.connected || !this.currentNode) {
      throw new Error('Not connected to any node')
    }
    const baseUrl = this.config.seedNodes.find(
      url => new URL(url).hostname === this.currentNode!.host
    )!
    return this.fetch(baseUrl, path, options)
  }

  private setupWebSocket(seedUrl: string): void {
    const wsUrl = seedUrl.replace(/^http/, 'ws') + '/ws'
    this.ws = new WebSocket(wsUrl)

    this.ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data)
        if (msg.type === 'block') {
          const block = this.parseBlock(msg.data)
          this.blockCallbacks.forEach(cb => cb(block))
        } else if (msg.type === 'transaction') {
          const tx = this.parseTransaction(msg.data)
          // Notify relevant subscribers
          this.txCallbacks.forEach((callbacks, key) => {
            const addresses = key.split(',')
            if (tx.counterparty && addresses.includes(tx.counterparty)) {
              callbacks.forEach(cb => cb(tx))
            }
          })
        }
      } catch {
        // Ignore malformed messages
      }
    }

    this.ws.onclose = () => {
      // Attempt to reconnect after a delay
      if (this.connected) {
        setTimeout(() => {
          if (this.connected) {
            this.setupWebSocket(seedUrl)
          }
        }, 5000)
      }
    }
  }

  private sendWsMessage(msg: unknown): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg))
    }
  }

  private parseBlock(data: Record<string, unknown>): Block {
    return {
      hash: data.hash as string,
      height: data.height as number,
      timestamp: data.timestamp as number,
      previousHash: data.previousHash as string,
      transactionCount: data.transactionCount as number,
      size: data.size as number,
      miner: data.miner as string | undefined,
      reward: BigInt((data.reward as string) || '0'),
      difficulty: BigInt((data.difficulty as string) || '0'),
    }
  }

  private parseTransaction(data: Record<string, unknown>): Transaction {
    return {
      id: data.id as string,
      type: data.type as Transaction['type'],
      amount: BigInt((data.amount as string) || '0'),
      fee: BigInt((data.fee as string) || '0'),
      privacyLevel: data.privacyLevel as Transaction['privacyLevel'],
      status: data.status as Transaction['status'],
      timestamp: data.timestamp as number,
      blockHeight: data.blockHeight as number | undefined,
      confirmations: data.confirmations as number,
      counterparty: data.counterparty as string | undefined,
      memo: data.memo as string | undefined,
    }
  }
}
