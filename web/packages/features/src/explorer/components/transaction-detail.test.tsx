/**
 * @vitest-environment jsdom
 *
 * The transaction-detail view must never fabricate values the node does not
 * expose (#913):
 *   - No "Amount" row (deprecation D1): the node exposes no per-tx amount and
 *     under confidential transactions never will. Only the public fee is shown.
 *   - An absent `timestamp` renders "—", not a fabricated wall-clock time.
 *   - An absent `type` renders a neutral label, not an asserted "receive".
 */
import { describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach } from 'vitest'
import type { Transaction } from '@botho/core'
import { ExplorerProvider } from '../context'
import type { ExplorerDataSource } from '../types'
import { TransactionDetail } from './transaction-detail'

afterEach(cleanup)

const dataSource: ExplorerDataSource = {
  getRecentBlocks: async () => [],
  getBlock: async () => null,
  getTransaction: async () => null,
}

function renderDetail(tx: Transaction) {
  return render(
    <ExplorerProvider dataSource={dataSource}>
      <TransactionDetail transaction={tx} />
    </ExplorerProvider>,
  )
}

/** An explorer-sourced tx: fee is public; direction/amount/timestamp unknown. */
function explorerTx(over: Partial<Transaction> = {}): Transaction {
  return {
    id: 'ab'.repeat(32),
    fee: 1_000_000n,
    privacyLevel: 'private',
    cryptoType: 'clsag',
    status: 'confirmed',
    confirmations: 12,
    ...over,
  }
}

describe('TransactionDetail', () => {
  it('does not render an Amount row (deprecation D1)', () => {
    renderDetail(explorerTx())
    expect(screen.queryByText('Amount')).toBeNull()
  })

  it('still renders the public Fee row', () => {
    renderDetail(explorerTx())
    expect(screen.getByText('Fee')).toBeTruthy()
  })

  it('renders "—" for an absent timestamp instead of fabricating a time', () => {
    renderDetail(explorerTx({ timestamp: undefined }))
    // The Time row's value should be the em dash placeholder.
    const timeLabel = screen.getByText('Time')
    const row = timeLabel.closest('div')?.parentElement ?? timeLabel.parentElement
    expect(row?.textContent).toContain('—')
  })

  it('renders a real time when the timestamp is present', () => {
    renderDetail(explorerTx({ timestamp: 1_751_840_000 }))
    const timeLabel = screen.getByText('Time')
    const row = timeLabel.closest('div')?.parentElement ?? timeLabel.parentElement
    expect(row?.textContent).not.toContain('—')
  })

  it('uses a neutral label when direction is unknown, not "receive"', () => {
    renderDetail(explorerTx({ type: undefined }))
    expect(screen.getByText('Transaction')).toBeTruthy()
    expect(screen.queryByText('receive')).toBeNull()
  })
})
