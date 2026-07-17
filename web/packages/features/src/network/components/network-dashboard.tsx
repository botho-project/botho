import type {
  FleetNode,
  FleetNodeStatus,
  MetricsHistorySample,
  ReserveProof,
  ReserveProofState,
} from '../types'
import type { BridgeStats, BridgeStatsState, Translate } from '../../bridge/types'
import { deriveFleetSummary } from '../fleet'
import { FleetSummaryStrip } from './fleet-summary'
import { NodeCard } from './node-card'
import { HistoryChart } from './history-chart'
import { ReserveProofCard } from './reserve-proof-card'
import { BridgeActivityCard } from './bridge-activity-card'

export interface NetworkDashboardProps {
  nodes: FleetNode[]
  /** Latest live snapshot per node id; missing key = first poll in flight. */
  statuses: Record<string, FleetNodeStatus>
  avgBlockSeconds: number | null
  /** History series per node id (may be empty). */
  history: Record<string, MetricsHistorySample[]>
  historyState: 'ok' | 'empty' | 'unavailable'
  /**
   * Latest bridge proof-of-reserves snapshot (#845). Optional so surfaces that
   * don't wire the reserve hook simply omit the card.
   */
  reserve?: ReserveProof | null
  /** Reserve fetch outcome; `undefined`/`absent` hides the card. */
  reserveState?: ReserveProofState
  /**
   * Aggregate wrap/unwrap activity (#1054). Optional so surfaces that don't
   * wire the bridge-stats hook simply omit the card.
   */
  bridgeStats?: BridgeStats | null
  /** Bridge-stats fetch outcome; `undefined`/`absent` hides the card. */
  bridgeStatsState?: BridgeStatsState
  /**
   * `bridge`-namespace translator for the activity card's strings. Required
   * (alongside `bridgeStatsState`) for the card to render — the features
   * package keeps no react-i18next dependency.
   */
  bridgeStatsT?: Translate
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
  reserve,
  reserveState,
  bridgeStats,
  bridgeStatsState,
  bridgeStatsT,
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

      {reserveState !== undefined && (
        <ReserveProofCard proof={reserve ?? null} state={reserveState} />
      )}

      {bridgeStatsState !== undefined && bridgeStatsT !== undefined && (
        <BridgeActivityCard
          stats={bridgeStats ?? null}
          state={bridgeStatsState}
          t={bridgeStatsT}
        />
      )}

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
