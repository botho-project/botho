/**
 * @vitest-environment jsdom
 *
 * The recent-blocks list must render honest reward data (#924): a block whose
 * reward is known shows "+<amount>"; a block delivered over the live WebSocket
 * event — whose payload carries no reward — shows "—" rather than a fabricated
 * "+0" (#541-class fabrication).
 */
import { describe, expect, it, vi, afterEach } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import type { Block } from '@botho/core'
import { ExplorerProvider } from '../context'
import { BlockList } from './block-list'

function block(height: number, over: Partial<Block> = {}): Block {
  return {
    hash: `hash-${height}`,
    height,
    timestamp: Math.floor(Date.now() / 1000) - 60,
    previousHash: `hash-${height - 1}`,
    transactionCount: 1,
    size: 128,
    reward: 4_800_000_000_000n,
    difficulty: 0n,
    ...over,
  }
}

function renderList(blocks: Block[]) {
  const dataSource = {
    getRecentBlocks: vi.fn(async () => blocks),
    getBlock: vi.fn(async () => null),
    getTransaction: vi.fn(async () => null),
  }
  return render(
    <ExplorerProvider dataSource={dataSource}>
      <BlockList />
    </ExplorerProvider>,
  )
}

afterEach(() => cleanup())

describe('BlockList reward rendering (#924)', () => {
  it('renders the reward for a block that has one', async () => {
    renderList([block(10)])
    expect(await screen.findByText('+4.80')).toBeTruthy()
  })

  it('renders "—" (not "+0") when a block omits its reward (live WS block)', async () => {
    // A WS-delivered block: reward/size absent, as parseBlockEvent now leaves them.
    renderList([block(11, { reward: undefined, size: undefined })])
    expect(await screen.findByText('—')).toBeTruthy()
    // Must NOT fabricate a "+0" reward.
    expect(screen.queryByText('+0.00')).toBeNull()
    expect(screen.queryByText(/^\+0/)).toBeNull()
  })
})
