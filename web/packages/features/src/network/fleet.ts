import type { FleetNode, FleetNodeStatus, FleetSummary } from './types'

/**
 * Poll one node's `node_getStatus` with a hard timeout.
 *
 * Failures resolve to `reachable: false` — the caller renders an explicit
 * error state. No field is ever carried over from a previous poll.
 */
export async function fetchFleetNodeStatus(
  node: FleetNode,
  timeoutMs = 5000,
): Promise<FleetNodeStatus> {
  const polledAt = Date.now()
  try {
    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), timeoutMs)
    const response = await fetch(node.rpcEndpoint, {
      method: 'POST',
      signal: controller.signal,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', method: 'node_getStatus', params: {}, id: 1 }),
    })
    clearTimeout(timeoutId)
    if (!response.ok) return { nodeId: node.id, reachable: false, polledAt }

    const json = (await response.json()) as {
      result?: {
        chainHeight?: number
        peerCount?: number
        scpPeerCount?: number
        mempoolSize?: number
        mintingActive?: boolean
        nodeVersion?: string
        slotStalled?: boolean
        synced?: boolean
      }
      error?: unknown
    }
    if (json.error || !json.result) return { nodeId: node.id, reachable: false, polledAt }

    const r = json.result
    return {
      nodeId: node.id,
      reachable: true,
      polledAt,
      chainHeight: r.chainHeight,
      peerCount: r.peerCount,
      scpPeerCount: r.scpPeerCount,
      mempoolSize: r.mempoolSize,
      mintingActive: r.mintingActive,
      nodeVersion: r.nodeVersion,
      slotStalled: r.slotStalled,
      synced: r.synced,
    }
  } catch {
    return { nodeId: node.id, reachable: false, polledAt }
  }
}

/** Poll the whole fleet concurrently; one slow/dead node never blocks the rest. */
export async function fetchFleetStatus(nodes: FleetNode[]): Promise<FleetNodeStatus[]> {
  return Promise.all(nodes.map((n) => fetchFleetNodeStatus(n)))
}

/** Derive fleet-level facts from live snapshots. Pure. */
export function deriveFleetSummary(statuses: FleetNodeStatus[]): FleetSummary {
  const reachable = statuses.filter((s) => s.reachable)
  const heights = reachable
    .map((s) => s.chainHeight)
    .filter((h): h is number => typeof h === 'number')
  const consensusHeight = heights.length > 0 ? Math.max(...heights) : null

  return {
    consensusHeight,
    nodesInSync:
      consensusHeight === null
        ? 0
        : reachable.filter(
            (s) => typeof s.chainHeight === 'number' && consensusHeight - s.chainHeight <= 1,
          ).length,
    nodesReachable: reachable.length,
    nodesTotal: statuses.length,
    totalMempool: reachable.reduce((acc, s) => acc + (s.mempoolSize ?? 0), 0),
    anySlotStalled: reachable.some((s) => s.slotStalled === true),
  }
}

/**
 * Average seconds per block over a recent window, from two block timestamps.
 *
 * Returns null when the window is degenerate (fewer than 2 blocks, or a
 * clock-skewed non-positive span) — the UI shows an empty state instead of a
 * fabricated rate.
 */
export function averageBlockSeconds(
  olderBlock: { height: number; timestamp: number },
  newerBlock: { height: number; timestamp: number },
): number | null {
  const blocks = newerBlock.height - olderBlock.height
  const seconds = newerBlock.timestamp - olderBlock.timestamp
  if (blocks <= 0 || seconds <= 0) return null
  return seconds / blocks
}
