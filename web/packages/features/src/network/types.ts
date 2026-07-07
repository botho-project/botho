/**
 * Network fleet dashboard types (#698).
 *
 * The dashboard has two data planes:
 * - LIVE: the browser polls every ingress node's `node_getStatus` directly
 *   (all five testnet nodes serve TLS + CORS since #636).
 * - HISTORY: the metrics-daemon fleet API (#697) serves per-node samples
 *   with 5min/hourly/daily rollups.
 */

/** A node the dashboard watches (mirrors the wallet's IngressNode). */
export interface FleetNode {
  /** Stable id (e.g. 'seed', 'eu'). */
  id: string
  /** Display name (e.g. 'EU seed (Frankfurt)'). */
  name: string
  /** Absolute JSON-RPC endpoint. */
  rpcEndpoint: string
}

/**
 * Live status snapshot for one node, from `node_getStatus`.
 *
 * `reachable: false` means the poll failed (timeout / network / RPC error) —
 * the card must render an explicit error state, never stale or fabricated
 * values (the #541 hardcoded-observability lesson).
 */
export interface FleetNodeStatus {
  nodeId: string
  reachable: boolean
  /** Unix millis when this snapshot was taken. */
  polledAt: number
  chainHeight?: number
  peerCount?: number
  scpPeerCount?: number
  mempoolSize?: number
  mintingActive?: boolean
  nodeVersion?: string
  /** SCP slot-stall verdict (#653); undefined when the node predates it. */
  slotStalled?: boolean
  synced?: boolean
}

/** Fleet-level facts derived from the live snapshots (pure function). */
export interface FleetSummary {
  /** Highest height any reachable node reports (consensus tip). */
  consensusHeight: number | null
  /** Reachable nodes at the consensus height (within 1 block). */
  nodesInSync: number
  /** Reachable node count. */
  nodesReachable: number
  /** Total nodes watched. */
  nodesTotal: number
  /** Sum of mempool sizes across reachable nodes. */
  totalMempool: number
  /** True when any reachable node reports a stalled SCP slot. */
  anySlotStalled: boolean
}

// ---------------------------------------------------------------------------
// Metrics-daemon fleet API (#697 contract)
// ---------------------------------------------------------------------------

/** One entry of `GET /api/metrics/latest`. */
export interface MetricsLatestEntry {
  node: string
  timestamp: number
  height: number
  peerCount: number
  scpPeerCount: number
  mempoolSize: number
  mintingActive: boolean
  uptimeSeconds: number
  heightStale: boolean
}

/** One sample of `GET /api/metrics/history`. */
export interface MetricsHistorySample {
  timestamp: number
  height: number
  peerCount: number
  mempoolSize: number
}

/** History fetcher the page injects (so components stay backend-agnostic). */
export interface NetworkHistorySource {
  /** Per-node history samples, oldest first. Empty array = no data yet. */
  getHistory(
    node: string,
    resolution: '5min' | 'hourly' | 'daily',
    sinceUnixSeconds: number,
  ): Promise<MetricsHistorySample[]>
}
