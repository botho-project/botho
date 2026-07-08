import { Card, CardContent } from '@botho/ui'
import { AlertTriangle, Lock, Server, ShieldAlert, ShieldCheck, WifiOff } from 'lucide-react'
import type { FleetNode } from '../../network/types'
import type {
  NodeTrustStatus,
  OperatorFetchResult,
  OperatorQuorumInfo,
  TrustPeer,
} from '../types'

export interface TrustNodeCardProps {
  node: FleetNode
  /** Trust snapshot; undefined while the first poll is in flight. */
  status?: NodeTrustStatus
  /**
   * Operator-only detail (#707) for this node, present only when a read token
   * is being used. `undefined` ⇒ public read-only view (no operator panel).
   */
  operatorInfo?: OperatorFetchResult<OperatorQuorumInfo>
  className?: string
}

/**
 * One node's quorum-trust posture: promotion-gate counts (#651), BFT posture
 * (#509), and the live peer table (#544).
 *
 * Renders exactly three shapes: loading (first poll pending), unreachable
 * (explicit error card — never stale values), and live posture. Gate fields
 * the node reports as null render as "—" (no gate evaluation yet), never as
 * zero. `quorumGateIntersectionRefused` and `quorumDegenerate` render as
 * prominent warnings.
 */
export function TrustNodeCard({
  node,
  status,
  operatorInfo,
  className,
}: TrustNodeCardProps) {
  if (status && !status.reachable) {
    return (
      <Card className={`border-[--color-danger]/40 ${className ?? ''}`}>
        <CardContent className="p-4">
          <TrustNodeCardHeader node={node} />
          <div className="mt-3 flex items-center gap-2 text-sm text-[--color-danger]">
            <WifiOff className="h-4 w-4 shrink-0" />
            <span>Unreachable</span>
          </div>
          <p className="mt-1 text-xs text-[--color-dim]">
            node_getStatus failed or timed out at{' '}
            {new Date(status.polledAt).toLocaleTimeString()}
          </p>
        </CardContent>
      </Card>
    )
  }

  const warned =
    status?.quorumGateIntersectionRefused === true || status?.quorumDegenerate === true

  return (
    <Card
      className={`${warned ? 'border-[--color-danger]/50' : ''} ${className ?? ''}`}
      data-testid={`trust-card-${node.id}`}
    >
      <CardContent className="p-4">
        <TrustNodeCardHeader node={node} />

        {!status ? (
          <p className="mt-3 text-sm text-[--color-dim]">Checking…</p>
        ) : (
          <>
            <div className="mt-3 flex flex-wrap items-center gap-2 text-xs">
              {status.quorumFaultTolerant === true && (
                <span className="inline-flex items-center gap-1 rounded bg-[--color-pulse]/15 px-1.5 py-0.5 text-[--color-pulse]">
                  <ShieldCheck className="h-3 w-3" /> BFT fault tolerant
                </span>
              )}
              {status.quorumFaultTolerant === false &&
                status.quorumDegenerate !== true && (
                  <span className="inline-flex items-center gap-1 rounded bg-[--color-warning]/15 px-1.5 py-0.5 text-[--color-warning]">
                    <AlertTriangle className="h-3 w-3" /> not fault tolerant
                  </span>
                )}
              {status.quorumDegenerate === true && (
                <span className="inline-flex items-center gap-1 rounded bg-[--color-danger]/15 px-1.5 py-0.5 text-[--color-danger]">
                  <ShieldAlert className="h-3 w-3" /> degenerate quorum — zero fault
                  tolerance
                </span>
              )}
              {status.quorumGateIntersectionRefused === true && (
                <span className="inline-flex items-center gap-1 rounded bg-[--color-danger]/15 px-1.5 py-0.5 text-[--color-danger]">
                  <AlertTriangle className="h-3 w-3" /> intersection check refused last
                  candidate
                </span>
              )}
            </div>

            <div className="mt-3 grid grid-cols-2 gap-x-8 gap-y-1.5 text-sm">
              <Stat label="Curated" value={formatCount(status.quorumCuratedMembers)} />
              <Stat label="Auto-promoted" value={formatCount(status.quorumAutoMembers)} />
              <Stat
                label="Suppressed"
                value={formatCount(status.quorumGateSuppressedPeers)}
                warn={(status.quorumGateSuppressedPeers ?? 0) > 0}
              />
              <Stat label="Auto cap" value={formatCount(status.quorumGateMaxAutoMembers)} />
              <Stat label="SCP peers" value={formatCount(status.scpPeerCount)} />
            </div>

            <PeerTable peers={status.peers} />
            <OperatorDetail info={operatorInfo} />
          </>
        )}
      </CardContent>
    </Card>
  )
}

