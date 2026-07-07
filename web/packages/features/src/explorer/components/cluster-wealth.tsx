import { Card, CardHeader, CardTitle, CardContent } from '@botho/ui'
import { Scale, Loader2 } from 'lucide-react'
import { formatBTH } from '@botho/core'
import {
  FACTOR_BANDS,
  bucketClusters,
  formatFactor,
  summarizeWealth,
  type ClusterWealthEntry,
} from '../wealth'

export interface ClusterWealthProps {
  /** Cluster entries; null = not yet loaded. */
  clusters: ClusterWealthEntry[] | null
  /** Fetch in flight. */
  loading: boolean
  /** Fetch error message. */
  error: string | null
  /** False when the data source has no cluster-wealth method. */
  supported?: boolean
  /** Custom class name */
  className?: string
}

const W = 600
const H = 220
const PAD_X = 8
const PAD_TOP = 12
const AXIS_H = 22

/**
 * Cluster-wealth distribution view (#699): log-scale BTH histogram where
 * each bar stacks the node-computed factor bands (floor 1.00x .. ceiling
 * 6.00x), plus summary stats. Dependency-free SVG, matching the network
 * dashboard's hand-rolled chart idiom.
 *
 * Privacy: aggregates only — cluster ids/wealth are already public tracker
 * state; no addresses, balances, or linkage data appear here.
 */
export function ClusterWealth({
  clusters,
  loading,
  error,
  supported = true,
  className,
}: ClusterWealthProps) {
  let body: React.ReactNode

  if (!supported) {
    body = (
      <p className="py-12 text-center text-sm text-[--color-dim]">
        Wealth distribution is unavailable — the connected node does not expose
        cluster-wealth data.
      </p>
    )
  } else if (error) {
    body = (
      <p className="py-12 text-center text-sm text-[--color-danger]">
        Failed to load cluster wealth: {error}
      </p>
    )
  } else if (loading || clusters === null) {
    body = (
      <div className="flex items-center justify-center py-16">
        <Loader2 className="h-8 w-8 animate-spin text-[--color-pulse]" />
      </div>
    )
  } else if (clusters.length === 0) {
    body = (
      <div className="flex flex-col items-center justify-center py-16">
        <Scale className="h-12 w-12 text-[--color-dim]" />
        <p className="mt-4 text-lg text-[--color-ghost]">No clusters tracked yet</p>
        <p className="mt-2 text-sm text-[--color-dim]">
          The wealth distribution appears once the chain has spend activity to cluster.
        </p>
      </div>
    )
  } else {
    const buckets = bucketClusters(clusters)
    const summary = summarizeWealth(clusters)
    const maxTotal = Math.max(...buckets.map((b) => b.total), 1)

    const plotW = W - 2 * PAD_X
    const plotH = H - PAD_TOP - AXIS_H
    const slotW = plotW / buckets.length
    const barW = Math.max(4, slotW * 0.7)

    body = (
      <div className="space-y-4">
        {/* Summary stats */}
        <div className="grid grid-cols-3 gap-4">
          <div>
            <p className="text-xs uppercase tracking-wider text-[--color-dim]">Clusters</p>
            <p className="mt-1 font-mono text-sm text-[--color-light]">
              {summary.clusterCount.toLocaleString()}
            </p>
          </div>
          <div>
            <p className="text-xs uppercase tracking-wider text-[--color-dim]">Total wealth</p>
            <p className="mt-1 font-mono text-sm text-[--color-light]">
              {formatBTH(summary.totalWealth)} BTH
            </p>
          </div>
          <div>
            <p className="text-xs uppercase tracking-wider text-[--color-dim]">Median factor</p>
            <p className="mt-1 font-mono text-sm text-[--color-light]">
              {summary.medianFactor === null ? '—' : formatFactor(summary.medianFactor)}
            </p>
          </div>
        </div>

        {/* Histogram: stacked bars per log-scale BTH bucket, colored by factor band */}
        <svg
          viewBox={`0 0 ${W} ${H}`}
          className="w-full"
          role="img"
          aria-label="Cluster wealth histogram"
        >
          {buckets.map((bucket) => {
            const xCenter = PAD_X + bucket.index * slotW + slotW / 2
            const x = xCenter - barW / 2
            let yCursor = PAD_TOP + plotH
            return (
              <g key={bucket.index}>
                {FACTOR_BANDS.map(({ band, color }) => {
                  const count = bucket.byBand[band]
                  if (count === 0) return null
                  const h = (count / maxTotal) * plotH
                  yCursor -= h
                  return (
                    <rect
                      key={band}
                      x={x}
                      y={yCursor}
                      width={barW}
                      height={h}
                      fill={color}
                    />
                  )
                })}
                {bucket.total > 0 && (
                  <text
                    x={xCenter}
                    y={yCursor - 4}
                    textAnchor="middle"
                    fontSize={10}
                    fill="currentColor"
                    opacity={0.7}
                  >
                    {bucket.total}
                  </text>
                )}
                <text
                  x={xCenter}
                  y={H - 6}
                  textAnchor="middle"
                  fontSize={9}
                  fill="currentColor"
                  opacity={0.5}
                >
                  {bucket.label}
                </text>
              </g>
            )
          })}
        </svg>
        <p className="text-center text-xs text-[--color-dim]">Cluster wealth (BTH, log scale)</p>

        {/* Factor-band legend */}
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-[--color-dim]">
          {FACTOR_BANDS.map(({ band, label, color }) => (
            <span key={band} className="inline-flex items-center gap-1.5">
              <span
                className="inline-block h-2 w-2 rounded-full"
                style={{ backgroundColor: color }}
              />
              {label}
            </span>
          ))}
        </div>
      </div>
    )
  }

  return (
    <Card className={className}>
      <CardHeader>
        <div className="flex items-center gap-2">
          <Scale className="h-4 w-4 text-[--color-pulse]" />
          <CardTitle>Wealth Distribution</CardTitle>
        </div>
      </CardHeader>
      <CardContent>{body}</CardContent>
    </Card>
  )
}
