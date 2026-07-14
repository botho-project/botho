import type { Transaction } from '@botho/core'
import { Card, CardHeader, CardTitle, CardContent, Button } from '@botho/ui'
import { ArrowLeft, ArrowUpRight, ArrowDownLeft, Pickaxe, FileText } from 'lucide-react'
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

  // The block explorer's node RPC exposes no per-tx direction, so `type` is
  // often absent (#913). Fall back to a neutral icon/label rather than
  // asserting "receive" for every transaction.
  const TypeIcon =
    transaction.type === 'send'
      ? ArrowUpRight
      : transaction.type === 'receive'
        ? ArrowDownLeft
        : transaction.type === 'minting'
          ? Pickaxe
          : FileText

  const typeColors = {
    send: 'text-[--color-danger]',
    receive: 'text-[--color-success]',
    minting: 'text-[--color-pulse]',
  } as const

  const typeBgColors = {
    send: 'bg-[--color-danger]/10',
    receive: 'bg-[--color-success]/10',
    minting: 'bg-[--color-pulse]/10',
  } as const

  const typeColor = transaction.type ? typeColors[transaction.type] : 'text-[--color-dim]'
  const typeBgColor = transaction.type ? typeBgColors[transaction.type] : 'bg-[--color-slate]'
  const typeLabel = transaction.type ?? 'Transaction'

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
              className={`flex h-12 w-12 items-center justify-center rounded-lg ${typeBgColor}`}
            >
              <TypeIcon className={`h-6 w-6 ${typeColor}`} />
            </div>
            <div>
              <CardTitle className="capitalize">{typeLabel}</CardTitle>
              <p className="mt-1 font-mono text-xs text-[--color-dim]">{transaction.id}</p>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          <div className="grid gap-4 sm:grid-cols-2">
            {/* No "Amount" row: the node exposes no per-tx amount, and under
                confidential transactions (ADR 0006) it never will. Rendering a
                fabricated "0 BTH" here read as a real value (#913, deprecation
                D1 in docs/design/post-ct-analytics.md). Fees are public and
                stay. */}
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
            <DetailRow
              label="Time"
              value={transaction.timestamp != null ? formatTime(transaction.timestamp) : '—'}
            />
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
