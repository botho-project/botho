import type { Transaction } from '@botho/core'
import { Card, CardHeader, CardTitle, CardContent, Button } from '@botho/ui'
import { ArrowLeft, ArrowUpRight, ArrowDownLeft, Pickaxe } from 'lucide-react'
import { useExplorer } from '../context'
import { DetailRow } from './detail-row'
import { formatTime, formatHash, formatAmount } from '../utils'

export interface TransactionDetailProps {
  /** The transaction to display */
  transaction: Transaction
  /** Custom class name */
  className?: string
}

export function TransactionDetail({ transaction, className }: TransactionDetailProps) {
  const { goBack, viewBlock } = useExplorer()

  const TypeIcon =
    transaction.type === 'send'
      ? ArrowUpRight
      : transaction.type === 'receive'
        ? ArrowDownLeft
        : Pickaxe

  const typeColors = {
    send: 'text-[--color-danger]',
    receive: 'text-[--color-success]',
    minting: 'text-[--color-pulse]',
  }

  const typeBgColors = {
    send: 'bg-[--color-danger]/10',
    receive: 'bg-[--color-success]/10',
    minting: 'bg-[--color-pulse]/10',
  }

  return (
    <div className={`space-y-4 ${className || ''}`}>
      {/* Back button */}
      <Button variant="ghost" onClick={goBack}>
        <ArrowLeft className="mr-2 h-4 w-4" />
        Back to blocks
      </Button>

      {/* Transaction header */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-3">
            <div
              className={`flex h-12 w-12 items-center justify-center rounded-lg ${typeBgColors[transaction.type]}`}
            >
              <TypeIcon className={`h-6 w-6 ${typeColors[transaction.type]}`} />
            </div>
            <div>
              <CardTitle className="capitalize">{transaction.type}</CardTitle>
              <p className="mt-1 font-mono text-xs text-[--color-dim]">{transaction.id}</p>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          <div className="grid gap-4 sm:grid-cols-2">
            <DetailRow
              label="Amount"
              value={`${formatAmount(transaction.amount)} BTH`}
              valueClass={typeColors[transaction.type]}
            />
            <DetailRow label="Fee" value={`${formatAmount(transaction.fee)} BTH`} />
            <DetailRow
              label="Status"
              value={transaction.status}
              valueClass={
                transaction.status === 'confirmed'
                  ? 'text-[--color-success]'
                  : transaction.status === 'pending'
                    ? 'text-[--color-pulse]'
                    : 'text-[--color-danger]'
              }
            />
            <DetailRow label="Confirmations" value={transaction.confirmations.toString()} />
            <DetailRow
              label="Privacy"
              value={transaction.privacyLevel}
              valueClass={
                transaction.privacyLevel === 'private'
                  ? 'text-[--color-success]'
                  : 'text-[--color-ghost]'
              }
            />
            <DetailRow label="Time" value={formatTime(transaction.timestamp)} />
            {transaction.blockHeight && (
              <DetailRow
                label="Block"
                value={`#${transaction.blockHeight}`}
                onClick={() => viewBlock(transaction.blockHeight!)}
              />
            )}
            {transaction.counterparty && (
              <div className="col-span-2">
                <DetailRow
                  label={transaction.type === 'send' ? 'To' : 'From'}
                  value={formatHash(transaction.counterparty, 16)}
                  mono
                />
              </div>
            )}
            {transaction.memo && (
              <div className="col-span-2">
                <DetailRow label="Memo" value={transaction.memo} />
              </div>
            )}
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
