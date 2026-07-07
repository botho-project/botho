import { Card, CardContent } from '@botho/ui'
import { AlertTriangle, Pickaxe, Server, WifiOff } from 'lucide-react'
import type { FleetNode, FleetNodeStatus } from '../types'

export interface NodeCardProps {
  node: FleetNode
  /** Live snapshot; undefined while the first poll is in flight. */
  status?: FleetNodeStatus
  /** Fleet consensus height, for lag highlighting. */
  consensusHeight: number | null
  className?: string
}

/**
 * One fleet node's live status.
 *
 * Renders exactly three shapes: loading (first poll pending), unreachable
 * (explicit error card — never stale values), and live stats. A node more
 * than 1 block behind the fleet's consensus height gets a lag highlight; a
 * stalled SCP slot gets a warning row.
 */
export function NodeCard({ node, status, consensusHeight, className }: NodeCardProps) {
  if (status && !status.reachable) {
    return (
      <Card className={`border-[--color-danger]/40 ${className ?? ''}`}>
        <CardContent className="p-4">
          <NodeCardHeader node={node} />
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

  const height = status?.chainHeight
  const lagging =
    consensusHeight !== null && typeof height === 'number' && consensusHeight - height > 1

  return (
    <Card
      className={`${lagging ? 'border-[--color-warning]/50' : ''} ${className ?? ''}`}
      data-testid={`node-card-${node.id}`}
    >
      <CardContent className="p-4">
        <NodeCardHeader node={node} version={status?.nodeVersion} />

        {!status ? (
          <p className="mt-3 text-sm text-[--color-dim]">Checking…</p>
        ) : (
          <>
            <div className="mt-3 grid grid-cols-2 gap-x-8 gap-y-1.5 text-sm">
              <Stat
                label="Height"
                value={height?.toLocaleString() ?? '—'}
                warn={lagging}
              />
              <Stat label="Mempool" value={status.mempoolSize?.toLocaleString() ?? '—'} />
              <Stat label="Peers" value={status.peerCount?.toLocaleString() ?? '—'} />
              <Stat label="SCP peers" value={status.scpPeerCount?.toLocaleString() ?? '—'} />
            </div>

            <div className="mt-3 flex flex-wrap items-center gap-2 text-xs">
              {status.mintingActive && (
                <span className="inline-flex items-center gap-1 rounded bg-[--color-pulse]/15 px-1.5 py-0.5 text-[--color-pulse]">
                  <Pickaxe className="h-3 w-3" /> minting
                </span>
              )}
              {lagging && (
                <span className="inline-flex items-center gap-1 rounded bg-[--color-warning]/15 px-1.5 py-0.5 text-[--color-warning]">
                  <AlertTriangle className="h-3 w-3" />
                  {consensusHeight! - height!} blocks behind
                </span>
              )}
              {status.slotStalled === true && (
                <span className="inline-flex items-center gap-1 rounded bg-[--color-danger]/15 px-1.5 py-0.5 text-[--color-danger]">
                  <AlertTriangle className="h-3 w-3" /> SCP slot stalled
                </span>
              )}
            </div>
          </>
        )}
      </CardContent>
    </Card>
  )
}

function NodeCardHeader({ node, version }: { node: FleetNode; version?: string }) {
  return (
    <div className="flex items-center justify-between gap-2">
      <div className="flex min-w-0 items-center gap-2">
        <Server className="h-4 w-4 shrink-0 text-[--color-pulse]" />
        <span className="truncate font-display font-medium text-[--color-light]">
          {node.name}
        </span>
      </div>
      {version && (
        <span className="shrink-0 rounded bg-[--color-slate] px-1.5 py-0.5 font-mono text-xs text-[--color-ghost]">
          v{version}
        </span>
      )}
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
