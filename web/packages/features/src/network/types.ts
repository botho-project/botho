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
  /**
   * Consensus tip: the highest height among *connected* nodes (peerCount > 0).
   * Peer-isolated nodes are excluded so a stale/forked singleton cannot define
   * the tip. `null` when nothing reachable.
   */
  consensusHeight: number | null
  /** Connected nodes at the consensus height (within 1 block). */
  nodesInSync: number
  /** Reachable node count. */
  nodesReachable: number
  /**
   * Reachable but peer-isolated nodes (peerCount === 0). Not participating in
   * the mesh — their height is a singleton view (possibly a stale pre-reset
   * fork), so they are excluded from `consensusHeight`/`nodesInSync`.
   */
  nodesIsolated: number
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

/**
 * Latest bridge proof-of-reserves snapshot, `GET /api/metrics/reserve` (#825).
 *
 * Backend source of truth: `ReserveProof` in
 * `infra/faucet/metrics-daemon/src/db.rs` (`#[serde(rename_all = "camelCase")]`).
 *
 * Numeric contract: `lockedReserve`/`ethSupply`/`solSupply`/`totalWrapped` are
 * Rust `u64` picocredits and `drift` is a signed `i64`, all serialized as bare
 * JSON numbers. A full-supply value can exceed `Number.MAX_SAFE_INTEGER`, so we
 * keep them as `number` on the wire type (that's how they arrive) and convert to
 * `bigint` at format time via `BigInt(...)` — never pass `Number` picocredits
 * through `formatBTH`, which takes a `bigint`.
 *
 * Nullable supplies mean the chain was unverified this pass (e.g. the Solana
 * transport is still pending). `null` renders as "unverified", never `0`.
 */
export interface ReserveProof {
  /** BTH locked in the bridge reserve (picocredits). */
  lockedReserve: number
  /** wBTH minted on Ethereum; null = chain unverified this pass. */
  ethSupply: number | null
  /** wBTH minted on Solana; null = transport pending / unverified. */
  solSupply: number | null
  /** ethSupply + solSupply; null if any leg is unverified. */
  totalWrapped: number | null
  /** Signed picocredits: lockedReserve − totalWrapped (can be negative). */
  drift: number
  /** Within the peg tolerance band. */
  inTolerance: boolean
  /** Authoritative red/green source for the peg indicator. */
  pegHealthy: boolean
  /** Unix SECONDS when the snapshot was taken. */
  takenAt: number
}

/**
 * `useReserveProof` state:
 * - `ok`          — 200 with a parsed `ReserveProof`.
 * - `absent`      — 404: daemon has not polled a bridge yet (hide/gray card).
 * - `unavailable` — network error / non-404 non-OK (degrade gracefully, #541).
 */
export type ReserveProofState = 'ok' | 'absent' | 'unavailable'

/** History fetcher the page injects (so components stay backend-agnostic). */
export interface NetworkHistorySource {
  /** Per-node history samples, oldest first. Empty array = no data yet. */
  getHistory(
    node: string,
    resolution: '5min' | 'hourly' | 'daily',
    sinceUnixSeconds: number,
  ): Promise<MetricsHistorySample[]>
}
