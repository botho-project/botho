import { Card, CardContent } from '@botho/ui'
import { ArrowLeftRight } from 'lucide-react'
import { formatBTHWithSymbol } from '@botho/core'
import type {
  BridgeStats,
  BridgeStatsState,
  BridgeStatsWindow,
  Translate,
} from '../../bridge/types'

export interface BridgeActivityCardProps {
  /** Latest aggregate; ignored unless `state === 'ok'`. */
  stats: BridgeStats | null
  /** Fetch outcome from `useBridgeStats`. */
  state: BridgeStatsState
  /**
   * `bridge`-namespace translator supplied by the page (`stats.*` keys), the
   * same injection pattern as the bridge feature components — the network
   * module keeps no dependency on react-i18next.
   */
  t: Translate
  className?: string
}

/**
 * Bridge activity panel (#1054): wrap (BTH → wBTH) and unwrap (wBTH → BTH)
 * counts + settled volumes over 24h and all-time windows, next to the
 * Proof-of-Reserves card on /network.
 *
 * Pure presentation, mirroring `ReserveProofCard`:
 * - `absent` (no public bridge API configured — it is disabled by default on
 *   nodes) renders nothing so the rest of the dashboard is unaffected.
 * - `unavailable` (endpoint configured but unreachable) renders a grayed
 *   placeholder rather than fabricating values (#541 lesson).
 *
 * Number contract: volumes are `u64` picocredit strings (possibly beyond
 * `Number.MAX_SAFE_INTEGER`), converted via `BigInt(...)` before formatting.
 */
export function BridgeActivityCard({ stats, state, t, className }: BridgeActivityCardProps) {
  // No public bridge API on the configured node — hide the card entirely.
  if (state === 'absent') return null

  if (state === 'unavailable' || stats === null) {
    return (
      <Card className={className}>
        <CardContent className="p-4">
          <div className="flex items-center gap-1.5 text-xs text-[--color-dim]">
            <ArrowLeftRight className="h-4 w-4" />
            {t('stats.title')}
          </div>
          <div className="mt-1 text-sm text-[--color-dim]">{t('stats.unavailable')}</div>
        </CardContent>
      </Card>
    )
  }

  return (
    <Card className={className}>
      <CardContent className="p-4">
        <div className="flex items-center gap-1.5 text-xs text-[--color-dim]">
          <ArrowLeftRight className="h-4 w-4" />
          {t('stats.title')}
        </div>

        <div className="mt-3 grid grid-cols-2 gap-4 sm:grid-cols-4">
          <WindowMetric label={t('stats.wraps24h')} win={stats.wraps.last24h} t={t} />
          <WindowMetric label={t('stats.wrapsAllTime')} win={stats.wraps.allTime} t={t} />
          <WindowMetric label={t('stats.unwraps24h')} win={stats.unwraps.last24h} t={t} />
          <WindowMetric label={t('stats.unwrapsAllTime')} win={stats.unwraps.allTime} t={t} />
        </div>
      </CardContent>
    </Card>
  )
}

/**
 * One window cell: the SETTLED volume as the headline (only completed value
 * actually moved across the bridge) with the completed/pending counts as the
 * sub line.
 */
function WindowMetric({
  label,
  win,
  t,
}: {
  label: string
  win: BridgeStatsWindow
  t: Translate
}) {
  return (
    <div>
      <div className="text-xs text-[--color-dim]">{label}</div>
      <div className="mt-1 font-display text-lg font-semibold text-[--color-light]">
        {formatBTHWithSymbol(BigInt(win.completed.volume))}
      </div>
      <div className="text-xs text-[--color-dim]">
        {t('stats.bucketSummary', {
          completed: win.completed.count,
          pending: win.pending.count,
        })}
      </div>
    </div>
  )
}
