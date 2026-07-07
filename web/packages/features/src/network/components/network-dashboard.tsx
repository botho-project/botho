import type { FleetNode, FleetNodeStatus, MetricsHistorySample } from '../types'
import { deriveFleetSummary } from '../fleet'
import { FleetSummaryStrip } from './fleet-summary'
import { NodeCard } from './node-card'
import { HistoryChart } from './history-chart'

export interface NetworkDashboardProps {
  nodes: FleetNode[]
  /** Latest live snapshot per node id; missing key = first poll in flight. */
  statuses: Record<string, FleetNodeStatus>
  avgBlockSeconds: number | null
  /** History series per node id (may be empty). */
  history: Record<string, MetricsHistorySample[]>
  historyState: 'ok' | 'empty' | 'unavailable'
  className?: string
}

/**
 * The fleet dashboard body: summary strip, per-node live grid, history
 * charts. Pure presentation — polling and history fetching live in the page
 * so this composes into other surfaces (e.g. the P4 admin dashboard, #695).
 */
export function NetworkDashboard({
  nodes,
  statuses,
  avgBlockSeconds,
  history,
  historyState,
  className,
}: NetworkDashboardProps) {
  const statusList = nodes
    .map((n) => statuses[n.id])
    .filter((s): s is FleetNodeStatus => s !== undefined)
  const summary = deriveFleetSummary(
    // Nodes never polled yet don't count as unreachable — only real results.
    statusList,
  )
  // Until at least one poll resolves, show totals over the configured fleet.
  const displaySummary = { ...summary, nodesTotal: nodes.length }

  return (
    <div className={`space-y-4 ${className ?? ''}`}>
      <FleetSummaryStrip summary={displaySummary} avgBlockSeconds={avgBlockSeconds} />

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {nodes.map((node) => (
          <NodeCard
            key={node.id}
            node={node}
            status={statuses[node.id]}
            consensusHeight={displaySummary.consensusHeight}
          />
        ))}
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        <HistoryChart
          title="Chain height"
          series={history}
          metric="height"
          state={historyState}
        />
        <HistoryChart
          title="Mempool depth"
          series={history}
          metric="mempoolSize"
          state={historyState}
        />
      </div>
    </div>
  )
}
