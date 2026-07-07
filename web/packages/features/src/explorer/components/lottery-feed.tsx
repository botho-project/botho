import type { Block } from '@botho/core'
import { Card, CardHeader, CardTitle, CardContent } from '@botho/ui'
import { Ticket, Clock } from 'lucide-react'
import { formatBTH } from '@botho/core'
import { selectLotteryBlocks } from '../lottery'
import { formatTime } from '../utils'

export interface LotteryFeedProps {
  /** Recent blocks to scan for lottery activity (the explorer's loaded window). */
  blocks: Block[]
  /** Navigate to a block's detail view. */
  onViewBlock?: (block: Block) => void
  /** Custom class name */
  className?: string
}

/**
 * Lottery events feed (#699): blocks in the loaded window with on-chain
 * lottery activity (payouts or pool distribution), newest first.
 *
 * Privacy: on-chain aggregates only — payout counts and totals, never
 * recipients or linkage tooling.
 */
export function LotteryFeed({ blocks, onViewBlock, className }: LotteryFeedProps) {
  const events = selectLotteryBlocks(blocks)

  return (
    <Card className={className}>
      <CardHeader>
        <div className="flex items-center gap-2">
          <Ticket className="h-4 w-4 text-[--color-pulse]" />
          <CardTitle>Lottery Events</CardTitle>
        </div>
      </CardHeader>
      <CardContent className={events.length === 0 ? undefined : 'p-0'}>
        {events.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16">
            <Ticket className="h-12 w-12 text-[--color-dim]" />
            <p className="mt-4 text-lg text-[--color-ghost]">No lottery activity</p>
            <p className="mt-2 text-sm text-[--color-dim]">
              No payouts or pool distributions in the {blocks.length} most recent loaded
              block{blocks.length !== 1 ? 's' : ''}. Load more blocks to widen the window.
            </p>
          </div>
        ) : (
          <div className="divide-y divide-[--color-slate]/30">
            {events.map((block) => {
              const lottery = block.lottery!
              return (
                <div
                  key={block.hash}
                  onClick={onViewBlock ? () => onViewBlock(block) : undefined}
                  className={`flex flex-wrap items-center gap-x-6 gap-y-2 px-6 py-4 ${
                    onViewBlock
                      ? 'cursor-pointer transition-colors hover:bg-[--color-abyss]/50'
                      : ''
                  }`}
                >
                  <div className="flex h-12 w-16 flex-shrink-0 items-center justify-center rounded-lg bg-[--color-pulse]/10">
                    <span className="font-mono text-sm font-bold text-[--color-pulse]">
                      #{block.height}
                    </span>
                  </div>
                  <div className="min-w-[6rem]">
                    <p className="text-xs uppercase tracking-wider text-[--color-dim]">Payouts</p>
                    <p className="mt-1 font-mono text-sm text-[--color-light]">
                      {lottery.payoutCount}
                    </p>
                  </div>
                  <div className="min-w-[8rem]">
                    <p className="text-xs uppercase tracking-wider text-[--color-dim]">
                      Payout total
                    </p>
                    <p className="mt-1 font-mono text-sm text-[--color-success]">
                      {formatBTH(lottery.payoutTotal)} BTH
                    </p>
                  </div>
                  <div className="min-w-[8rem]">
                    <p className="text-xs uppercase tracking-wider text-[--color-dim]">
                      Pool distributed
                    </p>
                    <p className="mt-1 font-mono text-sm text-[--color-light]">
                      {formatBTH(lottery.poolDistributed)} BTH
                    </p>
                  </div>
                  <div className="min-w-[8rem]">
                    <p className="text-xs uppercase tracking-wider text-[--color-dim]">Burned</p>
                    <p className="mt-1 font-mono text-sm text-[--color-danger]">
                      {formatBTH(lottery.amountBurned)} BTH
                    </p>
                  </div>
                  <span className="ml-auto flex items-center gap-1 text-xs text-[--color-dim]">
                    <Clock className="h-3 w-3" />
                    {formatTime(block.timestamp)}
                  </span>
                </div>
              )
            })}
          </div>
        )}
      </CardContent>
    </Card>
  )
}
