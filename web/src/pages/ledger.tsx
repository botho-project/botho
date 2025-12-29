import { Layout } from '@/components/layout'
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card'
import { motion } from 'motion/react'
import {
  Blocks,
  Activity,
  ChevronRight,
  Clock,
  Cpu,
  Database,
  Hash,
  Layers,
  Search,
  Shield,
  ShieldOff,
} from 'lucide-react'
import { useState } from 'react'
import { cn, formatHash, timeAgo } from '@/lib/utils'

interface Block {
  height: number
  hash: string
  prevHash: string
  timestamp: number
  txCount: number
  size: number
  miner: string
  reward: number
  difficulty: string
  nonce: number
}

interface Transaction {
  hash: string
  blockHeight: number
  timestamp: number
  type: 'plain' | 'hidden' | 'mining'
  inputs: number
  outputs: number
  fee: number
  amount?: number
}

const mockBlocks: Block[] = Array.from({ length: 10 }, (_, i) => ({
  height: 1234567 - i,
  hash: `${Math.random().toString(16).slice(2)}${Math.random().toString(16).slice(2)}`.slice(0, 64),
  prevHash: `${Math.random().toString(16).slice(2)}${Math.random().toString(16).slice(2)}`.slice(0, 64),
  timestamp: Date.now() / 1000 - i * 60 - Math.random() * 30,
  txCount: Math.floor(Math.random() * 100) + 10,
  size: Math.floor(Math.random() * 50000) + 10000,
  miner: i % 3 === 0 ? 'pool.cadence.io' : `0x${Math.random().toString(16).slice(2, 10)}`,
  reward: 2.5,
  difficulty: '2.45T',
  nonce: Math.floor(Math.random() * 1000000000),
}))

const mockTransactions: Transaction[] = Array.from({ length: 15 }, (_, i) => ({
  hash: `${Math.random().toString(16).slice(2)}${Math.random().toString(16).slice(2)}`.slice(0, 64),
  blockHeight: 1234567 - Math.floor(i / 3),
  timestamp: Date.now() / 1000 - i * 45 - Math.random() * 30,
  type: ['plain', 'hidden', 'mining'][Math.floor(Math.random() * 3)] as 'plain' | 'hidden' | 'mining',
  inputs: Math.floor(Math.random() * 5) + 1,
  outputs: Math.floor(Math.random() * 3) + 1,
  fee: Math.random() * 0.1,
  amount: Math.random() * 500 + 10,
}))

const txTypeStyles = {
  plain: { icon: ShieldOff, bg: 'bg-[--color-pulse]/10', text: 'text-[--color-pulse]' },
  hidden: { icon: Shield, bg: 'bg-[--color-purple]/10', text: 'text-[--color-purple]' },
  mining: { icon: Cpu, bg: 'bg-[--color-success]/10', text: 'text-[--color-success]' },
}

