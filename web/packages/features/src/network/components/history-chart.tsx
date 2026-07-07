import { Card, CardContent, CardHeader, CardTitle } from '@botho/ui'
import { TrendingUp } from 'lucide-react'
import type { MetricsHistorySample } from '../types'

export interface HistoryChartProps {
  title: string
  /** Series keyed by node id; each series oldest-first. */
  series: Record<string, MetricsHistorySample[]>
  /** Which sample field to plot. */
  metric: 'height' | 'mempoolSize' | 'peerCount'
  /**
   * Backend state: 'ok' renders the chart, 'empty' the no-data-yet state,
   * 'unavailable' the degraded state (metrics API unreachable — the live
   * grid above still works, so this is informational, not an error page).
   */
  state: 'ok' | 'empty' | 'unavailable'
  className?: string
}

/** Distinct line colors per node (cycled). */
const LINE_COLORS = ['#06b6d4', '#a78bfa', '#f59e0b', '#34d399', '#f472b6']

const W = 600
const H = 160
const PAD = 8

/**
 * Dependency-free multi-series SVG line chart.
 *
 * Deliberately hand-rolled: the wallet has no chart library, and one metric
 * line per node doesn't justify adding one. Scales are shared across series
 * so per-node divergence (e.g. a stale height) is visible.
 */
export function HistoryChart({ title, series, metric, state, className }: HistoryChartProps) {
  const entries = Object.entries(series).filter(([, samples]) => samples.length > 0)

  let body: React.ReactNode
  if (state === 'unavailable') {
    body = (
      <p className="py-8 text-center text-sm text-[--color-dim]">
        History unavailable — the metrics API could not be reached. Live status above is
        unaffected.
      </p>
    )
  } else if (state === 'empty' || entries.length === 0) {
    body = (
      <p className="py-8 text-center text-sm text-[--color-dim]">
        No history yet — samples appear as the metrics daemon collects them.
      </p>
    )
  } else {
    const allSamples = entries.flatMap(([, s]) => s)
    const tMin = Math.min(...allSamples.map((s) => s.timestamp))
    const tMax = Math.max(...allSamples.map((s) => s.timestamp))
    const vMin = Math.min(...allSamples.map((s) => s[metric]))
    const vMax = Math.max(...allSamples.map((s) => s[metric]))
    const tSpan = Math.max(1, tMax - tMin)
    const vSpan = Math.max(1, vMax - vMin)

    const x = (t: number) => PAD + ((t - tMin) / tSpan) * (W - 2 * PAD)
    const y = (v: number) => H - PAD - ((v - vMin) / vSpan) * (H - 2 * PAD)

    body = (
      <>
        <svg
          viewBox={`0 0 ${W} ${H}`}
          className="w-full"
          role="img"
          aria-label={`${title} chart`}
        >
          {entries.map(([nodeId, samples], i) => (
            <polyline
              key={nodeId}
              fill="none"
              stroke={LINE_COLORS[i % LINE_COLORS.length]}
              strokeWidth={1.5}
              points={samples.map((s) => `${x(s.timestamp)},${y(s[metric])}`).join(' ')}
            />
          ))}
        </svg>
        <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-[--color-dim]">
          {entries.map(([nodeId], i) => (
            <span key={nodeId} className="inline-flex items-center gap-1.5">
              <span
                className="inline-block h-2 w-2 rounded-full"
                style={{ backgroundColor: LINE_COLORS[i % LINE_COLORS.length] }}
              />
              {nodeId}
            </span>
          ))}
          <span className="ml-auto font-mono">
            {vMin.toLocaleString()} – {vMax.toLocaleString()}
          </span>
        </div>
      </>
    )
  }

  return (
    <Card className={className}>
      <CardHeader>
        <div className="flex items-center gap-2">
          <TrendingUp className="h-4 w-4 text-[--color-pulse]" />
          <CardTitle>{title}</CardTitle>
        </div>
      </CardHeader>
      <CardContent>{body}</CardContent>
    </Card>
  )
}
