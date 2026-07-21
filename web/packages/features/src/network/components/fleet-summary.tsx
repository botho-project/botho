import { Card, CardContent } from '@botho/ui'
import { Activity, AlertTriangle, Blocks, Layers, Timer } from 'lucide-react'
import type { FleetSummary } from '../types'

export interface FleetSummaryStripProps {
  summary: FleetSummary
  /** Average seconds per block over a recent window; null = unknown. */
  avgBlockSeconds: number | null
  className?: string
}

/** Fleet-level facts in one strip above the node grid. */
export function FleetSummaryStrip({
  summary,
  avgBlockSeconds,
  className,
}: FleetSummaryStripProps) {
  return (
    <Card className={className}>
      <CardContent className="grid grid-cols-2 gap-4 p-4 sm:grid-cols-4">
        <Metric
          icon={<Blocks className="h-4 w-4" />}
          label="Consensus height"
          value={summary.consensusHeight?.toLocaleString() ?? '—'}
        />
        <Metric
          icon={
            summary.anySlotStalled ? (
              <AlertTriangle className="h-4 w-4 text-[--color-danger]" />
            ) : (
              <Activity className="h-4 w-4" />
            )
          }
          label="Nodes in sync"
          value={`${summary.nodesInSync}/${summary.nodesTotal}`}
          sub={syncSub(summary)}
          warn={
            summary.nodesInSync < summary.nodesReachable ||
            summary.nodesIsolated > 0 ||
            summary.anySlotStalled
          }
        />
        <Metric
          icon={<Layers className="h-4 w-4" />}
          label="Fleet mempool"
          value={summary.totalMempool.toLocaleString()}
          sub="txs across reachable nodes"
        />
        <Metric
          icon={<Timer className="h-4 w-4" />}
          label="Avg block spacing"
          value={formatSpacing(avgBlockSeconds)}
          sub={
            avgBlockSeconds === null
              ? 'not enough blocks'
              : 'last 20 blocks — testnet mints on demand, idle gaps included'
          }
        />
      </CardContent>
    </Card>
  )
}

/**
 * Sub-label for the "Nodes in sync" metric: call out unreachable and
 * peer-isolated (off-consensus) nodes so a stale/forked relay is visible
 * rather than silently green.
 */
function syncSub(summary: FleetSummary): string | undefined {
  const parts: string[] = []
  const unreachable = summary.nodesTotal - summary.nodesReachable
  if (unreachable > 0) parts.push(`${unreachable} unreachable`)
  if (summary.nodesIsolated > 0) parts.push(`${summary.nodesIsolated} isolated`)
  return parts.length > 0 ? parts.join(', ') : undefined
}

/**
 * Human-readable spacing. The testnet mints on demand, so spacing routinely
 * reaches hours while idle — render h/m rather than a wall of seconds.
 */
function formatSpacing(seconds: number | null): string {
  if (seconds === null) return '—'
  if (seconds < 90) return `${seconds.toFixed(0)}s`
  if (seconds < 5400) return `${(seconds / 60).toFixed(0)}m`
  return `${(seconds / 3600).toFixed(1)}h`
}

function Metric({
  icon,
  label,
  value,
  sub,
  warn,
}: {
  icon: React.ReactNode
  label: string
  value: string
  sub?: string
  warn?: boolean
}) {
  return (
    <div>
      <div className="flex items-center gap-1.5 text-xs text-[--color-dim]">
        {icon}
        {label}
      </div>
      <div
        className={`mt-1 font-display text-xl font-semibold ${
          warn ? 'text-[--color-warning]' : 'text-[--color-light]'
        }`}
      >
        {value}
      </div>
      {sub && <div className="text-xs text-[--color-dim]">{sub}</div>}
    </div>
  )
}
