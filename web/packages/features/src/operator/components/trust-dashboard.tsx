import { AlertTriangle, Lock, ShieldAlert } from 'lucide-react'
import { Card, CardContent } from '@botho/ui'
import type { FleetNode } from '../../network/types'
import type { NodeTrustStatus, OperatorFetchResult, OperatorQuorumInfo } from '../types'
import { deriveTrustSummary } from '../quorum'
import { TrustNodeCard } from './trust-node-card'

export interface TrustDashboardProps {
  nodes: FleetNode[]
  /** Latest trust snapshot per node id; missing key = first poll in flight. */
  statuses: Record<string, NodeTrustStatus>
  /**
   * Operator-only detail per node id (#707), present only when a read token is
   * in use. Absent ⇒ the public read-only view (no operator panels).
   */
  operatorInfo?: Record<string, OperatorFetchResult<OperatorQuorumInfo>>
  /** Fleet-level operator posture, from `useOperatorQuorumInfo`. */
  operatorMode?: 'disabled' | 'active' | 'unauthorized' | 'not-enabled'
  className?: string
}

/**
 * The trust/quorum tab body (#706): fleet-level warning banners + per-node
 * posture cards. Pure presentation — polling lives in `useTrustStatus` so
 * this composes into other surfaces.
 *
 * `quorumGateIntersectionRefused: true` and `quorumDegenerate: true` are
 * surfaced as prominent banners above the grid (the #509 warn-don't-refuse
 * posture): the fleet is still running, but the operator must see it.
 */
export function TrustDashboard({
  nodes,
  statuses,
  operatorInfo,
  operatorMode = 'disabled',
  className,
}: TrustDashboardProps) {
  const statusList = nodes
    .map((n) => statuses[n.id])
    .filter((s): s is NodeTrustStatus => s !== undefined)
  const summary = deriveTrustSummary(statusList)
  const nameOf = (id: string) => nodes.find((n) => n.id === id)?.name ?? id

  return (
    <div className={`space-y-4 ${className ?? ''}`}>
      <OperatorModeBanner mode={operatorMode} />
      {summary.intersectionRefusedNodeIds.length > 0 && (
        <WarningBanner
          icon={<AlertTriangle className="h-5 w-5 shrink-0" />}
          title="Quorum intersection check refused the latest candidate"
          detail={`${summary.intersectionRefusedNodeIds
            .map(nameOf)
            .join(
              ', ',
            )} refused a candidate quorum set that failed the bth-quorum-sim intersection check and kept the previous safe set. Peer churn or curation is proposing an unsafe quorum.`}
        />
      )}
      {summary.degenerateNodeIds.length > 0 && (
        <WarningBanner
          icon={<ShieldAlert className="h-5 w-5 shrink-0" />}
          title="Degenerate quorum — zero fault tolerance"
          detail={`${summary.degenerateNodeIds
            .map(nameOf)
            .join(
              ', ',
            )} report an n-of-n quorum below 4 participating nodes: a single crashed node stalls consensus. The network keeps running (warn, don't refuse — #509), but this posture tolerates ZERO faults.`}
        />
      )}

      {summary.nodesReachable < summary.nodesTotal && statusList.length > 0 && (
        <p className="text-xs text-[--color-dim]">
          {summary.nodesTotal - summary.nodesReachable} of {summary.nodesTotal} nodes
          unreachable — their posture is unknown, not healthy.
        </p>
      )}

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {nodes.map((node) => (
          <TrustNodeCard
            key={node.id}
            node={node}
            status={statuses[node.id]}
            operatorInfo={operatorInfo?.[node.id]}
          />
        ))}
      </div>
    </div>
  )
}

/**
 * A small banner announcing the operator posture (#707): whether the view is
 * the public read-only one, an active operator session, an expired link, or a
 * fleet with no operator surface. `disabled` renders nothing — the public view
 * needs no chrome.
 */
function OperatorModeBanner({
  mode,
}: {
  mode: 'disabled' | 'active' | 'unauthorized' | 'not-enabled'
}) {
  if (mode === 'disabled') return null

  if (mode === 'active') {
    return (
      <div
        className="flex items-center gap-2 rounded border border-[--color-pulse]/40 bg-[--color-pulse]/10 p-2 text-xs text-[--color-pulse]"
        role="status"
      >
        <Lock className="h-3.5 w-3.5 shrink-0" />
        Operator view — showing per-peer classification and configured quorum
        members (read-only).
      </div>
    )
  }
  if (mode === 'unauthorized') {
    return (
      <div
        className="flex items-center gap-2 rounded border border-[--color-warning]/40 bg-[--color-warning]/10 p-2 text-xs text-[--color-warning]"
        role="status"
      >
        <Lock className="h-3.5 w-3.5 shrink-0" />
        Operator link expired or invalid — showing the public read-only view.
        Mint a fresh link with{' '}
        <code className="font-mono">botho operator mint-read-link</code>.
      </div>
    )
  }
  // not-enabled
  return (
    <div
      className="flex items-center gap-2 rounded border border-[--color-slate] p-2 text-xs text-[--color-dim]"
      role="status"
    >
      <Lock className="h-3.5 w-3.5 shrink-0" />
      Operator reads are not enabled on these nodes — showing the public
      read-only view.
    </div>
  )
}

function WarningBanner({
  icon,
  title,
  detail,
}: {
  icon: React.ReactNode
  title: string
  detail: string
}) {
  return (
    <Card className="border-[--color-danger]/60 bg-[--color-danger]/10" role="alert">
      <CardContent className="flex items-start gap-3 p-4 text-[--color-danger]">
        {icon}
        <div>
          <div className="font-display font-semibold">{title}</div>
          <p className="mt-1 text-sm text-[--color-light]">{detail}</p>
        </div>
      </CardContent>
    </Card>
  )
}
