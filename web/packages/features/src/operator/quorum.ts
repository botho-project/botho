import type { FleetNode } from '../network/types'
import type {
  NodeTrustStatus,
  OperatorFetchResult,
  OperatorQuorumInfo,
  PerPeerClassification,
  TrustPeer,
  TrustSummary,
} from './types'

/** Node RPC error codes for the operator surface (`botho/src/rpc/mod.rs`). */
const OPERATOR_NOT_ENABLED = -32020
const OPERATOR_TOKEN_REJECTED = -32021

/**
 * Fetch + derive for the operator trust view (#706).
 *
 * Same anti-#541 contract as `../network/fleet.ts` (`fetchFleetNodeStatus`):
 * hard timeout, failures resolve to explicit error states, wire `null`s map
 * to `undefined` (absent), never to fabricated zeros.
 */

/**
 * One JSON-RPC call that resolves to the `result` object, or `null` on ANY
 * failure (transport, HTTP, RPC-level error, malformed body). Never throws.
 */
async function rpcResult(
  endpoint: string,
  method: string,
  signal: AbortSignal,
): Promise<Record<string, unknown> | null> {
  try {
    const response = await fetch(endpoint, {
      method: 'POST',
      signal,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', method, params: {}, id: 1 }),
    })
    if (!response.ok) return null
    const json = (await response.json()) as {
      result?: Record<string, unknown>
      error?: unknown
    }
    if (json.error || !json.result || typeof json.result !== 'object') return null
    return json.result
  } catch {
    return null
  }
}

/** Wire value -> field: numbers pass through, `null`/anything else is absent. */
function asNumber(v: unknown): number | undefined {
  return typeof v === 'number' ? v : undefined
}

/** Wire value -> field: booleans pass through, `null`/anything else is absent. */
function asBoolean(v: unknown): boolean | undefined {
  return typeof v === 'boolean' ? v : undefined
}

/**
 * Parse a `network_getPeers` result into peer rows.
 *
 * Returns `undefined` when the call failed or the shape is unrecognized —
 * the UI renders an explicit "peer list unavailable" state, never an empty
 * table masquerading as "no peers".
 */
function parsePeers(result: Record<string, unknown> | null): TrustPeer[] | undefined {
  if (!result || !Array.isArray(result.peers)) return undefined
  const peers: TrustPeer[] = []
  for (const raw of result.peers as unknown[]) {
    if (typeof raw !== 'object' || raw === null) continue
    const p = raw as Record<string, unknown>
    if (typeof p.peerId !== 'string') continue
    peers.push({
      peerId: p.peerId,
      address: typeof p.address === 'string' ? p.address : null,
      protocolVersion: typeof p.protocolVersion === 'string' ? p.protocolVersion : null,
      versionWarning: p.versionWarning === true,
      lastSeenSecs: asNumber(p.lastSeenSecs),
    })
  }
  return peers
}

/**
 * Poll one node's trust posture with a hard timeout: `node_getStatus` gate
 * fields merged with the `network_getPeers` peer table (both public).
 *
 * - `node_getStatus` failure ⇒ `reachable: false` (explicit error card).
 * - `network_getPeers` failure alone ⇒ reachable snapshot with
 *   `peers: undefined` (explicit "unavailable" table state).
 */
export async function fetchNodeTrustStatus(
  node: FleetNode,
  timeoutMs = 5000,
): Promise<NodeTrustStatus> {
  const polledAt = Date.now()
  const controller = new AbortController()
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const [status, peersResult] = await Promise.all([
      rpcResult(node.rpcEndpoint, 'node_getStatus', controller.signal),
      rpcResult(node.rpcEndpoint, 'network_getPeers', controller.signal),
    ])
    if (!status) return { nodeId: node.id, reachable: false, polledAt }

    return {
      nodeId: node.id,
      reachable: true,
      polledAt,
      quorumFaultTolerant: asBoolean(status.quorumFaultTolerant),
      quorumDegenerate: asBoolean(status.quorumDegenerate),
      quorumCuratedMembers: asNumber(status.quorumCuratedMembers),
      quorumAutoMembers: asNumber(status.quorumAutoMembers),
      quorumGateSuppressedPeers: asNumber(status.quorumGateSuppressedPeers),
      quorumGateMaxAutoMembers: asNumber(status.quorumGateMaxAutoMembers),
      quorumGateIntersectionRefused: asBoolean(status.quorumGateIntersectionRefused),
      scpPeerCount: asNumber(status.scpPeerCount),
      peers: parsePeers(peersResult),
    }
  } finally {
    clearTimeout(timeoutId)
  }
}

