import { Card, CardHeader, CardTitle, CardContent, Button } from '@botho/ui'
import { motion } from 'motion/react'
import {
  Blocks,
  Clock,
  Hash,
  ChevronRight,
  Package,
  Loader2,
} from 'lucide-react'
import { useExplorer } from '../context'
import { formatTime, formatHash, formatAmount } from '../utils'

export interface BlockListProps {
  /** Title for the card */
  title?: string
  /** Custom class name */
  className?: string
}

export function BlockList({ title = 'Recent Blocks', className }: BlockListProps) {
  const { blocks, loading, loadingMore, viewBlock, loadMore } = useExplorer()

  if (loading && blocks.length === 0) {
    return (
      <Card className={className}>
        <CardContent className="flex items-center justify-center py-20">
          <Loader2 className="h-8 w-8 animate-spin text-[--color-pulse]" />
        </CardContent>
      </Card>
    )
  }

  if (blocks.length === 0) {
    return (
      <Card className={className}>
        <CardContent className="flex flex-col items-center justify-center py-20">
          <Blocks className="h-16 w-16 text-[--color-dim]" />
          <p className="mt-4 text-lg text-[--color-ghost]">No blocks found</p>
          <p className="mt-2 text-sm text-[--color-dim]">
            Waiting for blocks to be mined...
          </p>
        </CardContent>
      </Card>
    )
  }

  return (
    <Card className={className}>
      <CardHeader>
        <div className="flex items-center gap-2">
          <Blocks className="h-4 w-4 text-[--color-pulse]" />
          <CardTitle>{title}</CardTitle>
        </div>
      </CardHeader>
      <CardContent className="p-0">
        <div className="divide-y divide-[--color-slate]/30">
          {blocks.map((block, i) => (
            <motion.div
              key={block.hash}
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: i * 0.02 }}
              onClick={() => viewBlock(block)}
              className="flex cursor-pointer items-center gap-4 px-6 py-4 transition-colors hover:bg-[--color-abyss]/50"
            >
              {/* Block height */}
              <div className="flex h-12 w-12 flex-shrink-0 items-center justify-center rounded-lg bg-[--color-pulse]/10">
                <span className="font-mono text-sm font-bold text-[--color-pulse]">
                  #{block.height}
                </span>
              </div>

              {/* Block info */}
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <Hash className="h-3 w-3 text-[--color-dim]" />
                  <span className="font-mono text-sm text-[--color-ghost]">
                    {formatHash(block.hash)}
                  </span>
                </div>
                <div className="mt-1 flex items-center gap-4 text-xs text-[--color-dim]">
                  <span className="flex items-center gap-1">
                    <Clock className="h-3 w-3" />
                    {formatTime(block.timestamp)}
                  </span>
                  <span className="flex items-center gap-1">
                    <Package className="h-3 w-3" />
                    {block.transactionCount} txs
                  </span>
                </div>
              </div>

              {/* Reward */}
              <div className="text-right">
                <p className="font-mono text-sm text-[--color-success]">
                  +{formatAmount(block.reward)}
                </p>
                <p className="mt-1 text-xs text-[--color-dim]">reward</p>
              </div>

              <ChevronRight className="h-4 w-4 text-[--color-dim]" />
            </motion.div>
          ))}
        </div>

        {/* Load more button */}
        <div className="border-t border-[--color-slate]/30 px-6 py-4">
          <Button
            variant="secondary"
            className="w-full"
            onClick={loadMore}
            disabled={loadingMore}
          >
            {loadingMore ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                Loading...
              </>
            ) : (
              'Load More'
            )}
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
