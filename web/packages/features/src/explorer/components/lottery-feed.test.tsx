/**
 * @vitest-environment jsdom
 *
 * The lottery feed's contract (#699): list blocks with on-chain lottery
 * activity (payouts or pool distribution) newest first, showing aggregates
 * only — never recipients or linkage data — and an explicit empty state.
 */
import { describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import type { Block, BlockLotterySummary } from '@botho/core'
import { LotteryFeed } from './lottery-feed'

const BTH = 10n ** 12n

function lottery(over: Partial<BlockLotterySummary> = {}): BlockLotterySummary {
  return {
    totalFees: 0n,
    poolDistributed: 0n,
    amountBurned: 0n,
    lotterySeed: 'aa'.repeat(32),
    payoutCount: 0,
    payoutTotal: 0n,
    ...over,
  }
}

function block(height: number, over: Partial<Block> = {}): Block {
  return {
    hash: `hash-${height}`,
    height,
    timestamp: Math.floor(Date.now() / 1000) - 60,
    previousHash: `hash-${height - 1}`,
    transactionCount: 1,
    size: 0,
    reward: 0n,
    difficulty: 0n,
    ...over,
  }
}

describe('LotteryFeed', () => {
  it('lists lottery blocks newest first with payout/pool/burn aggregates', () => {
    cleanup()
    render(
      <LotteryFeed
        blocks={[
          block(10, {
            lottery: lottery({
              payoutCount: 2,
              payoutTotal: 3n * BTH,
              poolDistributed: 5n * BTH,
              amountBurned: 1n * BTH,
            }),
          }),
          block(11), // no lottery field (older node) — excluded
          block(12, { lottery: lottery({ poolDistributed: 7n * BTH }) }),
        ]}
      />,
    )
    const heights = screen.getAllByText(/^#\d+$/).map((el) => el.textContent)
    expect(heights).toEqual(['#12', '#10']) // newest first, non-events dropped
    expect(screen.getByText('3.00 BTH')).toBeDefined() // payout total
    expect(screen.getByText('5.00 BTH')).toBeDefined() // pool distributed
    expect(screen.getByText('1.00 BTH')).toBeDefined() // burned
    expect(screen.getByText('7.00 BTH')).toBeDefined()
  })

  it('navigates to the block detail when a row is clicked', () => {
    cleanup()
    const onViewBlock = vi.fn()
    const target = block(20, { lottery: lottery({ payoutCount: 1 }) })
    render(<LotteryFeed blocks={[target]} onViewBlock={onViewBlock} />)
    fireEvent.click(screen.getByText('#20'))
    expect(onViewBlock).toHaveBeenCalledWith(target)
  })

  it('shows an explicit empty state scoped to the loaded window', () => {
    cleanup()
    render(<LotteryFeed blocks={[block(1), block(2, { lottery: lottery() })]} />)
    expect(screen.getByText('No lottery activity')).toBeDefined()
    expect(screen.getByText(/2 most recent loaded blocks/)).toBeDefined()
  })
})
