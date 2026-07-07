import { useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { Logo } from '@botho/ui'
import {
  NetworkDashboard,
  fetchFleetNodeStatus,
  averageBlockSeconds,
  type FleetNode,
  type FleetNodeStatus,
  type MetricsHistorySample,
} from '@botho/features'
import { ArrowLeft } from 'lucide-react'
import { INGRESS_NODES } from '../config/networks'

/** Live-grid poll cadence. */
const POLL_MS = 15_000
/** History refresh cadence (the daemon samples every ~5 min; no need to hammer). */
const HISTORY_MS = 120_000
/** Metrics-daemon fleet API (#697), served via the faucet's nginx. */
const METRICS_API_BASE = 'https://faucet.botho.io/metrics-api'
/** Blocks/hour derivation window. */
const BLOCK_WINDOW = 20

/** The dashboard watches every ingress node plus region labels. */
const FLEET: FleetNode[] = INGRESS_NODES.map((n) => ({
  id: n.id,
  name: n.name,
  rpcEndpoint: n.rpcEndpoint,
}))

export function NetworkPage() {
  const [statuses, setStatuses] = useState<Record<string, FleetNodeStatus>>({})
  const [avgBlockSecs, setAvgBlockSecs] = useState<number | null>(null)
  const [history, setHistory] = useState<Record<string, MetricsHistorySample[]>>({})
  const [historyState, setHistoryState] = useState<'ok' | 'empty' | 'unavailable'>('empty')
  // The block-time derivation reuses the freshest reachable node endpoint.
  const bestEndpointRef = useRef<string | null>(null)

  // Live fleet polling.
  useEffect(() => {
    let cancelled = false
    const poll = async () => {
      const results = await Promise.all(FLEET.map((n) => fetchFleetNodeStatus(n)))
      if (cancelled) return
      const byId: Record<string, FleetNodeStatus> = {}
      let bestHeight = -1
      for (const [i, s] of results.entries()) {
        byId[s.nodeId] = s
        if (s.reachable && (s.chainHeight ?? -1) > bestHeight) {
          bestHeight = s.chainHeight ?? -1
          bestEndpointRef.current = FLEET[i].rpcEndpoint
        }
      }
      setStatuses(byId)
    }
    poll()
    const id = setInterval(poll, POLL_MS)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [])

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
    const older = Math.max(1, consensusHeight - BLOCK_WINDOW)
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
  }, [consensusHeight])

  // History from the metrics-daemon fleet API — degrade gracefully when the
  // backend is absent (it ships/deploys independently, #697).
  useEffect(() => {
    let cancelled = false
    const load = async () => {
      try {
        const since = Math.floor(Date.now() / 1000) - 24 * 3600
        const results = await Promise.all(
          FLEET.map(async (n) => {
            const r = await fetch(
              `${METRICS_API_BASE}/api/metrics/history?node=${encodeURIComponent(n.id)}&resolution=5min&since=${since}`,
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
    const id = setInterval(load, HISTORY_MS)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [])

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">
              Network
            </span>
            <span className="font-display text-base font-semibold sm:hidden">Network</span>
          </Link>
          <Link
            to="/explorer"
            className="text-sm text-ghost hover:text-light transition-colors"
          >
            Block Explorer
          </Link>
        </div>
      </header>

      <main className="py-6 sm:py-8">
        <div className="max-w-6xl mx-auto px-4 sm:px-6">
          <NetworkDashboard
            nodes={FLEET}
            statuses={statuses}
            avgBlockSeconds={avgBlockSecs}
            history={history}
            historyState={historyState}
          />
        </div>
      </main>
    </div>
  )
}
