import { describe, expect, it } from 'vitest'
import {
  bucketClusters,
  bucketLabel,
  factorBand,
  formatFactor,
  summarizeWealth,
  wealthBucketIndex,
  PICO_PER_BTH,
  type ClusterWealthEntry,
} from './wealth'

function cluster(wealth: bigint, factor = 1000, clusterId = '1'): ClusterWealthEntry {
  return { clusterId, wealth, factor }
}

describe('wealthBucketIndex', () => {
  it('puts sub-1-BTH wealth (including zero) in bucket 0', () => {
    expect(wealthBucketIndex(0n)).toBe(0)
    expect(wealthBucketIndex(1n)).toBe(0)
    expect(wealthBucketIndex(PICO_PER_BTH - 1n)).toBe(0)
  })

  it('buckets by BTH decade: bucket k = [10^(k-1), 10^k) BTH', () => {
    expect(wealthBucketIndex(PICO_PER_BTH)).toBe(1) // exactly 1 BTH
    expect(wealthBucketIndex(PICO_PER_BTH * 10n - 1n)).toBe(1) // just under 10 BTH
    expect(wealthBucketIndex(PICO_PER_BTH * 10n)).toBe(2) // exactly 10 BTH
    expect(wealthBucketIndex(PICO_PER_BTH * 100_000n)).toBe(6) // 100k BTH (w_mid)
  })

  it('handles values far above 2^53 exactly (BigInt-safe, never Number)', () => {
    // 10^18 BTH = 10^30 picocredits — decades above Number.MAX_SAFE_INTEGER.
    expect(wealthBucketIndex(10n ** 30n)).toBe(19)
    expect(wealthBucketIndex(10n ** 30n - 1n)).toBe(18)
    // u128 max: ~3.4e26 BTH -> bucket 27.
    expect(wealthBucketIndex(340282366920938463463374607431768211455n)).toBe(27)
    // 2^53 picocredits itself (~9007 BTH) lands in the 1k–10k BTH bucket.
    expect(wealthBucketIndex(2n ** 53n)).toBe(4)
  })
})

describe('bucketLabel', () => {
  it('labels the sub-1-BTH bucket and decade ranges', () => {
    expect(bucketLabel(0)).toBe('<1')
    expect(bucketLabel(1)).toBe('1–10')
    expect(bucketLabel(4)).toBe('1k–10k')
    expect(bucketLabel(7)).toBe('1M–10M')
    expect(bucketLabel(10)).toBe('1B–10B')
  })
})

describe('factorBand', () => {
  it('classifies band boundaries exactly per the display spec', () => {
    expect(factorBand(1000)).toBe('floor') // 1.00x exactly
    expect(factorBand(1001)).toBe('low')
    expect(factorBand(1999)).toBe('low')
    expect(factorBand(2000)).toBe('mid')
    expect(factorBand(4999)).toBe('mid')
    expect(factorBand(5000)).toBe('high')
    expect(factorBand(5999)).toBe('high')
    expect(factorBand(6000)).toBe('ceiling') // 6.00x exactly
  })

  it('clamps out-of-range factors into the edge bands (node-bug defense)', () => {
    expect(factorBand(999)).toBe('floor')
    expect(factorBand(6001)).toBe('ceiling')
  })
})

describe('formatFactor', () => {
  it('renders milli-x factors as 2-decimal multipliers', () => {
    expect(formatFactor(1000)).toBe('1.00x')
    expect(formatFactor(1260)).toBe('1.26x')
    expect(formatFactor(6000)).toBe('6.00x')
  })
})

describe('bucketClusters', () => {
  it('returns [] for an empty input (young chain -> empty state)', () => {
    expect(bucketClusters([])).toEqual([])
  })

  it('produces contiguous buckets with per-band counts', () => {
    const buckets = bucketClusters([
      cluster(0n, 1000), // bucket 0, floor
      cluster(PICO_PER_BTH * 5n, 1200), // bucket 1, low
      cluster(PICO_PER_BTH * 5n, 1000), // bucket 1, floor
      cluster(PICO_PER_BTH * 5000n, 3500), // bucket 4, mid
    ])
    expect(buckets).toHaveLength(5) // indices 0..4, gaps included
    expect(buckets.map((b) => b.total)).toEqual([1, 2, 0, 0, 1])
    expect(buckets[1].byBand.low).toBe(1)
    expect(buckets[1].byBand.floor).toBe(1)
    expect(buckets[4].byBand.mid).toBe(1)
    expect(buckets[2].total).toBe(0) // empty intermediate bucket kept
    expect(buckets[0].label).toBe('<1')
    expect(buckets[4].label).toBe('1k–10k')
  })

  it('buckets a > 2^53 whale exactly', () => {
    const whale = cluster(340282366920938463463374607431768211454n, 6000)
    const buckets = bucketClusters([whale])
    expect(buckets).toHaveLength(28)
    expect(buckets[27].total).toBe(1)
    expect(buckets[27].byBand.ceiling).toBe(1)
  })
})

describe('summarizeWealth', () => {
  it('returns zeros and a null median for an empty set', () => {
    expect(summarizeWealth([])).toEqual({
      clusterCount: 0,
      totalWealth: 0n,
      medianFactor: null,
    })
  })

  it('sums total wealth exactly beyond 2^53 (BigInt, no precision loss)', () => {
    const a = 2n ** 60n
    const b = 2n ** 60n + 1n
    const summary = summarizeWealth([cluster(a, 1000), cluster(b, 2000)])
    expect(summary.totalWealth).toBe(a + b)
    expect(summary.totalWealth.toString()).toBe('2305843009213693953')
    expect(summary.clusterCount).toBe(2)
  })

  it('takes the middle factor for odd counts and the middle-pair average for even', () => {
    expect(
      summarizeWealth([cluster(0n, 3000), cluster(0n, 1000), cluster(0n, 6000)]).medianFactor,
    ).toBe(3000)
    expect(
      summarizeWealth([cluster(0n, 1000), cluster(0n, 2000)]).medianFactor,
    ).toBe(1500)
  })
})
