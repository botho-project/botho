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
  ClusterWealthEntry,
  FeeEstimate,
  MempoolUpdate,
  NodeAdapter,
  PeerStatus,
  RemoteNodeConfig,
  TxHistoryOptions,
  TxSubmitResult,
  WsConnectionStatus,
} from './types'

const DEFAULT_CONFIG: Required<RemoteNodeConfig> = {
  // Seed node read RPC (CORS-enabled, see infra/seed/seed-nginx.conf).
  seedNodes: ['https://seed.botho.io/rpc'],
  networkId: 'botho-testnet',
  timeout: 10000,
  useWebSocket: true,
}

/**
 * Resolve an RPC endpoint (which may be absolute, e.g.
 * "https://seed.botho.io/rpc", or relative, e.g. "/rpc" when served behind a
 * same-origin proxy) into a URL object. Returns null if it cannot be parsed.
 */
function resolveUrl(endpoint: string): URL | null {
  try {
    return new URL(endpoint)
  } catch {
    // Relative endpoint - resolve against the page origin if available.
    if (typeof window !== 'undefined' && window.location?.origin) {
      try {
        return new URL(endpoint, window.location.origin)
      } catch {
        return null
      }
    }
    return null
  }
}

/**
 * Decode a little-endian hex string (the node's `amountCommitment`, which is
 * `u64::to_le_bytes` hex-encoded) into a bigint amount.
 */
function leHexToBigInt(hex: string): bigint {
  let result = 0n
  // Each byte is two hex chars; iterate from the most-significant byte (end of
  // the string for little-endian) down to the least.
  for (let i = hex.length - 2; i >= 0; i -= 2) {
    result = (result << 8n) | BigInt(parseInt(hex.slice(i, i + 2), 16))
  }
  return result
}

/**
 * Wire shape of `getBlockByHeight` / `getBlockByHash` results. The
 * `transactions` / `totalFees` / `lottery` fields are the additive explorer
 * enrichment from #700 — older nodes omit them, so they are all optional and
 * the mapping below guards with undefined checks.
 */
interface RpcBlockResult {
  height: number
  hash: string
  prevHash: string
  timestamp: number
  difficulty: number
  txCount: number
  mintingReward: number
  transactions?: Array<{ hash: string; fee: number; ringSize: number }>
  totalFees?: number
  lottery?: {
    totalFees?: number
    poolDistributed?: number
    amountBurned?: number
    lotterySeed?: string
    payoutCount?: number
    payoutTotal?: number
  }
}

/**
 * Map an RPC block result to the core `Block` shape. Fee/lottery amounts go
 * through `BigInt(String(...))` (never bare `Number()` arithmetic) so future
 * wide values cannot be silently coerced. Enriched fields stay `undefined`
 * when an older node omits them (#699 additive contract).
 */