/**
 * Operator-only detail (#707), rendered ONLY when a read token is in use.
 *
 * Shows the configured `[network.quorum]` members panel and per-peer
 * classification badges (curated / auto / suppressed) — data the public
 * surface deliberately withholds. Degrades explicitly:
 *   - `unauthorized`: the token was rejected — prompt for a fresh link.
 *   - `not-enabled`: this node has no operator surface — say so, don't nag.
 *   - `perPeer` absent: the gate has not evaluated yet ("—", anti-#541).
 */
function OperatorDetail({ info }: { info?: OperatorFetchResult<OperatorQuorumInfo> }) {
  if (!info) return null

  if (info.status === 'unauthorized') {
    return (
      <div className="mt-3 flex items-center gap-2 rounded border border-[--color-warning]/40 bg-[--color-warning]/10 p-2 text-xs text-[--color-warning]">
        <Lock className="h-3.5 w-3.5 shrink-0" />
        Operator link expired or invalid — mint a fresh link to view trust internals.
      </div>
    )
  }
  if (info.status === 'not-enabled') {
    return (
      <p className="mt-3 text-xs text-[--color-dim]">
        Operator reads not enabled on this node.
      </p>
    )
  }
  if (info.status === 'unreachable') {
    return (
      <p className="mt-3 flex items-center gap-2 text-xs text-[--color-danger]">
        <WifiOff className="h-3.5 w-3.5 shrink-0" />
        Operator detail unavailable (operator_getQuorumInfo failed)
      </p>
    )
  }

  const q = info.data
  const pp = q.perPeer
  return (
    <div className="mt-3 rounded border border-[--color-pulse]/30 bg-[--color-pulse]/5 p-2.5">
      <div className="flex items-center gap-1.5 text-xs font-medium text-[--color-pulse]">
        <Lock className="h-3.5 w-3.5 shrink-0" /> Operator detail
      </div>

      <div className="mt-2 grid grid-cols-2 gap-x-6 gap-y-1 text-xs">
        <Stat label="Mode" value={q.mode} />
        <Stat label="Fault model" value={q.faultModel} />
        <Stat label="Threshold" value={q.threshold.toLocaleString()} />
        <Stat label="Min peers" value={q.minPeers.toLocaleString()} />
      </div>

      <div className="mt-2 text-xs text-[--color-dim]">
        Configured members ({q.members.length})
      </div>
      {q.members.length === 0 ? (
        <p className="text-xs text-[--color-dim]">No curated members configured</p>
      ) : (
        <ul className="mt-0.5 space-y-0.5">
          {q.members.map((m) => (
            <li key={m} className="truncate font-mono text-xs text-[--color-light]" title={m}>
              {m}
            </li>
          ))}
        </ul>
      )}

      <div className="mt-2 text-xs text-[--color-dim]">Per-peer classification</div>
      {pp === undefined ? (
        <p className="text-xs text-[--color-dim]">— no gate evaluation yet</p>
      ) : (
        <div className="mt-1 flex flex-wrap gap-1">
          {pp.curated.map((p) => (
            <ClassBadge key={`c-${p}`} peerId={p} kind="curated" />
          ))}
          {pp.auto.map((p) => (
            <ClassBadge key={`a-${p}`} peerId={p} kind="auto" />
          ))}
          {pp.suppressed.map((p) => (
            <ClassBadge key={`s-${p}`} peerId={p} kind="suppressed" />
          ))}
          {pp.curated.length + pp.auto.length + pp.suppressed.length === 0 && (
            <span className="text-xs text-[--color-dim]">No connected peers classified</span>
          )}
        </div>
      )}
    </div>
  )
}