/** Poll the whole fleet concurrently; one slow/dead node never blocks the rest. */
export async function fetchTrustStatuses(nodes: FleetNode[]): Promise<NodeTrustStatus[]> {
  return Promise.all(nodes.map((n) => fetchNodeTrustStatus(n)))
}

/** Parse a string array field, dropping non-strings. `null`/absent ⇒ undefined. */
function asStringArray(v: unknown): string[] | undefined {
  if (!Array.isArray(v)) return undefined
  return v.filter((x): x is string => typeof x === 'string')
}

/**
 * Parse the `perPeer` object from `operator_getQuorumInfo`. The node reports
 * `null` until the first gate evaluation — we map that (and any malformed
 * shape) to `undefined`, so the UI shows "no evaluation yet" rather than an
 * empty classification (anti-#541).
 */
function parsePerPeer(v: unknown): PerPeerClassification | undefined {
  if (!v || typeof v !== 'object') return undefined
  const o = v as Record<string, unknown>
  const curated = asStringArray(o.curated)
  const auto = asStringArray(o.auto)
  const suppressed = asStringArray(o.suppressed)
  if (curated === undefined || auto === undefined || suppressed === undefined) return undefined
  return { curated, auto, suppressed }
}

/**
 * Fetch the operator-only `operator_getQuorumInfo` for one node with a hard
 * timeout. Distinguishes not-enabled / unauthorized / unreachable so the UI
 * degrades correctly (see [`OperatorFetchResult`]).
 *
 * The token is passed straight through as a param; the NODE verifies it. An
 * empty/absent token still calls the node, which rejects it — so the
 * `unauthorized` path is exercised identically whether the token is missing or
 * simply wrong.
 */
export async function fetchOperatorQuorumInfo(
  node: FleetNode,
  token: string | null,
  timeoutMs = 5000,
): Promise<OperatorFetchResult<OperatorQuorumInfo>> {
  const controller = new AbortController()
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const response = await fetch(node.rpcEndpoint, {
      method: 'POST',
      signal: controller.signal,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method: 'operator_getQuorumInfo',
        params: { token: token ?? '' },
        id: 1,
      }),
    })
    if (!response.ok) return { status: 'unreachable' }
    const json = (await response.json()) as {
      result?: Record<string, unknown>
      error?: { code?: number }
    }
    if (json.error) {
      if (json.error.code === OPERATOR_NOT_ENABLED) return { status: 'not-enabled' }
      if (json.error.code === OPERATOR_TOKEN_REJECTED) return { status: 'unauthorized' }
      return { status: 'unreachable' }
    }
    const result = json.result
    if (!result || typeof result !== 'object') return { status: 'unreachable' }
    const quorum = result.quorum as Record<string, unknown> | undefined
    if (!quorum || typeof quorum !== 'object') return { status: 'unreachable' }

    return {
      status: 'ok',
      data: {
        mode: typeof quorum.mode === 'string' ? quorum.mode : 'unknown',
        faultModel: typeof quorum.faultModel === 'string' ? quorum.faultModel : 'unknown',
        threshold: asNumber(quorum.threshold) ?? 0,
        members: asStringArray(quorum.members) ?? [],
        minPeers: asNumber(quorum.minPeers) ?? 0,
        maxAutoMembers: asNumber(quorum.maxAutoMembers) ?? 0,
        perPeer: parsePerPeer(result.perPeer),
      },
    }
  } catch {
    return { status: 'unreachable' }
  } finally {
    clearTimeout(timeoutId)
  }
}

/**
 * Derive fleet-level trust facts from live snapshots. Pure.
 *
 * Only reachable nodes contribute: an unreachable node's last-known posture
 * is unknown, not "healthy" and not "warning" (anti-#541).
 */
export function deriveTrustSummary(statuses: NodeTrustStatus[]): TrustSummary {
  const reachable = statuses.filter((s) => s.reachable)
  return {
    nodesReachable: reachable.length,
    nodesTotal: statuses.length,
    intersectionRefusedNodeIds: reachable
      .filter((s) => s.quorumGateIntersectionRefused === true)
      .map((s) => s.nodeId),
    degenerateNodeIds: reachable
      .filter((s) => s.quorumDegenerate === true)
      .map((s) => s.nodeId),
    faultTolerantCount: reachable.filter((s) => s.quorumFaultTolerant === true).length,
  }
}
