import type { Block } from '@botho/core'
import { Card, CardHeader, CardTitle, CardContent, Button } from '@botho/ui'
import { ArrowLeft, Blocks, Package, Ticket } from 'lucide-react'
import { useExplorer } from '../context'
import { DetailRow } from './detail-row'
import { formatTime, formatHash, formatAmount, formatDifficulty, formatSize, ZERO_HASH } from '../utils'

export interface BlockDetailProps {
  /** The block to display */
  block: Block
  /** Custom class name */
  className?: string
}

export function BlockDetail({ block, className }: BlockDetailProps) {
  const { goBack, viewBlock, viewTransactionByHash } = useExplorer()

  // Enriched fields (#699/#700) are additive — older nodes omit them, so
  // every render below guards with undefined checks.
  const hasLotteryActivity =
    block.lottery !== undefined &&
    (block.lottery.payoutCount > 0 || block.lottery.poolDistributed > 0n)

  return (
    <div className={`space-y-4 ${className || ''}`}>
      {/* Back button */}
      <Button variant="ghost" onClick={goBack}>
        <ArrowLeft className="mr-2 h-4 w-4" />
        Back to blocks
      </Button>

      {/* Block header */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-3">
            <div className="flex h-12 w-12 items-center justify-center rounded-lg bg-[--color-pulse]/10">
              <Blocks className="h-6 w-6 text-[--color-pulse]" />
            </div>
            <div>
              <CardTitle>Block #{block.height}</CardTitle>
              <p className="mt-1 font-mono text-xs text-[--color-dim]">{block.hash}</p>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          <div className="grid gap-4 sm:grid-cols-2">
            <DetailRow label="Height" value={block.height.toLocaleString()} />
            <DetailRow label="Timestamp" value={formatTime(block.timestamp)} />
            <DetailRow label="Transactions" value={block.transactionCount.toString()} />
            <DetailRow
              label="Size"
              value={block.size !== undefined ? formatSize(block.size) : '—'}
            />
            <DetailRow
              label="Reward"
              value={
                block.reward !== undefined ? `${formatAmount(block.reward)} BTH` : '—'
              }
              valueClass={block.reward !== undefined ? 'text-[--color-success]' : undefined}
            />
            <DetailRow label="Difficulty" value={formatDifficulty(block.difficulty)} />
            {block.totalFees !== undefined && (
              <DetailRow label="Total Fees" value={`${formatAmount(block.totalFees)} BTH`} />
            )}
            <div className="col-span-2">
              <DetailRow
                label="Previous Block"
                value={formatHash(block.previousHash, 16)}
                mono
                onClick={
                  block.previousHash !== ZERO_HASH
                    ? () => viewBlock(block.previousHash)
                    : undefined
                }
              />
            </div>
            {block.minter && (
              <div className="col-span-2">
                <DetailRow label="Minter" value={formatHash(block.minter, 16)} mono />
              </div>
            )}
          </div>
        </CardContent>
      </Card>

      {/* Transactions in block */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <Package className="h-4 w-4 text-[--color-pulse]" />
            <CardTitle>Transactions</CardTitle>
          </div>
        </CardHeader>
        <CardContent className={block.transactions && block.transactions.length > 0 ? 'p-0' : undefined}>
          {block.transactions && block.transactions.length > 0 ? (
            /* Enriched per-tx structure (#700): hash, fee, ring size. */
            <div className="divide-y divide-[--color-slate]/30">
              {block.transactions.map((tx) => (
                <div key={tx.hash} className="flex items-center gap-4 px-6 py-3">
                  <button
                    onClick={() => viewTransactionByHash(tx.hash)}
                    className="min-w-0 flex-1 truncate text-left font-mono text-sm text-[--color-ghost] hover:text-[--color-pulse] hover:underline"
                    title={tx.hash}
                  >
                    {formatHash(tx.hash, 12)}
                  </button>
                  <div className="text-right">
                    <p className="text-xs uppercase tracking-wider text-[--color-dim]">Fee</p>
                    <p className="mt-0.5 font-mono text-sm text-[--color-light]">
                      {formatAmount(tx.fee)} BTH
                    </p>
                  </div>
                  <div className="w-16 text-right">
                    <p className="text-xs uppercase tracking-wider text-[--color-dim]">Ring</p>
                    <p className="mt-0.5 font-mono text-sm text-[--color-light]">{tx.ringSize}</p>
                  </div>
                </div>
              ))}
            </div>
          ) : block.transactionCount === 0 ? (
            <p className="py-8 text-center text-sm text-[--color-dim]">
              No transactions in this block (minting reward only)
            </p>
          ) : (
            <p className="py-8 text-center text-sm text-[--color-dim]">
              {block.transactionCount} transaction
              {block.transactionCount !== 1 ? 's' : ''} in this block
            </p>
          )}
        </CardContent>
      </Card>

      {/* Lottery summary (#699): shown only when the block has lottery activity */}
      {hasLotteryActivity && block.lottery && (
        <Card>
          <CardHeader>
            <div className="flex items-center gap-2">
              <Ticket className="h-4 w-4 text-[--color-pulse]" />
              <CardTitle>Lottery</CardTitle>
            </div>
          </CardHeader>
          <CardContent>
            <div className="grid gap-4 sm:grid-cols-2">
              <DetailRow label="Payouts" value={block.lottery.payoutCount.toString()} />
              <DetailRow
                label="Payout Total"
                value={`${formatAmount(block.lottery.payoutTotal)} BTH`}
                valueClass="text-[--color-success]"
              />
              <DetailRow
                label="Pool Distributed"
                value={`${formatAmount(block.lottery.poolDistributed)} BTH`}
              />
              <DetailRow
                label="Amount Burned"
                value={`${formatAmount(block.lottery.amountBurned)} BTH`}
                valueClass="text-[--color-danger]"
              />
              <DetailRow
                label="Lottery Fees"
                value={`${formatAmount(block.lottery.totalFees)} BTH`}
              />
              {block.lottery.lotterySeed && (
                <DetailRow
                  label="Lottery Seed"
                  value={formatHash(block.lottery.lotterySeed, 16)}
                  mono
                />
              )}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  )
}
