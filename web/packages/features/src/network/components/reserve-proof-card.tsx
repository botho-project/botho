import { Card, CardContent } from '@botho/ui'
import { AlertTriangle, Lock, ShieldAlert, ShieldCheck } from 'lucide-react'
import { formatBTHWithSymbol, formatRelativeTime } from '@botho/core'
import type { ReserveProof, ReserveProofState } from '../types'

export interface ReserveProofCardProps {
  /** Latest snapshot; ignored unless `state === 'ok'`. */
  proof: ReserveProof | null
  /** Fetch outcome from `useReserveProof`. */
  state: ReserveProofState
  className?: string
}

/**
 * Proof-of-Reserves panel (#845): BTH locked in the bridge reserve vs total
 * wBTH wrapped across chains, signed drift, and a red/green peg indicator.
 *
 * Pure presentation, mirroring `FleetSummaryStrip`. The peg color is driven
 * SOLELY by `proof.pegHealthy` — the UI never re-derives health.
 *
 * Number contract: reserve/supply figures are `u64` picocredits (possibly
 * beyond `Number.MAX_SAFE_INTEGER`) and `drift` is signed `i64`. We convert to
 * `bigint` via `BigInt(...)` before formatting, never passing `Number`
 * picocredits through the `bigint`-typed `formatBTH*`.
 *
 * `absent` (404 — daemon not polling a bridge yet) renders nothing so the rest
 * of the dashboard is unaffected. `unavailable` (daemon down / non-404 error)
 * renders a grayed placeholder rather than fabricating values (#541 lesson).
 */
export function ReserveProofCard({ proof, state, className }: ReserveProofCardProps) {
  // 404: the daemon has no bridge poll yet — hide the card entirely.
  if (state === 'absent') return null

  if (state === 'unavailable' || proof === null) {
    return (
      <Card className={className}>
        <CardContent className="p-4">
          <div className="flex items-center gap-1.5 text-xs text-[--color-dim]">
            <Lock className="h-4 w-4" />
            Proof of Reserves
          </div>
          <div className="mt-1 text-sm text-[--color-dim]">
            Reserve proof unavailable — the metrics daemon is not reachable.
          </div>
        </CardContent>
      </Card>
    )
  }

  const healthy = proof.pegHealthy
  const totalWrapped =
    proof.totalWrapped === null
      ? 'unverified'
      : formatBTHWithSymbol(BigInt(proof.totalWrapped))
  const ethSupply =
    proof.ethSupply === null ? 'unverified' : formatBTHWithSymbol(BigInt(proof.ethSupply))
  const solSupply =
    proof.solSupply === null
      ? 'unverified (Solana pending)'
      : formatBTHWithSymbol(BigInt(proof.solSupply))

  // Signed drift: branch on the bigint so very large magnitudes are exact.
  const driftPico = BigInt(proof.drift)
  const driftSign = driftPico < 0n ? '−' : '+'
  const driftAbs = driftPico < 0n ? -driftPico : driftPico
  const driftDisplay = `${driftSign}${formatBTHWithSymbol(driftAbs)}`

  return (
    <Card className={className}>
      <CardContent className="p-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-1.5 text-xs text-[--color-dim]">
            <Lock className="h-4 w-4" />
            Proof of Reserves
          </div>
          <PegIndicator healthy={healthy} />
        </div>

        <div className="mt-3 grid grid-cols-2 gap-4 sm:grid-cols-4">
          <Metric label="Locked reserve" value={formatBTHWithSymbol(BigInt(proof.lockedReserve))} />
          <Metric
            label="Total wrapped"
            value={totalWrapped}
            unverified={proof.totalWrapped === null}
          />
          <Metric
            label="Drift"
            value={driftDisplay}
            sub={proof.inTolerance ? 'within tolerance' : 'out of tolerance'}
            warn={!proof.inTolerance}
          />
          <Metric
            label="Snapshot"
            value={formatRelativeTime(proof.takenAt)}
            sub="metrics daemon poll"
          />
        </div>

        <div className="mt-3 grid grid-cols-2 gap-4 text-xs text-[--color-dim] sm:grid-cols-4">
          <div>
            <span className="text-[--color-dim]">Ethereum wBTH</span>
            <div className={proof.ethSupply === null ? 'text-[--color-dim]' : 'text-[--color-light]'}>
              {ethSupply}
            </div>
          </div>
          <div>
            <span className="text-[--color-dim]">Solana wBTH</span>
            <div className={proof.solSupply === null ? 'text-[--color-dim]' : 'text-[--color-light]'}>
              {solSupply}
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
  )
}

/** Red/green peg badge — green iff `healthy`. Never re-derives health. */
function PegIndicator({ healthy }: { healthy: boolean }) {
  if (healthy) {
    return (
      <div className="flex items-center gap-1.5 text-xs font-medium text-emerald-400">
        <ShieldCheck className="h-4 w-4" />
        Peg healthy
      </div>
    )
  }
  return (
    <div className="flex items-center gap-1.5 text-xs font-medium text-[--color-danger]">
      <ShieldAlert className="h-4 w-4" />
      Peg unhealthy
    </div>
  )
}

function Metric({
  label,
  value,
  sub,
  warn,
  unverified,
}: {
  label: string
  value: string
  sub?: string
  warn?: boolean
  unverified?: boolean
}) {
  const valueClass = unverified
    ? 'text-[--color-dim]'
    : warn
      ? 'text-[--color-warning]'
      : 'text-[--color-light]'
  return (
    <div>
      <div className="flex items-center gap-1.5 text-xs text-[--color-dim]">
        {warn && <AlertTriangle className="h-4 w-4 text-[--color-warning]" />}
        {label}
      </div>
      <div className={`mt-1 font-display text-lg font-semibold ${valueClass}`}>{value}</div>
      {sub && <div className="text-xs text-[--color-dim]">{sub}</div>}
    </div>
  )
}
