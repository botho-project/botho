import { useEffect, useState } from 'react'
import type { FleetNode } from '../network/types'
import { fetchTrustStatuses } from './quorum'
import type { NodeTrustStatus } from './types'

/**
 * Polling hook for the operator trust view (#706), mirroring
 * `useFleetStatus` in `../network/hooks.ts`.
 *
 * Callers must pass a referentially-stable `nodes` array (module-level
 * constant) — it is an effect dependency.
 */

export interface UseTrustStatusOptions {
  /** Poll cadence in ms. */
  pollMs?: number
}

export interface UseTrustStatusResult {
  /** Latest trust snapshot per node id; missing key = first poll in flight. */
  statuses: Record<string, NodeTrustStatus>
}

/**
 * Poll every node's trust posture (`node_getStatus` gate fields +
 * `network_getPeers`). Failed polls resolve to explicit `reachable: false`
 * snapshots — never stale values (anti-#541).
 */
export function useTrustStatus(
  nodes: FleetNode[],
  { pollMs = 15_000 }: UseTrustStatusOptions = {},
): UseTrustStatusResult {
  const [statuses, setStatuses] = useState<Record<string, NodeTrustStatus>>({})

  useEffect(() => {
    let cancelled = false
    const poll = async () => {
      const results = await fetchTrustStatuses(nodes)
      if (cancelled) return
      setStatuses(Object.fromEntries(results.map((s) => [s.nodeId, s])))
    }
    poll()
    const id = setInterval(poll, pollMs)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [nodes, pollMs])

  return { statuses }
}