/** A per-peer classification badge, colored by kind. */
function ClassBadge({
  peerId,
  kind,
}: {
  peerId: string
  kind: 'curated' | 'auto' | 'suppressed'
}) {
  const style =
    kind === 'curated'
      ? 'bg-[--color-pulse]/15 text-[--color-pulse]'
      : kind === 'auto'
        ? 'bg-[--color-warning]/15 text-[--color-warning]'
        : 'bg-[--color-danger]/15 text-[--color-danger]'
  const short = peerId.length > 12 ? `${peerId.slice(0, 8)}…${peerId.slice(-4)}` : peerId
  return (
    <span
      className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 font-mono text-[10px] ${style}`}
      title={`${peerId} (${kind})`}
    >
      <span className="uppercase">{kind}</span>
      <span className="text-[--color-light]">{short}</span>
    </span>
  )
}

/** "—" for absent fields — a node that omits a field reported nothing (anti-#541). */
function formatCount(value: number | undefined): string {
  return typeof value === 'number' ? value.toLocaleString() : '—'
}

function TrustNodeCardHeader({ node }: { node: FleetNode }) {
  return (
    <div className="flex min-w-0 items-center gap-2">
      <Server className="h-4 w-4 shrink-0 text-[--color-pulse]" />
      <span className="truncate font-display font-medium text-[--color-light]">
        {node.name}
      </span>
    </div>
  )
}

function Stat({ label, value, warn }: { label: string; value: string; warn?: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="text-[--color-dim]">{label}</span>
      <span
        className={`font-mono ${warn ? 'text-[--color-warning]' : 'text-[--color-light]'}`}
      >
        {value}
      </span>
    </div>
  )
}

/**
 * Live connected-peer table. Three explicit shapes: unavailable (the
 * `network_getPeers` call failed — never an empty table pretending to be
 * "no peers"), genuinely empty, and populated.
 */
function PeerTable({ peers }: { peers?: TrustPeer[] }) {
  if (peers === undefined) {
    return (
      <p className="mt-3 flex items-center gap-2 text-xs text-[--color-danger]">
        <WifiOff className="h-3.5 w-3.5 shrink-0" />
        Peer list unavailable (network_getPeers failed)
      </p>
    )
  }
  if (peers.length === 0) {
    return <p className="mt-3 text-xs text-[--color-dim]">No connected peers</p>
  }
  return (
    <div className="mt-3">
      <div className="text-xs text-[--color-dim]">Connected peers ({peers.length})</div>
      <table className="mt-1 w-full text-xs">
        <thead>
          <tr className="text-left text-[--color-dim]">
            <th className="py-0.5 pr-2 font-normal">Peer</th>
            <th className="py-0.5 pr-2 font-normal">Version</th>
            <th className="py-0.5 text-right font-normal">Seen</th>
          </tr>
        </thead>
        <tbody>
          {peers.map((p) => (
            <tr key={p.peerId} className="border-t border-[--color-slate]/50">
              <td
                className="max-w-0 truncate py-1 pr-2 font-mono text-[--color-light]"
                title={p.address ?? p.peerId}
              >
                {p.peerId}
              </td>
              <td className="whitespace-nowrap py-1 pr-2 font-mono text-[--color-ghost]">
                {p.protocolVersion ?? '—'}
                {p.versionWarning && (
                  <span className="ml-1 inline-flex items-center gap-0.5 rounded bg-[--color-warning]/15 px-1 py-0.5 text-[--color-warning]">
                    <AlertTriangle className="h-3 w-3" /> outdated
                  </span>
                )}
              </td>
              <td className="whitespace-nowrap py-1 text-right font-mono text-[--color-ghost]">
                {formatLastSeen(p.lastSeenSecs)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function formatLastSeen(secs: number | undefined): string {
  if (typeof secs !== 'number') return '—'
  if (secs < 90) return `${secs}s ago`
  if (secs < 5400) return `${Math.round(secs / 60)}m ago`
  return `${(secs / 3600).toFixed(1)}h ago`
}
