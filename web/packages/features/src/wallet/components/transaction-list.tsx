import type { Transaction } from '@botho/core'
import { Card, CardHeader, CardTitle, CardContent } from '@botho/ui'
import { Clock, Sparkles } from 'lucide-react'
import { TransactionRow } from './transaction-row'

export interface TransactionListProps {
  /** List of transactions */
  transactions: Transaction[]
  /** Title for the card (default: "Transaction History") */
  title?: string
  /** Whether to show privacy badges */
  showPrivacy?: boolean
  /** Whether to show chevrons for clickable rows */
  showChevron?: boolean
  /** Click handler for transaction rows */
  onTransactionClick?: (tx: Transaction) => void
  /** Custom class name */
  className?: string
}

/**
 * Card containing a list of transactions.
 */
export function TransactionList({
  transactions,
  title = 'Transaction History',
  showPrivacy = true,
  showChevron = true,
  onTransactionClick,
  className = '',
}: TransactionListProps) {
  return (
    <Card className={className}>
      <CardHeader>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Clock className="h-4 w-4 text-[--color-pulse]" />
            <CardTitle>{title}</CardTitle>
          </div>
          <span className="text-sm text-[--color-dim]">
            {transactions.length} transaction{transactions.length !== 1 ? 's' : ''}
          </span>
        </div>
      </CardHeader>
      <CardContent>
        {transactions.length === 0 ? (
          <div className="py-12 text-center">
            <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-xl bg-[--color-slate]">
              <Sparkles className="h-6 w-6 text-[--color-dim]" />
            </div>
            <p className="text-[--color-ghost]">No transactions yet</p>
            <p className="mt-1 text-sm text-[--color-dim]">
              Transactions will appear here once you send or receive BTH.
            </p>
          </div>
        ) : (
          <div className="space-y-2">
            {transactions.map((tx, i) => (
              <TransactionRow
                key={tx.id}
                transaction={tx}
                index={i}
                showPrivacy={showPrivacy}
                showChevron={showChevron}
                onClick={onTransactionClick}
              />
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  )
}
