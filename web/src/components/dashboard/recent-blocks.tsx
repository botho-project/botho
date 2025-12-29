import { motion } from 'motion/react'
import { Blocks, ArrowRight } from 'lucide-react'
import { Link } from 'react-router-dom'
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card'
import { formatHash, timeAgo } from '@/lib/utils'

interface Block {
  height: number
  hash: string
  timestamp: number
  txCount: number
  miner: string
  reward: number
}

// Mock data
const mockBlocks: Block[] = [
  { height: 1234567, hash: 'a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0', timestamp: Date.now() / 1000 - 12, txCount: 45, miner: 'pool.cadence.io', reward: 2.5 },
  { height: 1234566, hash: 'b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0a1', timestamp: Date.now() / 1000 - 78, txCount: 32, miner: '0x1234...5678', reward: 2.5 },
  { height: 1234565, hash: 'c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0a1b2', timestamp: Date.now() / 1000 - 145, txCount: 67, miner: 'pool.cadence.io', reward: 2.5 },
  { height: 1234564, hash: 'd4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0a1b2c3', timestamp: Date.now() / 1000 - 198, txCount: 28, miner: '0xabcd...ef01', reward: 2.5 },
  { height: 1234563, hash: 'e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0a1b2c3d4', timestamp: Date.now() / 1000 - 267, txCount: 51, miner: 'pool.cadence.io', reward: 2.5 },
]

export function RecentBlocks() {
  return (
    <Card>
      <CardHeader>
        <div className="flex items-center gap-2">
          <Blocks className="h-4 w-4 text-[--color-pulse]" />
          <CardTitle>Recent Blocks</CardTitle>
        </div>
        <Link
          to="/blocks"
          className="flex items-center gap-1 text-xs text-[--color-pulse] transition-colors hover:text-[--color-pulse-dim]"
        >
          View all <ArrowRight className="h-3 w-3" />
        </Link>
      </CardHeader>
      <CardContent className="p-0">
        <div className="divide-y divide-[--color-steel]">
          {mockBlocks.map((block, i) => (
            <motion.div
              key={block.height}
              initial={{ opacity: 0, x: -20 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ duration: 0.3, delay: i * 0.05 }}
              className="group flex items-center justify-between px-5 py-3 transition-colors hover:bg-[--color-slate]/50"
            >
              <div className="flex items-center gap-4">
                <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-pulse]/10 font-display text-sm font-bold text-[--color-pulse]">
                  #{block.height.toString().slice(-4)}
                </div>
                <div>
                  <Link
                    to={`/blocks/${block.height}`}
                    className="font-mono text-sm text-[--color-light] transition-colors hover:text-[--color-pulse]"
                  >
                    {formatHash(block.hash, 10)}
                  </Link>
                  <p className="text-xs text-[--color-dim]">{timeAgo(block.timestamp)}</p>
                </div>
              </div>
              <div className="text-right">
                <p className="font-mono text-sm text-[--color-soft]">{block.txCount} txs</p>
                <p className="text-xs text-[--color-success]">+{block.reward} CAD</p>
              </div>
            </motion.div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}
