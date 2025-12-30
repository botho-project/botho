import type { Block } from '@botho/core'
import { Card, CardHeader, CardTitle, CardContent, Button } from '@botho/ui'
import { ArrowLeft, Blocks, Package } from 'lucide-react'
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
  const { goBack, viewBlock } = useExplorer()

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
            <DetailRow label="Size" value={formatSize(block.size)} />
            <DetailRow
              label="Reward"
              value={`${formatAmount(block.reward)} BTH`}
              valueClass="text-[--color-success]"
            />
            <DetailRow label="Difficulty" value={formatDifficulty(block.difficulty)} />
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
        <CardContent>
          {block.transactionCount === 0 ? (
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
    </div>
  )
}
