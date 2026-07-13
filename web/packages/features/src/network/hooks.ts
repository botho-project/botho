import { useEffect, useMemo, useRef, useState } from 'react'
import { averageBlockSeconds, fetchFleetNodeStatus } from './fleet'
import type {
  FleetNode,
  FleetNodeStatus,
  MetricsHistorySample,
  ReserveProof,
  ReserveProofState,
} from './types'

/**
 * Reusable polling/history hooks for the fleet dashboard (#706).
 *
 * Extracted from the `/network` page so `/operator` (the P4 dashboard, #695)
 * shares one implementation instead of a forked copy. The hooks own the
 * wiring; `NetworkDashboard` stays pure presentation.
 *
 * Callers must pass a referentially-stable `nodes` array (module-level
 * constant) — it is an effect dependency, so a fresh array per render would
 * restart polling every render.
 */

export interface UseFleetStatusOptions {
  /** Live-grid poll cadence in ms. */
  pollMs?: number
  /** Blocks/hour derivation window (block spacing sample width). */
  blockWindow?: number
}

export interface UseFleetStatusResult {
  /** Latest live snapshot per node id; missing key = first poll in flight. */
  statuses: Record<string, FleetNodeStatus>
  /** Highest height any reachable node reports; null before the first result. */
  consensusHeight: number | null
  /** Average seconds per block over the recent window; null = unknown. */
  avgBlockSeconds: number | null
}

/**
 * Poll every node's `node_getStatus` and derive the average block spacing
 * from two block timestamps on the freshest reachable node.
 *
 * Failed polls resolve to explicit `reachable: false` snapshots (the
 * anti-#541 contract in `fetchFleetNodeStatus`) — never stale values.
 */
export function useFleetStatus(
  nodes: FleetNode[],
  { pollMs = 15_000, blockWindow = 20 }: UseFleetStatusOptions = {},
): UseFleetStatusResult {
  const [statuses, setStatuses] = useState<Record<string, FleetNodeStatus>>({})
  const [avgBlockSecs, setAvgBlockSecs] = useState<number | null>(null)
  // The block-time derivation reuses the freshest reachable node endpoint.
  const bestEndpointRef = useRef<string | null>(null)

  // Live fleet polling.
  useEffect(() => {
    let cancelled = false
    const poll = async () => {
      const results = await Promise.all(nodes.map((n) => fetchFleetNodeStatus(n)))
      if (cancelled) return
      const byId: Record<string, FleetNodeStatus> = {}
      let bestHeight = -1
      for (const [i, s] of results.entries()) {
        byId[s.nodeId] = s
        if (s.reachable && (s.chainHeight ?? -1) > bestHeight) {
          bestHeight = s.chainHeight ?? -1
          bestEndpointRef.current = nodes[i].rpcEndpoint
        }
      }
      setStatuses(byId)
    }
    poll()
    const id = setInterval(poll, pollMs)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [nodes, pollMs])

  // Average block time from two block timestamps on the freshest node.
  const consensusHeight = useMemo(() => {
    const heights = Object.values(statuses)
      .filter((s) => s.reachable && typeof s.chainHeight === 'number')
      .map((s) => s.chainHeight as number)
    return heights.length ? Math.max(...heights) : null
  }, [statuses])

  useEffect(() => {
    const endpoint = bestEndpointRef.current
    if (consensusHeight === null || consensusHeight < 2 || !endpoint) return
    let cancelled = false
    const older = Math.max(1, consensusHeight - blockWindow)
    const fetchBlock = async (height: number) => {
      const r = await fetch(endpoint, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          jsonrpc: '2.0',
          method: 'getBlockByHeight',
          params: { height },
          id: 1,
        }),
      })
      const json = (await r.json()) as {
        result?: { height: number; timestamp: number }
      }
      return json.result ?? null
    }
    Promise.all([fetchBlock(older), fetchBlock(consensusHeight)])
      .then(([a, b]) => {
        if (!cancelled && a && b) setAvgBlockSecs(averageBlockSeconds(a, b))
      })
      .catch(() => {
        /* leave the empty state; live grid is unaffected */
      })
    return () => {
      cancelled = true
    }
  }, [consensusHeight, blockWindow])

  return { statuses, consensusHeight, avgBlockSeconds: avgBlockSecs }
}