export function LedgerPage() {
  const [activeTab, setActiveTab] = useState<'blocks' | 'transactions'>('blocks')
  const [selectedBlock, setSelectedBlock] = useState<Block | null>(null)

  return (
    <Layout title="Ledger" subtitle="Explore blocks and transactions">
      <div className="space-y-6">
        {/* Stats bar */}
        <div className="grid grid-cols-5 gap-4">
          {[
            { label: 'Block Height', value: '1,234,567', icon: Layers, color: 'pulse' },
            { label: 'Total Transactions', value: '45.2M', icon: Activity, color: 'success' },
            { label: 'Chain Size', value: '128.5 GB', icon: Database, color: 'purple' },
            { label: 'Avg Block Time', value: '60s', icon: Clock, color: 'warning' },
            { label: 'Avg Block Size', value: '32 KB', icon: Blocks, color: 'pulse' },
          ].map((stat, i) => (
            <motion.div
              key={stat.label}
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: i * 0.05 }}
              className="rounded-xl border border-[--color-steel] bg-[--color-abyss]/80 p-4"
            >
              <div className="flex items-center gap-2">
                <stat.icon className={cn('h-4 w-4', `text-[--color-${stat.color}]`)} />
                <span className="text-xs text-[--color-dim]">{stat.label}</span>
              </div>
              <p className="mt-1 font-display text-xl font-bold text-[--color-light]">{stat.value}</p>
            </motion.div>
          ))}
        </div>

        {/* Search */}
        <div className="relative">
          <Search className="absolute left-4 top-1/2 h-5 w-5 -translate-y-1/2 text-[--color-dim]" />
          <input
            type="text"
            placeholder="Search by block height, block hash, transaction hash, or address..."
            className="h-12 w-full rounded-xl border border-[--color-steel] bg-[--color-abyss] pl-12 pr-4 text-[--color-light] placeholder:text-[--color-dim] focus:border-[--color-pulse-dim] focus:outline-none focus:ring-1 focus:ring-[--color-pulse-dim]"
          />
        </div>

        {/* Tabs */}
        <div className="flex gap-2 border-b border-[--color-steel]">
          <button
            onClick={() => setActiveTab('blocks')}
            className={cn(
              'flex items-center gap-2 border-b-2 px-4 py-3 text-sm font-medium transition-colors',
              activeTab === 'blocks'
                ? 'border-[--color-pulse] text-[--color-pulse]'
                : 'border-transparent text-[--color-dim] hover:text-[--color-soft]'
            )}
          >
            <Blocks className="h-4 w-4" />
            Blocks
          </button>
          <button
            onClick={() => setActiveTab('transactions')}
            className={cn(
              'flex items-center gap-2 border-b-2 px-4 py-3 text-sm font-medium transition-colors',
              activeTab === 'transactions'
                ? 'border-[--color-pulse] text-[--color-pulse]'
                : 'border-transparent text-[--color-dim] hover:text-[--color-soft]'
            )}
          >
            <Activity className="h-4 w-4" />
            Transactions
          </button>
        </div>

        {/* Content */}
        <div className="grid grid-cols-3 gap-6">
          {/* Main list */}
          <div className="col-span-2">
            <Card>
              <CardHeader>
                <CardTitle>
                  {activeTab === 'blocks' ? 'Recent Blocks' : 'Recent Transactions'}
                </CardTitle>
                <span className="text-xs text-[--color-dim]">
                  Showing latest {activeTab === 'blocks' ? mockBlocks.length : mockTransactions.length}
                </span>
              </CardHeader>
              <CardContent className="p-0">
                {activeTab === 'blocks' ? (
                  <div className="divide-y divide-[--color-steel]">
                    {mockBlocks.map((block, i) => (
                      <motion.div
                        key={block.height}
                        initial={{ opacity: 0, x: -20 }}
                        animate={{ opacity: 1, x: 0 }}
                        transition={{ delay: i * 0.03 }}
                        onClick={() => setSelectedBlock(block)}
                        className={cn(
                          'group flex cursor-pointer items-center justify-between px-5 py-4 transition-colors hover:bg-[--color-slate]/50',
                          selectedBlock?.height === block.height && 'bg-[--color-slate]/50'
                        )}
                      >
                        <div className="flex items-center gap-4">
                          <div className="flex h-12 w-16 flex-col items-center justify-center rounded-lg bg-[--color-pulse]/10">
                            <Blocks className="h-4 w-4 text-[--color-pulse]" />
                            <span className="mt-0.5 font-mono text-xs font-bold text-[--color-pulse]">
                              {block.height.toLocaleString()}
                            </span>
                          </div>
                          <div>
                            <div className="flex items-center gap-2">
                              <Hash className="h-3 w-3 text-[--color-dim]" />
                              <span className="font-mono text-sm text-[--color-soft]">
                                {formatHash(block.hash, 12)}
                              </span>
                            </div>
                            <div className="mt-1 flex items-center gap-3 text-xs text-[--color-dim]">
                              <span>{timeAgo(block.timestamp)}</span>
                              <span>•</span>
                              <span>{block.txCount} txs</span>
                              <span>•</span>
                              <span>{(block.size / 1024).toFixed(1)} KB</span>
                            </div>
                          </div>
                        </div>
                        <div className="flex items-center gap-4">
                          <div className="text-right">
                            <p className="font-mono text-sm text-[--color-success]">+{block.reward} CAD</p>
                            <p className="text-xs text-[--color-dim]">{block.miner}</p>
                          </div>
                          <ChevronRight className="h-4 w-4 text-[--color-dim] transition-transform group-hover:translate-x-1" />
                        </div>
                      </motion.div>
                    ))}
                  </div>
                ) : (
                  <div className="divide-y divide-[--color-steel]">
                    {mockTransactions.map((tx, i) => {
                      const typeStyle = txTypeStyles[tx.type]
                      const TypeIcon = typeStyle.icon

                      return (
                        <motion.div
                          key={tx.hash}
                          initial={{ opacity: 0, x: -20 }}
                          animate={{ opacity: 1, x: 0 }}
                          transition={{ delay: i * 0.03 }}
                          className="group flex cursor-pointer items-center justify-between px-5 py-4 transition-colors hover:bg-[--color-slate]/50"
                        >
                          <div className="flex items-center gap-4">
                            <div className={cn('flex h-10 w-10 items-center justify-center rounded-lg', typeStyle.bg)}>
                              <TypeIcon className={cn('h-5 w-5', typeStyle.text)} />
                            </div>
                            <div>
                              <div className="flex items-center gap-2">
                                <span className="font-mono text-sm text-[--color-soft]">
                                  {formatHash(tx.hash, 12)}
                                </span>
                                <span className={cn('rounded px-1.5 py-0.5 text-xs font-medium capitalize', typeStyle.bg, typeStyle.text)}>
                                  {tx.type}
                                </span>
                              </div>
                              <div className="mt-1 flex items-center gap-3 text-xs text-[--color-dim]">
                                <span>{timeAgo(tx.timestamp)}</span>
                                <span>•</span>
                                <span>Block #{tx.blockHeight.toLocaleString()}</span>
                                <span>•</span>
                                <span>{tx.inputs} in → {tx.outputs} out</span>
                              </div>
                            </div>
                          </div>
                          <div className="flex items-center gap-4">
                            <div className="text-right">
                              {tx.amount && (
                                <p className="font-mono text-sm text-[--color-light]">
                                  {tx.amount.toFixed(2)} CAD
                                </p>
                              )}
                              <p className="text-xs text-[--color-warning]">
                                Fee: {tx.fee.toFixed(4)} CAD
                              </p>
                            </div>
                            <ChevronRight className="h-4 w-4 text-[--color-dim] transition-transform group-hover:translate-x-1" />
                          </div>
                        </motion.div>
                      )
                    })}
                  </div>
                )}
              </CardContent>
            </Card>
          </div>

          {/* Details panel */}
          <div>
            {selectedBlock ? (
              <motion.div
                key={selectedBlock.height}
                initial={{ opacity: 0, x: 20 }}
                animate={{ opacity: 1, x: 0 }}
              >
                <Card>
                  <CardHeader>
                    <div className="flex items-center gap-2">
                      <Blocks className="h-4 w-4 text-[--color-pulse]" />
                      <CardTitle>Block #{selectedBlock.height.toLocaleString()}</CardTitle>
                    </div>
                  </CardHeader>
                  <CardContent className="space-y-4">
                    <div>
                      <p className="text-xs text-[--color-dim]">Block Hash</p>
                      <p className="mt-1 break-all font-mono text-xs text-[--color-soft]">
                        {selectedBlock.hash}
                      </p>
                    </div>
                    <div>
                      <p className="text-xs text-[--color-dim]">Previous Block</p>
                      <p className="mt-1 break-all font-mono text-xs text-[--color-soft]">
                        {selectedBlock.prevHash}
                      </p>
                    </div>
                    <div className="grid grid-cols-2 gap-4">
                      <div>
                        <p className="text-xs text-[--color-dim]">Timestamp</p>
                        <p className="mt-1 font-mono text-sm text-[--color-light]">
                          {new Date(selectedBlock.timestamp * 1000).toLocaleString()}
                        </p>
                      </div>
                      <div>
                        <p className="text-xs text-[--color-dim]">Transactions</p>
                        <p className="mt-1 font-mono text-sm text-[--color-light]">
                          {selectedBlock.txCount}
                        </p>
                      </div>
                      <div>
                        <p className="text-xs text-[--color-dim]">Size</p>
                        <p className="mt-1 font-mono text-sm text-[--color-light]">
                          {(selectedBlock.size / 1024).toFixed(2)} KB
                        </p>
                      </div>
                      <div>
                        <p className="text-xs text-[--color-dim]">Difficulty</p>
                        <p className="mt-1 font-mono text-sm text-[--color-light]">
                          {selectedBlock.difficulty}
                        </p>
                      </div>
                      <div>
                        <p className="text-xs text-[--color-dim]">Nonce</p>
                        <p className="mt-1 font-mono text-sm text-[--color-light]">
                          {selectedBlock.nonce.toLocaleString()}
                        </p>
                      </div>
                      <div>
                        <p className="text-xs text-[--color-dim]">Reward</p>
                        <p className="mt-1 font-mono text-sm text-[--color-success]">
                          +{selectedBlock.reward} CAD
                        </p>
                      </div>
                    </div>
                    <div>
                      <p className="text-xs text-[--color-dim]">Miner</p>
                      <p className="mt-1 font-mono text-sm text-[--color-soft]">
                        {selectedBlock.miner}
                      </p>
                    </div>
                  </CardContent>
                </Card>
              </motion.div>
            ) : (
              <div className="flex h-64 items-center justify-center rounded-xl border border-dashed border-[--color-steel]">
                <div className="text-center">
                  <Blocks className="mx-auto h-8 w-8 text-[--color-dim]" />
                  <p className="mt-2 text-sm text-[--color-dim]">
                    Select a block to view details
                  </p>
                </div>
              </div>
            )}

            {/* Chain visualization */}
            <Card className="mt-6">
              <CardHeader>
                <CardTitle>Chain Visualization</CardTitle>
              </CardHeader>
              <CardContent>
                <div className="flex items-center justify-between">
                  {mockBlocks.slice(0, 5).map((block, i) => (
                    <div key={block.height} className="flex items-center">
                      <motion.div
                        initial={{ scale: 0 }}
                        animate={{ scale: 1 }}
                        transition={{ delay: i * 0.1 }}
                        onClick={() => setSelectedBlock(block)}
                        className={cn(
                          'flex h-10 w-10 cursor-pointer items-center justify-center rounded-lg border-2 transition-all',
                          selectedBlock?.height === block.height
                            ? 'border-[--color-pulse] bg-[--color-pulse]/20'
                            : 'border-[--color-steel] bg-[--color-slate] hover:border-[--color-pulse-dim]'
                        )}
                      >
                        <span className="font-mono text-xs text-[--color-light]">
                          {block.height.toString().slice(-3)}
                        </span>
                      </motion.div>
                      {i < 4 && (
                        <div className="mx-1 h-0.5 w-4 bg-[--color-steel]" />
                      )}
                    </div>
                  ))}
                </div>
                <p className="mt-3 text-center text-xs text-[--color-dim]">
                  ← Older blocks | Newer blocks →
                </p>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </Layout>
  )
}
