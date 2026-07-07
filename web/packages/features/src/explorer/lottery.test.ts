import { describe, expect, it } from 'vitest'
import type { Block, BlockLotterySummary } from '@botho/core'
import { selectLotteryBlocks } from './lottery'

function lottery(over: Partial<BlockLotterySummary> = {}): BlockLotterySummary {
  return {
    totalFees: 0n,
    poolDistributed: 0n,
    amountBurned: 0n,
    lotterySeed: '',
    payoutCount: 0,
    payoutTotal: 0n,
    ...over,
  }
}

function block(height: number, over: Partial<Block> = {}): Block {
  return {
    hash: `hash-${height}`,
    height,
    timestamp: 1_751_840_000 + height,
    previousHash: `hash-${height - 1}`,
    transactionCount: 0,
    size: 0,
    reward: 0n,
    difficulty: 0n,
    ...over,
  }
}

describe('selectLotteryBlocks', () => {
  it('keeps blocks with payouts OR pool distribution, newest first', () => {
    const selected = selectLotteryBlocks([
      block(10, { lottery: lottery({ payoutCount: 2, payoutTotal: 100n }) }),
      block(12, { lottery: lottery() }), // zero activity — dropped
      block(11, { lottery: lottery({ poolDistributed: 200n }) }), // pool only — kept
      block(13, { lottery: lottery({ payoutCount: 1 }) }),
    ])
    expect(selected.map((b) => b.height)).toEqual([13, 11, 10])
  })

  it('excludes blocks from older nodes without the lottery field', () => {
    expect(selectLotteryBlocks([block(5), block(6)])).toEqual([])
  })

  it('returns [] for an empty window and never mutates the input', () => {
    expect(selectLotteryBlocks([])).toEqual([])

    const input = [
      block(1, { lottery: lottery({ payoutCount: 1 }) }),
      block(2, { lottery: lottery({ payoutCount: 1 }) }),
    ]
    selectLotteryBlocks(input)
    expect(input.map((b) => b.height)).toEqual([1, 2]) // original order intact
  })

  it('treats a burn-only block (fees burned, nothing distributed) as no activity', () => {
    const burnOnly = block(7, { lottery: lottery({ amountBurned: 50n, totalFees: 50n }) })
    expect(selectLotteryBlocks([burnOnly])).toEqual([])
  })
})