export interface UseFleetHistoryOptions {
  /** History refresh cadence in ms (the daemon samples every ~5 min). */
  refreshMs?: number
  /** How far back to fetch, in seconds. */
  windowSeconds?: number
}

export interface UseFleetHistoryResult {
  /** History series per node id (may be empty). */
  history: Record<string, MetricsHistorySample[]>
  historyState: 'ok' | 'empty' | 'unavailable'
}

/**
 * Fetch per-node history from the metrics-daemon fleet API (#697), degrading
 * gracefully to `unavailable` when the backend is absent (it ships/deploys
 * independently).
 */
export function useFleetHistory(
  nodes: FleetNode[],
  metricsApiBase: string,
  { refreshMs = 120_000, windowSeconds = 24 * 3600 }: UseFleetHistoryOptions = {},
): UseFleetHistoryResult {
  const [history, setHistory] = useState<Record<string, MetricsHistorySample[]>>({})
  const [historyState, setHistoryState] = useState<'ok' | 'empty' | 'unavailable'>('empty')

  useEffect(() => {
    let cancelled = false
    const load = async () => {
      try {
        const since = Math.floor(Date.now() / 1000) - windowSeconds
        const results = await Promise.all(
          nodes.map(async (n) => {
            const r = await fetch(
              `${metricsApiBase}/api/metrics/history?node=${encodeURIComponent(n.id)}&resolution=5min&since=${since}`,
            )
            if (!r.ok) throw new Error(`history ${r.status}`)
            return [n.id, (await r.json()) as MetricsHistorySample[]] as const
          }),
        )
        if (cancelled) return
        const byNode = Object.fromEntries(results)
        const any = results.some(([, samples]) => samples.length > 0)
        setHistory(byNode)
        setHistoryState(any ? 'ok' : 'empty')
      } catch {
        if (!cancelled) setHistoryState('unavailable')
      }
    }
    load()
    const id = setInterval(load, refreshMs)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [nodes, metricsApiBase, refreshMs, windowSeconds])

  return { history, historyState }
}

export interface UseReserveProofOptions {
  /** Reserve refresh cadence in ms. The daemon reconciles infrequently, so a
   * slow poll (~2 min, same cadence as history) is right. */
  refreshMs?: number
}

export interface UseReserveProofResult {
  /** Latest snapshot, or null until the first successful poll. */
  proof: ReserveProof | null
  /** Discriminated fetch outcome (see {@link ReserveProofState}). */
  state: ReserveProofState
}

/**
 * Fetch the latest bridge proof-of-reserves snapshot from the metrics-daemon
 * (#825), degrading gracefully when the endpoint is absent or the daemon is
 * down. Mirrors {@link useFleetHistory}'s poll + `cancelled` cleanup structure.
 *
 * The endpoint returns 404 until the daemon has polled a bridge (no
 * `--bridge-url` configured). That is NOT an error — it maps to `absent`, the
 * "hide or gray the card" case. Any other non-OK response or a network error
 * maps to `unavailable` (same graceful path as history-unavailable; never
 * fabricate values — the #541 lesson).
 */
export function useReserveProof(
  metricsApiBase: string,
  { refreshMs = 120_000 }: UseReserveProofOptions = {},
): UseReserveProofResult {
  const [proof, setProof] = useState<ReserveProof | null>(null)
  const [state, setState] = useState<ReserveProofState>('unavailable')

  useEffect(() => {
    let cancelled = false
    const load = async () => {
      try {
        const r = await fetch(`${metricsApiBase}/api/metrics/reserve`)
        // 404 = daemon not polling a bridge yet → hide/gray the card. Check
        // this before the generic non-OK path so it isn't treated as an error.
        if (r.status === 404) {
          if (!cancelled) {
            setProof(null)
            setState('absent')
          }
          return
        }
        if (!r.ok) throw new Error(`reserve ${r.status}`)
        const data = (await r.json()) as ReserveProof
        if (cancelled) return
        setProof(data)
        setState('ok')
      } catch {
        if (!cancelled) setState('unavailable')
      }
    }
    load()
    const id = setInterval(load, refreshMs)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [metricsApiBase, refreshMs])

  return { proof, state }
}
