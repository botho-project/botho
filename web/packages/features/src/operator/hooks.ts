import { useEffect, useState } from 'react'
import type { FleetNode } from '../network/types'
import { fetchOperatorQuorumInfo, fetchTrustStatuses } from './quorum'
import type { NodeTrustStatus, OperatorFetchResult, OperatorQuorumInfo } from './types'

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

export interface UseOperatorQuorumInfoResult {
  /** Per-node operator fetch result; missing key = first poll in flight. */
  info: Record<string, OperatorFetchResult<OperatorQuorumInfo>>
  /**
   * Fleet-level operator posture, derived from the per-node results:
   *   - `disabled`: no token is present, so we never call the operator RPCs.
   *   - `active`: at least one node returned operator data for the token.
   *   - `unauthorized`: a token is present but every reachable node rejected
   *     it (expired / wrong secret) — prompt for a fresh link.
   *   - `not-enabled`: a token is present but no node exposes the operator
   *     surface — nothing to unlock here.
   */
  mode: 'disabled' | 'active' | 'unauthorized' | 'not-enabled'
}

/**
 * Poll every node's operator-only `operator_getQuorumInfo` using `token`
 * (#707). When `token` is falsy the hook does nothing and reports
 * `mode: 'disabled'` — the dashboard then renders the public read-only view.
 *
 * Callers must pass a referentially-stable `nodes` array (module-level
 * constant) — it is an effect dependency.
 */
export function useOperatorQuorumInfo(
  nodes: FleetNode[],
  token: string | null,
  { pollMs = 30_000 }: UseTrustStatusOptions = {},
): UseOperatorQuorumInfoResult {
  const [info, setInfo] = useState<Record<string, OperatorFetchResult<OperatorQuorumInfo>>>({})

  useEffect(() => {
    if (!token) {
      setInfo({})
      return
    }
    let cancelled = false
    const poll = async () => {
      const results = await Promise.all(
        nodes.map(async (n) => [n.id, await fetchOperatorQuorumInfo(n, token)] as const),
      )
      if (cancelled) return
      setInfo(Object.fromEntries(results))
    }
    poll()
    const id = setInterval(poll, pollMs)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [nodes, token, pollMs])

  const results = Object.values(info)
  let mode: UseOperatorQuorumInfoResult['mode']
  if (!token) {
    mode = 'disabled'
  } else if (results.some((r) => r.status === 'ok')) {
    mode = 'active'
  } else if (results.length > 0 && results.every((r) => r.status === 'not-enabled')) {
    mode = 'not-enabled'
  } else if (results.some((r) => r.status === 'unauthorized')) {
    mode = 'unauthorized'
  } else {
    // A token is present but no result has landed yet (first poll in flight) or
    // every node was unreachable — treat as active-pending so the UI shows the
    // operator affordance rather than flashing the public view.
    mode = 'active'
  }

  return { info, mode }
}