function mapRpcBlock(result: RpcBlockResult): Block {
  const block: Block = {
    hash: result.hash,
    height: result.height,
    timestamp: result.timestamp,
    previousHash: result.prevHash,
    transactionCount: result.txCount,
    size: 0, // Not provided
    reward: BigInt(result.mintingReward || 0),
    difficulty: BigInt(result.difficulty || 0),
  }

  if (Array.isArray(result.transactions)) {
    block.transactions = result.transactions.map((tx) => ({
      hash: tx.hash,
      fee: BigInt(String(tx.fee ?? 0)),
      ringSize: tx.ringSize ?? 0,
    }))
  }
  if (result.totalFees !== undefined) {
    block.totalFees = BigInt(String(result.totalFees))
  }
  if (result.lottery !== undefined) {
    block.lottery = {
      totalFees: BigInt(String(result.lottery.totalFees ?? 0)),
      poolDistributed: BigInt(String(result.lottery.poolDistributed ?? 0)),
      amountBurned: BigInt(String(result.lottery.amountBurned ?? 0)),
      lotterySeed: result.lottery.lotterySeed ?? '',
      payoutCount: result.lottery.payoutCount ?? 0,
      payoutTotal: BigInt(String(result.lottery.payoutTotal ?? 0)),
    }
  }

  return block
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
          const resolvedUrl = resolveUrl(seedUrl)
          this.currentNode = {
            id: seedUrl,
            host: resolvedUrl?.hostname ?? seedUrl,
            port: resolvedUrl ? Number(resolvedUrl.port) || (resolvedUrl.protocol === 'http:' ? 80 : 443) : 443,
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
        const result = await this.call<RpcBlockResult>('getBlockByHeight', {
          height: heightOrHash,
        })
        return mapRpcBlock(result)
      } else {
        // Resolve by hash via the node's getBlockByHash RPC (issue #330).
        const result = await this.call<RpcBlockResult>('getBlockByHash', {
          hash: heightOrHash,
        })
        return mapRpcBlock(result)
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

  /**
   * @deprecated The adapter has no wallet keys, so it cannot tell which on-chain
   * outputs the user owns or decode their amounts — its previous implementation
   * mapped EVERY chain output to a `{ type: 'receive', amount: 0n }` entry,
   * producing dozens of bogus "received 0 BTH" rows (#459). Transaction history
   * is now built CLIENT-SIDE from the wallet's OWNED outputs in the wallet
   * context (see `buildOwnedHistory`, which scans via the wasm signer exactly
   * like balance does). This method is retained only so the `NodeAdapter`
   * interface stays satisfied; it now returns an empty list rather than spam.
   */
  async getTransactionHistory(_addresses: Address[], _options?: TxHistoryOptions): Promise<Transaction[]> {
    return []
  }

  /**
   * Fetch raw chain outputs for a height range as
   * `{ targetKey, publicKey, amount }`.
   *
   * Unlike {@link getTransactionHistory} (which maps to the explorer's
   * `Transaction` shape and drops amounts), this returns the data the
   * client-side transaction builder needs: the stealth keys plus the
   * transparent amount recovered from the output's commitment (the node sends
   * the amount as little-endian bytes in `amountCommitment`).
   */
  async getRawOutputs(
    startHeight: number,
    endHeight: number,
  ): Promise<Array<{ targetKey: string; publicKey: string; amount: bigint }>> {
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
      start_height: startHeight,
      end_height: endHeight,
    })

    return result.flatMap((block) =>
      block.outputs.map((output) => ({
        targetKey: output.targetKey,
        publicKey: output.publicKey,
        amount: leHexToBigInt(output.amountCommitment),
      })),
    )
  }

  /**
   * Like {@link getRawOutputs}, but preserves each output's block height (and
   * its source transaction hash). The thin wallet uses this to build its
   * transaction history CLIENT-SIDE: scan these for owned outputs (wasm), then
   * stamp each owned receive with the block height it landed in (#459). Kept
   * separate from {@link getRawOutputs} so the hot send/balance path stays a
   * flat `{ targetKey, publicKey, amount }[]`.
   */
  async getRawOutputsWithMeta(
    startHeight: number,
    endHeight: number,
  ): Promise<
    Array<{
      txHash: string
      height: number
      targetKey: string
      publicKey: string
      amount: bigint
    }>
  > {
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
      start_height: startHeight,
      end_height: endHeight,
    })

    return result.flatMap((block) =>
      block.outputs.map((output) => ({
        txHash: output.txHash,
        height: block.height,
        targetKey: output.targetKey,
        publicKey: output.publicKey,
        amount: leHexToBigInt(output.amountCommitment),
      })),
    )
  }

  /**
   * Query the node's `chain_areKeyImagesSpent` RPC: for each supplied
   * hex-encoded key image, report whether it is spent on-chain or pending in
   * the mempool. The thin wallet uses this (with key images derived client-side
   * by the wasm signer) to exclude already-spent owned outputs from its balance
   * and from spendable-input selection — the node's `wallet_getBalance` only
   * spent-filters the node's OWN wallet, not arbitrary thin-wallet keys (#392).
   */
  async areKeyImagesSpent(
    keyImages: string[],
  ): Promise<
    Array<{
      keyImage: string
      spent: boolean
      spentHeight: number | null
      pending: boolean
    }>
  > {
    return this.call<
      Array<{
        keyImage: string
        spent: boolean
        spentHeight: number | null
        pending: boolean
      }>
    >('chain_areKeyImagesSpent', { keyImages })
  }

  async getTransaction(txHash: TxHash): Promise<Transaction | null> {
    try {
      // The node's getTransaction RPC returns a "Transaction not found" error
      // for unknown hashes, which rpcCall surfaces as a throw -> null here.
      const result = await this.call<{
        txHash: string
        status: 'confirmed' | 'pending' | 'unknown'
        blockHeight: number | null
        confirmations: number
        inMempool: boolean
        type?: string
        fee?: number
      }>('getTransaction', { tx_hash: txHash })

      // Map RPC signature type to the explorer CryptoType.
      let cryptoType: CryptoType = 'clsag'
      if (result.type === 'minting') {
        cryptoType = 'minting'
      } else if (result.type === 'hybrid') {
        cryptoType = 'hybrid'
      }

      const status: Transaction['status'] = result.status === 'confirmed' ? 'confirmed' : 'pending'

      return {
        id: result.txHash,
        type: 'receive' as const,
        amount: BigInt(0), // Private (ring signatures) - amount not visible
        fee: BigInt(result.fee || 0),
        privacyLevel: 'private' as const,
        cryptoType,
        status,
        timestamp: Date.now(), // RPC does not expose tx timestamp; block timestamp would require an extra lookup
        blockHeight: result.blockHeight ?? undefined,
        confirmations: result.confirmations || 0,
      }
    } catch {
      return null
    }
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

  async estimateFee(_sizeBytes: number, clusterWealth?: bigint): Promise<FeeEstimate> {
    // Forward the sender's cluster wealth so the node applies the progressive
    // fee factor (#626/#628). It is a string-encoded u128 on the wire — never a
    // JS number — so the full u128 range survives without precision loss. When
    // omitted, the node returns the 1.00x base-rate fee (matches local.ts).
    const params: Record<string, unknown> = {
      amount: 0,
      private: true,
      memos: 0,
    }
    if (clusterWealth !== undefined) {
      params.cluster_wealth = clusterWealth.toString()
    }

    const result = await this.call<{
      // The node serializes these fees as JSON numbers (u64), but we parse them
      // through BigInt(String(...)) so a future wide value cannot be silently
      // coerced/rounded by Number().
      minimumFee: number | string
      recommendedFee: number | string
      // Node-computed display string for the cluster fee factor, e.g. "1.85x"
      // (#635). Server-side from the live log-domain curve — never hardcoded
      // client-side. Absent on older nodes, in which case we fall back to
      // "1.00x" (base rate).
      clusterFactorDisplay?: string
    }>('estimateFee', params)

    return {
      fee: BigInt(String(result.recommendedFee ?? result.minimumFee ?? 0)),
      clusterFactorDisplay: result.clusterFactorDisplay ?? '1.00x',
    }
  }

  /**
   * Look up the sender's cluster wealth (string-encoded u128) for a set of owned
   * output target keys, so {@link estimateFee} can be given a real cluster
   * wealth and the node applies the correct progressive fee factor (#634).
   *
   * The node's `cluster_getWealthByTargetKeys` returns `max_cluster_wealth` as a
   * decimal string (u128, #628) — it MUST flow through `BigInt()`, never
   * `Number()`, since cluster wealth can exceed `Number.MAX_SAFE_INTEGER`.
   * Returns `0n` for an empty target-key list (fresh/empty wallet).
   */
  async getClusterWealth(targetKeys: string[]): Promise<bigint> {
    if (targetKeys.length === 0) return 0n
    const result = await this.call<{
      max_cluster_wealth: string
      cluster_factor: number
      total_value: number
    }>('cluster_getWealthByTargetKeys', { target_keys: targetKeys })
    return BigInt(result.max_cluster_wealth || '0')
  }

  /**
   * Fetch every tracked cluster's wealth + live fee-curve factor via the
   * node's `cluster_getAllWealth` RPC (#699/#700), for the explorer's
   * wealth-distribution histogram.
   *
   * - `wealth` is a string-encoded u128 (#628) — parsed with `BigInt()`,
   *   never `Number()` (precision loss above 2^53).
   * - `factor` is the node-computed milli-x multiplier (1000..6000) from the
   *   live Rust fee curve; older nodes omit it and we default to the 1000
   *   floor rather than re-deriving the curve client-side (#610 drift class).
   * - `cluster_id` stays a string — real ids exceed `Number.MAX_SAFE_INTEGER`.
   */
  async getAllClusterWealth(): Promise<ClusterWealthEntry[]> {
    const result = await this.call<{
      count: number
      total_tracked_wealth: string
      clusters?: Array<{ cluster_id: string; wealth: string; factor?: number }>
    }>('cluster_getAllWealth', {})

    return (result.clusters ?? []).map((cluster) => ({
      clusterId: cluster.cluster_id,
      wealth: BigInt(cluster.wealth || '0'),
      factor: cluster.factor ?? 1000,
    }))
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
    // Connect to node WebSocket endpoint for real-time events.
    // Handle both absolute (https://host/rpc) and relative (/rpc) endpoints.
    const resolved = resolveUrl(seedUrl)
    if (!resolved) {
      // Cannot derive a WebSocket URL; fall back to polling.
      this.setWsStatus('disconnected')
      return
    }
    const wsProtocol = resolved.protocol === 'https:' ? 'wss:' : 'ws:'
    const wsUrl = `${wsProtocol}//${resolved.host}${resolved.pathname}/ws`

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
    if (rpcType === 'minting') {
      cryptoType = 'minting'
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
