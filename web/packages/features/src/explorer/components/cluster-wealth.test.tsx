/**
 * @vitest-environment jsdom
 *
 * The wealth view's contract (#699): a histogram of node-supplied factors
 * (never a TS re-derivation of the curve), explicit empty state for a young
 * chain, and explicit unavailable/error states — no fabricated values.
 */
import { describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { ClusterWealth } from './cluster-wealth'
import type { ClusterWealthEntry } from '../wealth'

const BTH = 10n ** 12n

function entry(wealth: bigint, factor: number, clusterId: string): ClusterWealthEntry {
  return { clusterId, wealth, factor }
}

function renderWealth(props: Partial<Parameters<typeof ClusterWealth>[0]> = {}) {
  cleanup()
  return render(
    <ClusterWealth clusters={[]} loading={false} error={null} supported {...props} />,
  )
}

describe('ClusterWealth', () => {
  it('renders the histogram, summary stats, and factor-band legend', () => {
    renderWealth({
      clusters: [
        entry(50n * BTH, 1000, '1'),
        entry(50n * BTH, 1000, '2'),
        entry(100_000n * BTH, 2500, '3'),
      ],
    })
    expect(screen.getByRole('img', { name: 'Cluster wealth histogram' })).toBeDefined()
    // Summary stats: 3 clusters, exact BigInt total (100,100 BTH), median 1.00x.
    expect(screen.getByText('Clusters')).toBeDefined()
    expect(screen.getByText('3')).toBeDefined()
    expect(screen.getByText('100,100.00 BTH')).toBeDefined()
    expect(screen.getByText('Median factor')).toBeDefined()
    expect(screen.getByText('1.00x')).toBeDefined()
    // Band legend renders every band label.
    expect(screen.getByText('1.00x (floor)')).toBeDefined()
    expect(screen.getByText('2.00x–4.99x')).toBeDefined()
    expect(screen.getByText('6.00x (ceiling)')).toBeDefined()
  })

  it('summarizes a > 2^53 whale without precision loss', () => {
    // 10^20 picocredits = 100,000,000 BTH — beyond Number.MAX_SAFE_INTEGER.
    renderWealth({ clusters: [entry(10n ** 20n, 6000, 'whale')] })
    expect(screen.getByText('100,000,000.00 BTH')).toBeDefined()
    expect(screen.getByText('6.00x')).toBeDefined()
  })

  it('shows the young-chain empty state for zero clusters', () => {
    renderWealth({ clusters: [] })
    expect(screen.getByText('No clusters tracked yet')).toBeDefined()
    expect(screen.queryByRole('img', { name: 'Cluster wealth histogram' })).toBeNull()
  })

  it('shows a loading state until the first fetch resolves', () => {
    const { container } = renderWealth({ clusters: null, loading: true })
    expect(container.querySelector('.animate-spin')).not.toBeNull()
  })

  it('degrades to an explicit unavailable state when the data source lacks the RPC', () => {
    renderWealth({ clusters: null, supported: false })
    expect(screen.getByText(/Wealth distribution is unavailable/)).toBeDefined()
  })

  it('renders the fetch error explicitly — never stale data', () => {
    renderWealth({ clusters: null, error: 'node timed out' })
    expect(screen.getByText(/Failed to load cluster wealth: node timed out/)).toBeDefined()
  })
})
