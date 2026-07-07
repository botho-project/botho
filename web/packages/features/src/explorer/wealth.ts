/**
 * Pure derivations for the explorer's cluster-wealth distribution view (#699).
 *
 * All wealth math is BigInt-safe: cluster wealth is a u128 in picocredits
 * (1 BTH = 1e12) and can exceed 2^53, so nothing here routes a raw wealth
 * value through `Number()`. Factors come from the node's live Rust fee curve
 * (milli-x, 1000 = 1.00x .. 6000 = 6.00x) — this module classifies them into
 * display bands but never re-derives the curve (#610 drift class).
 */

/** One cluster's tracked wealth + node-computed fee factor. */
export interface ClusterWealthEntry {
  /** Cluster id as a decimal string (u64 — can exceed 2^53, never parseInt). */
  clusterId: string
  /** Tracked wealth in picocredits (parsed from the wire's string u128). */
  wealth: bigint
  /** Node-supplied milli-x fee-curve multiplier (1000..6000). */
  factor: number
}

/** Picocredits per BTH (12 decimals). */
export const PICO_PER_BTH = 10n ** 12n

// ---------------------------------------------------------------------------
// Factor bands
// ---------------------------------------------------------------------------

export type FactorBand = 'floor' | 'low' | 'mid' | 'high' | 'ceiling'

/** Band order + display metadata (histogram stacking + legend). */
export const FACTOR_BANDS: ReadonlyArray<{
  band: FactorBand
  label: string
  color: string
}> = [
  { band: 'floor', label: '1.00x (floor)', color: '#34d399' },
  { band: 'low', label: '1.01x–1.99x', color: '#06b6d4' },
  { band: 'mid', label: '2.00x–4.99x', color: '#f59e0b' },
  { band: 'high', label: '5.00x–5.99x', color: '#f97316' },
  { band: 'ceiling', label: '6.00x (ceiling)', color: '#ef4444' },
]

/**
 * Classify a node-supplied milli-x factor into its display band.
 *
 * The curve clamps to [1000, 6000] in Rust; out-of-range values (which would
 * indicate a node bug) are clamped into the floor/ceiling bands rather than
 * dropped, so every cluster is always visible in the histogram.
 */
export function factorBand(factor: number): FactorBand {
  if (factor <= 1000) return 'floor'
  if (factor < 2000) return 'low'
  if (factor < 5000) return 'mid'
  if (factor < 6000) return 'high'
  return 'ceiling'
}

/** Format a milli-x factor for display, e.g. 1260 -> "1.26x". */
export function formatFactor(factor: number): string {
  return `${(factor / 1000).toFixed(2)}x`
}

// ---------------------------------------------------------------------------
// Log-scale BTH bucketing
// ---------------------------------------------------------------------------

/**
 * Log10 bucket index for a wealth value (picocredits), in BTH decades:
 * - index 0: < 1 BTH
 * - index k (k >= 1): [10^(k-1), 10^k) BTH
 *
 * Pure BigInt comparisons — safe for the full u128 range.
 */
export function wealthBucketIndex(wealthPico: bigint): number {
  if (wealthPico < PICO_PER_BTH) return 0
  let index = 1
  let upperBound = PICO_PER_BTH * 10n
  while (wealthPico >= upperBound) {
    index++
    upperBound *= 10n
  }
  return index
}

/** Human label for 10^exp BTH, e.g. 0 -> "1", 4 -> "10k", 7 -> "10M". */
function pow10Label(exp: number): string {
  if (exp >= 12) return `1e${exp}`
  if (exp >= 9) return `${10 ** (exp - 9)}B`
  if (exp >= 6) return `${10 ** (exp - 6)}M`
  if (exp >= 3) return `${10 ** (exp - 3)}k`
  return String(10 ** exp)
}

/** Display label (in BTH) for a bucket index. */
export function bucketLabel(index: number): string {
  if (index === 0) return '<1'
  return `${pow10Label(index - 1)}–${pow10Label(index)}`
}

/** One histogram bucket: total cluster count plus per-band breakdown. */
export interface WealthBucket {
  index: number
  /** BTH range label, e.g. "<1", "1–10", "10k–100k". */
  label: string
  total: number
  byBand: Record<FactorBand, number>
}

/**
 * Bucket clusters into contiguous log-scale BTH buckets (empty intermediate
 * buckets included so the histogram axis is honest). Returns [] for an empty
 * input — the component renders the young-chain empty state.
 */
export function bucketClusters(clusters: ClusterWealthEntry[]): WealthBucket[] {
  if (clusters.length === 0) return []

  const indices = clusters.map((c) => wealthBucketIndex(c.wealth))
  const maxIndex = Math.max(...indices)

  const buckets: WealthBucket[] = Array.from({ length: maxIndex + 1 }, (_, index) => ({
    index,
    label: bucketLabel(index),
    total: 0,
    byBand: { floor: 0, low: 0, mid: 0, high: 0, ceiling: 0 },
  }))

  clusters.forEach((cluster, i) => {
    const bucket = buckets[indices[i]]
    bucket.total++
    bucket.byBand[factorBand(cluster.factor)]++
  })

  return buckets
}

// ---------------------------------------------------------------------------
// Summary stats
// ---------------------------------------------------------------------------

export interface WealthSummary {
  clusterCount: number
  /** Exact BigInt sum of all cluster wealth, in picocredits. */
  totalWealth: bigint
  /** Median milli-x factor (average of middle pair for even counts); null when empty. */
  medianFactor: number | null
}

/** Summarize the cluster set. Pure; BigInt-exact total. */
export function summarizeWealth(clusters: ClusterWealthEntry[]): WealthSummary {
  if (clusters.length === 0) {
    return { clusterCount: 0, totalWealth: 0n, medianFactor: null }
  }

  const totalWealth = clusters.reduce((acc, c) => acc + c.wealth, 0n)

  const factors = clusters.map((c) => c.factor).sort((a, b) => a - b)
  const mid = Math.floor(factors.length / 2)
  const medianFactor =
    factors.length % 2 === 1 ? factors[mid] : (factors[mid - 1] + factors[mid]) / 2

  return { clusterCount: clusters.length, totalWealth, medianFactor }
}
