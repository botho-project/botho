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
  LocalNodeConfig,
  NodeAdapter,
  TxHistoryOptions,
  TxSubmitResult,
} from './types'

const DEFAULT_CONFIG: Required<LocalNodeConfig> = {
  host: 'localhost',
  port: 0, // 0 means scan
  scanPorts: [8080, 8081, 8082, 8083, 8084, 3000, 3001, 9090, 9091],
  timeout: 2000,
}

/**
 * Local node adapter for connecting to a locally running Botho node
 * Used by the desktop Tauri app
 */
export class LocalNodeAdapter implements NodeAdapter {
  private config: Required<LocalNodeConfig>
  private connected = false
  private currentNode: NodeInfo | null = null
  private eventSource: EventSource | null = null
  private blockCallbacks: Set<(block: Block) => void> = new Set()
  private txCallbacks: Map<string, Set<(tx: Transaction) => void>> = new Map()

  constructor(config: Partial<LocalNodeConfig> = {}) {
    this.config = { ...DEFAULT_CONFIG, ...config }
  }

  async connect(): Promise<void> {
    // If a specific port is given, try that first
    if (this.config.port > 0) {
      const node = await this.probeNode(this.config.host, this.config.port)
      if (node) {
        this.currentNode = node
        this.connected = true
        this.setupEventSource()
        return
      }
    }

    // Otherwise, scan common ports
    for (const port of this.config.scanPorts) {
      const node = await this.probeNode(this.config.host, port)
      if (node) {
        this.currentNode = node
        this.connected = true
        this.setupEventSource()
        return
      }
    }

    throw new Error('No local Botho node found. Is the node running?')
  }

  disconnect(): void {
    this.connected = false
    this.currentNode = null
    if (this.eventSource) {
      this.eventSource.close()
      this.eventSource = null
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
    return data.blocks.map((b: unknown) => this.parseBlock(b as Record<string, unknown>))
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
    return data.transactions.map((t: unknown) => this.parseTransaction(t as Record<string, unknown>))
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
        body: signedTx as unknown as BodyInit,
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
    }
    this.txCallbacks.get(key)!.add(callback)

    return () => {
      const callbacks = this.txCallbacks.get(key)
      if (callbacks) {
        callbacks.delete(callback)
        if (callbacks.size === 0) {
          this.txCallbacks.delete(key)
        }
      }
    }
  }

  // =========================================================================
  // Local-specific methods
  // =========================================================================

  /**
   * Scan for local nodes on common ports
   */
  async scanForNodes(): Promise<NodeInfo[]> {
    const nodes: NodeInfo[] = []
    const hosts = ['localhost', '127.0.0.1']

    for (const host of hosts) {
      for (const port of this.config.scanPorts) {
        const node = await this.probeNode(host, port)
        if (node) {
          // Deduplicate
          if (!nodes.some(n => n.port === port)) {
            nodes.push(node)
          }
        }
      }
    }

    return nodes
  }

  // =========================================================================
  // Private Helpers
  // =========================================================================

  private async probeNode(host: string, port: number): Promise<NodeInfo | null> {
    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), this.config.timeout)

    try {
      const startTime = performance.now()
      const response = await fetch(`http://${host}:${port}/api/status`, {
        signal: controller.signal,
        headers: { Accept: 'application/json' },
      })

      if (!response.ok) return null

      const data = await response.json()
      const latency = Math.round(performance.now() - startTime)

      return {
        id: `${host}:${port}`,
        host,
        port,
        version: data.version || 'unknown',
        blockHeight: data.blockHeight || data.block_height,
        networkId: data.networkId || data.network_id || 'botho-mainnet',
        latency,
        status: 'online',
      }
    } catch {
      return null
    } finally {
      clearTimeout(timeoutId)
    }
  }

  private async fetchApi(path: string, options?: RequestInit): Promise<Response> {
    if (!this.connected || !this.currentNode) {
      throw new Error('Not connected to any node')
    }

    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), this.config.timeout)

    try {
      return await fetch(`http://${this.currentNode.host}:${this.currentNode.port}${path}`, {
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

  private setupEventSource(): void {
    if (!this.currentNode) return

    const url = `http://${this.currentNode.host}:${this.currentNode.port}/api/events`
    this.eventSource = new EventSource(url)

    this.eventSource.addEventListener('block', (event) => {
      try {
        const block = this.parseBlock(JSON.parse(event.data))
        this.blockCallbacks.forEach(cb => cb(block))
      } catch {
        // Ignore malformed events
      }
    })

    this.eventSource.addEventListener('transaction', (event) => {
      try {
        const tx = this.parseTransaction(JSON.parse(event.data))
        // Notify relevant subscribers
        this.txCallbacks.forEach((callbacks, key) => {
          const addresses = key.split(',')
          if (tx.counterparty && addresses.includes(tx.counterparty)) {
            callbacks.forEach(cb => cb(tx))
          }
        })
      } catch {
        // Ignore malformed events
      }
    })

    this.eventSource.onerror = () => {
      // Attempt to reconnect after a delay
      if (this.connected) {
        this.eventSource?.close()
        setTimeout(() => {
          if (this.connected) {
            this.setupEventSource()
          }
        }, 5000)
      }
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
