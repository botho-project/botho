/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, beforeEach } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'
import type { Transaction } from '@botho/core'
import { TransactionList } from './transaction-list'

const SENDER = 'btho1qaliceaddress0000000000000000000000000000000000aaaa'

function makeTx(overrides: Partial<Transaction> = {}): Transaction {
  return {
    id: 'tx-1',
    type: 'send',
    amount: 1_500_000_000_000n,
    fee: 4000n,
    privacyLevel: 'private',
    cryptoType: 'clsag',
    status: 'confirmed',
    timestamp: Math.floor(Date.now() / 1000),
    confirmations: 12,
    counterparty: SENDER,
    ...overrides,
  }
}

describe('TransactionList', () => {
  beforeEach(() => cleanup())

  it('renders a friendly empty state when there are no transactions', () => {
    render(<TransactionList transactions={[]} />)
    expect(screen.getByText('No transactions yet')).toBeDefined()
    // The hint text is present too.
    expect(screen.getByText(/Transactions will appear here/i)).toBeDefined()
    // No rows rendered.
    expect(document.querySelector('.space-y-2')).toBeNull()
  })

  it('shows the truncated address when no name resolver is provided', () => {
    render(<TransactionList transactions={[makeTx()]} showChevron={false} />)
    expect(screen.getByText(SENDER)).toBeDefined()
  })

  it('shows a resolved contact name instead of the raw address', () => {
    const resolveName = (addr: string) => (addr === SENDER ? 'Alice' : undefined)
    render(
      <TransactionList
        transactions={[makeTx()]}
        resolveName={resolveName}
        showChevron={false}
      />
    )
    expect(screen.getByText('Alice')).toBeDefined()
    // Raw address no longer shown as the label.
    expect(screen.queryByText(SENDER)).toBeNull()
  })

  it('falls back to the raw address when the resolver returns undefined', () => {
    const resolveName = () => undefined
    render(
      <TransactionList
        transactions={[makeTx()]}
        resolveName={resolveName}
        showChevron={false}
      />
    )
    expect(screen.getByText(SENDER)).toBeDefined()
  })

  it('renders a pending indicator for in-flight sends', () => {
    render(
      <TransactionList
        transactions={[makeTx({ status: 'pending', confirmations: 0 })]}
        showChevron={false}
      />
    )
    expect(screen.getByText('Pending')).toBeDefined()
  })

  it('renders a confirming indicator for shallow confirmations', () => {
    render(
      <TransactionList
        transactions={[makeTx({ status: 'confirmed', confirmations: 1 })]}
        showChevron={false}
      />
    )
    expect(screen.getByText('Confirming')).toBeDefined()
  })

  it('renders a confirmation count for settled transactions', () => {
    render(
      <TransactionList
        transactions={[makeTx({ status: 'confirmed', confirmations: 12 })]}
        showChevron={false}
      />
    )
    expect(screen.getByText('12 conf')).toBeDefined()
  })

  it('shows an absolute time tooltip on the relative timestamp', () => {
    const ts = 1_700_000_000
    const { container } = render(
      <TransactionList transactions={[makeTx({ timestamp: ts })]} showChevron={false} />
    )
    const titled = container.querySelector(
      `[title="${new Date(ts * 1000).toLocaleString()}"]`
    )
    expect(titled).not.toBeNull()
  })

  it('reports the transaction count in the header', () => {
    render(<TransactionList transactions={[makeTx(), makeTx({ id: 'tx-2' })]} />)
    expect(screen.getByText('2 transactions')).toBeDefined()
  })
})
