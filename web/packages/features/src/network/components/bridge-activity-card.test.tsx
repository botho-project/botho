/**
 * @vitest-environment jsdom
 *
 * Bridge activity card contract (#1054): renders wrap/unwrap settled volumes
 * + counts per window when stats are live; an `absent` state (no public
 * bridge API configured — the default for nodes) renders NOTHING; an
 * `unavailable` state (configured but unreachable) degrades to a grayed
 * placeholder without fabricating values (#541 lesson).
 */
import { describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { BridgeActivityCard } from './bridge-activity-card'
import type { BridgeStats, BridgeStatsWindow, Translate } from '../../bridge/types'

// Passthrough translator: returns the key (plus interpolations for the
// summary line) so assertions are locale-independent.
const t: Translate = (key, options) => {
  if (key === 'stats.bucketSummary' && options) {
    return `${String(options.completed)} completed · ${String(options.pending)} pending`
  }
  return key
}

function mkWindow(over: Partial<BridgeStatsWindow> = {}): BridgeStatsWindow {
  const zero = { count: 0, volume: '0' }
  return { completed: zero, pending: zero, expired: zero, failed: zero, ...over }
}

function stats(): BridgeStats {
  return {
    generatedAt: Math.floor(Date.now() / 1000),
    wraps: {
      // 5 BTH settled today across 2 wraps, 1 more in flight.
      last24h: mkWindow({
        completed: { count: 2, volume: '5000000000000' },
        pending: { count: 1, volume: '1000000000000' },
      }),
      // 123 BTH settled all-time.
      allTime: mkWindow({ completed: { count: 7, volume: '123000000000000' } }),
    },
    unwraps: {
      last24h: mkWindow({ completed: { count: 1, volume: '2000000000000' } }),
      allTime: mkWindow({ completed: { count: 3, volume: '9000000000000' } }),
    },
  }
}

describe('BridgeActivityCard', () => {
  it('renders settled volumes and counts for all four windows', () => {
    cleanup()
    render(<BridgeActivityCard stats={stats()} state="ok" t={t} />)

    expect(screen.getByText('stats.title')).toBeDefined()
    expect(screen.getByText('stats.wraps24h')).toBeDefined()
    expect(screen.getByText('stats.wrapsAllTime')).toBeDefined()
    expect(screen.getByText('stats.unwraps24h')).toBeDefined()
    expect(screen.getByText('stats.unwrapsAllTime')).toBeDefined()

    // Volumes are u64 picocredit strings formatted via BigInt.
    expect(screen.getByText(/5\.00 BTH/)).toBeDefined()
    expect(screen.getByText(/123\.00 BTH/)).toBeDefined()
    expect(screen.getByText(/2\.00 BTH/)).toBeDefined()
    expect(screen.getByText(/9\.00 BTH/)).toBeDefined()

    // Counts surface in the per-window summary line.
    expect(screen.getByText('2 completed · 1 pending')).toBeDefined()
    expect(screen.getByText('7 completed · 0 pending')).toBeDefined()
  })

  it('renders NOTHING when the public bridge API is not configured (absent)', () => {
    cleanup()
    const { container } = render(<BridgeActivityCard stats={null} state="absent" t={t} />)
    expect(container.innerHTML).toBe('')
  })

  it('degrades to an unavailable placeholder without fabricating values', () => {
    cleanup()
    render(<BridgeActivityCard stats={null} state="unavailable" t={t} />)
    expect(screen.getByText('stats.title')).toBeDefined()
    expect(screen.getByText('stats.unavailable')).toBeDefined()
    // No fabricated zero volumes.
    expect(screen.queryByText(/0\.00 BTH/)).toBeNull()
    expect(screen.queryByText('stats.wraps24h')).toBeNull()
  })

  it('treats ok-with-null-stats as unavailable (never renders empty metrics)', () => {
    cleanup()
    render(<BridgeActivityCard stats={null} state="ok" t={t} />)
    expect(screen.getByText('stats.unavailable')).toBeDefined()
  })
})
