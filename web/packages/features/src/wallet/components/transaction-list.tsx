import type { CryptoType, Transaction } from '@botho/core'
import { Card, CardHeader, CardTitle, CardContent } from '@botho/ui'
import { Clock, Filter, Sparkles } from 'lucide-react'
import { useState, useMemo } from 'react'
import { TransactionRow } from './transaction-row'

export type CryptoTypeFilter = CryptoType | 'all'

export interface TransactionListProps {
  /** List of transactions */
  transactions: Transaction[]
  /** Title for the card (default: "Transaction History") */
  title?: string
  /** Whether to show privacy badges */
  showPrivacy?: boolean
  /** Whether to show chevrons for clickable rows */
  showChevron?: boolean
  /** Whether to show the crypto type filter */
  showFilter?: boolean
  /** Click handler for transaction rows */
  onTransactionClick?: (tx: Transaction) => void
  /** Custom class name */
  className?: string
}

const filterOptions: { value: CryptoTypeFilter; label: string }[] = [
  { value: 'all', label: 'All Types' },
  { value: 'clsag', label: 'Private (CLSAG)' },
  { value: 'mldsa', label: 'Minting (ML-DSA)' },
  { value: 'hybrid', label: 'Hybrid' },
]

/**
 * Card containing a list of transactions with optional filtering by crypto type.
 */
export function TransactionList({
  transactions,
  title = 'Transaction History',
  showPrivacy = true,
  showChevron = true,
  showFilter = true,
  onTransactionClick,
  className = '',
}: TransactionListProps) {
  const [cryptoTypeFilter, setCryptoTypeFilter] = useState<CryptoTypeFilter>('all')
  const [isFilterOpen, setIsFilterOpen] = useState(false)

  const filteredTransactions = useMemo(() => {
    if (cryptoTypeFilter === 'all') {
      return transactions
    }
    return transactions.filter((tx) => tx.cryptoType === cryptoTypeFilter)
  }, [transactions, cryptoTypeFilter])

  const selectedLabel = filterOptions.find((opt) => opt.value === cryptoTypeFilter)?.label || 'All Types'

  return (
    <Card className={className}>
      <CardHeader>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Clock className="h-4 w-4 text-[--color-pulse]" />
            <CardTitle>{title}</CardTitle>
          </div>
          <div className="flex items-center gap-3">
            {showFilter && (
              <div className="relative">
                <button
                  onClick={() => setIsFilterOpen(!isFilterOpen)}
                  className="flex items-center gap-2 rounded-lg border border-[--color-steel] bg-[--color-slate] px-3 py-1.5 text-sm text-[--color-ghost] transition-colors hover:border-[--color-pulse] hover:text-[--color-light]"
                >
                  <Filter className="h-3.5 w-3.5" />
                  {selectedLabel}
                </button>
                {isFilterOpen && (
                  <>
                    <div
                      className="fixed inset-0 z-40"
                      onClick={() => setIsFilterOpen(false)}
                    />
                    <div className="absolute right-0 top-full z-50 mt-1 w-48 rounded-lg border border-[--color-steel] bg-[--color-void] py-1 shadow-xl">
                      {filterOptions.map((option) => (
                        <button
                          key={option.value}
                          onClick={() => {
                            setCryptoTypeFilter(option.value)
                            setIsFilterOpen(false)
                          }}
                          className={`w-full px-4 py-2 text-left text-sm transition-colors hover:bg-[--color-slate] ${
                            cryptoTypeFilter === option.value
                              ? 'text-[--color-pulse]'
                              : 'text-[--color-ghost]'
                          }`}
                        >
                          {option.label}
                        </button>
                      ))}
                    </div>
                  </>
                )}
              </div>
            )}
            <span className="text-sm text-[--color-dim]">
              {filteredTransactions.length} transaction{filteredTransactions.length !== 1 ? 's' : ''}
              {cryptoTypeFilter !== 'all' && ` (filtered)`}
            </span>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        {filteredTransactions.length === 0 ? (
          <div className="py-12 text-center">
            <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-xl bg-[--color-slate]">
              <Sparkles className="h-6 w-6 text-[--color-dim]" />
            </div>
            {cryptoTypeFilter !== 'all' ? (
              <>
                <p className="text-[--color-ghost]">No {selectedLabel.toLowerCase()} transactions</p>
                <p className="mt-1 text-sm text-[--color-dim]">
                  Try selecting a different filter or &quot;All Types&quot;.
                </p>
              </>
            ) : (
              <>
                <p className="text-[--color-ghost]">No transactions yet</p>
                <p className="mt-1 text-sm text-[--color-dim]">
                  Transactions will appear here once you send or receive BTH.
                </p>
              </>
            )}
          </div>
        ) : (
          <div className="space-y-2">
            {filteredTransactions.map((tx, i) => (
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
