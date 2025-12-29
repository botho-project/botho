import { useCallback, useEffect, useState } from 'react'
import { Layout } from '../components/layout'
import { Card, CardHeader, CardTitle, CardContent, Button } from '@botho/ui'
import { useConnection } from '../contexts/connection'
import type { Block, Transaction } from '@botho/core'
import { motion, AnimatePresence } from 'motion/react'
import {
  Database,
  Blocks,
  Clock,
  Hash,
  ArrowLeft,
  ChevronRight,
  Search,
  Loader2,
  Package,
  ArrowUpRight,
  ArrowDownLeft,
  Pickaxe,
  AlertCircle,
} from 'lucide-react'

// Format a timestamp as relative time or date
function formatTime(timestamp: number): string {
  const now = Math.floor(Date.now() / 1000)
  const diff = now - timestamp

  if (diff < 60) return `${diff}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`

  return new Date(timestamp * 1000).toLocaleDateString()
}

// Format a hash for display (truncated)
function formatHash(hash: string, length = 8): string {
  if (hash.length <= length * 2) return hash
  return `${hash.slice(0, length)}...${hash.slice(-length)}`
}

// Format amount (assuming 12 decimal places like Monero)
function formatAmount(amount: bigint): string {
  const credits = Number(amount) / 1_000_000_000_000
  return credits.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 6,
  })
}

// Format difficulty as human readable
function formatDifficulty(difficulty: bigint): string {
  const num = Number(difficulty)
  if (num >= 1e12) return `${(num / 1e12).toFixed(2)}T`
  if (num >= 1e9) return `${(num / 1e9).toFixed(2)}G`
  if (num >= 1e6) return `${(num / 1e6).toFixed(2)}M`
  if (num >= 1e3) return `${(num / 1e3).toFixed(2)}K`
  return num.toString()
}

type ViewMode = 'list' | 'block' | 'transaction'

interface ViewState {
  mode: ViewMode
  blockHash?: string
  txHash?: string
}

export function LedgerPage() {
  const { adapter, connectedNode } = useConnection()
  const [blocks, setBlocks] = useState<Block[]>([])
  const [loading, setLoading] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [searchQuery, setSearchQuery] = useState('')
  const [view, setView] = useState<ViewState>({ mode: 'list' })
  const [selectedBlock, setSelectedBlock] = useState<Block | null>(null)
  const [selectedTx, setSelectedTx] = useState<Transaction | null>(null)

  // Load recent blocks
  const loadBlocks = useCallback(async () => {
    if (!adapter) return

    setLoading(true)
    setError(null)

    try {
      const recentBlocks = await adapter.getRecentBlocks({ limit: 20 })
      setBlocks(recentBlocks)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load blocks')
    } finally {
      setLoading(false)
    }
  }, [adapter])

  // Load more blocks
  const loadMoreBlocks = useCallback(async () => {
    if (!adapter || blocks.length === 0 || loadingMore) return

    setLoadingMore(true)
    try {
      const lastBlock = blocks[blocks.length - 1]
      const moreBlocks = await adapter.getRecentBlocks({
        limit: 20,
        startHeight: lastBlock.height - 1,
      })
      setBlocks((prev) => [...prev, ...moreBlocks])
    } catch (err) {
      console.error('Failed to load more blocks:', err)
    } finally {
      setLoadingMore(false)
    }
  }, [adapter, blocks, loadingMore])

  // Handle search
  const handleSearch = useCallback(async () => {
    if (!adapter || !searchQuery.trim()) return

    setLoading(true)
    setError(null)

    const query = searchQuery.trim()

    try {
      // Check if it's a block height (number)
      if (/^\d+$/.test(query)) {
        const block = await adapter.getBlock(parseInt(query, 10))
        if (block) {
          setSelectedBlock(block)
          setView({ mode: 'block', blockHash: block.hash })
        } else {
          setError(`Block at height ${query} not found`)
        }
      }
      // Check if it's a hash (64 hex chars)
      else if (/^[0-9a-fA-F]{64}$/.test(query)) {
        // Try as block hash first
        const block = await adapter.getBlock(query)
        if (block) {
          setSelectedBlock(block)
          setView({ mode: 'block', blockHash: block.hash })
        } else {
          // Try as transaction hash
          const tx = await adapter.getTransaction(query)
          if (tx) {
            setSelectedTx(tx)
            setView({ mode: 'transaction', txHash: tx.id })
          } else {
            setError('Block or transaction not found')
          }
        }
      } else {
        setError('Invalid search query. Enter a block height or hash.')
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Search failed')
    } finally {
      setLoading(false)
    }
  }, [adapter, searchQuery])

  // View a specific block
  const viewBlock = useCallback(
    async (blockOrHashOrHeight: Block | string | number) => {
      if (!adapter) return

      setLoading(true)
      setError(null)

      try {
        let block: Block | null
        if (typeof blockOrHashOrHeight === 'object') {
          block = blockOrHashOrHeight
        } else {
          block = await adapter.getBlock(blockOrHashOrHeight)
        }

        if (block) {
          setSelectedBlock(block)
          setView({ mode: 'block', blockHash: block.hash })
        } else {
          setError('Block not found')
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to load block')
      } finally {
        setLoading(false)
      }
    },
    [adapter]
  )

  // Go back to list
  const goBack = useCallback(() => {
    setView({ mode: 'list' })
    setSelectedBlock(null)
    setSelectedTx(null)
    setError(null)
  }, [])

  // Initial load
  useEffect(() => {
    if (adapter && connectedNode) {
      loadBlocks()
    }
  }, [adapter, connectedNode, loadBlocks])

  // Subscribe to new blocks
  useEffect(() => {
    if (!adapter) return

    const unsubscribe = adapter.onNewBlock((block) => {
      setBlocks((prev) => {
        // Add new block at the beginning, remove duplicates
        const filtered = prev.filter((b) => b.hash !== block.hash)
        return [block, ...filtered].slice(0, 50)
      })
    })

    return unsubscribe
  }, [adapter])

  // Not connected state
  if (!connectedNode) {
    return (
      <Layout title="Ledger" subtitle="Browse blocks and transactions">
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-20">
            <Database className="h-16 w-16 text-[--color-dim]" />
            <p className="mt-4 text-lg text-[--color-ghost]">
              Not connected to a node
            </p>
            <p className="mt-2 text-sm text-[--color-dim]">
              Connect to a Botho node to browse the blockchain
            </p>
          </CardContent>
        </Card>
      </Layout>
    )
  }

  return (
    <Layout title="Ledger" subtitle="Browse blocks and transactions">
      <div className="space-y-6">
        {/* Search bar */}
        <Card>
          <CardContent className="py-4">
            <div className="flex gap-3">
              <div className="relative flex-1">
                <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[--color-dim]" />
                <input
                  type="text"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
                  placeholder="Search by block height or hash..."
                  className="w-full rounded-lg border border-[--color-slate]/50 bg-[--color-void]/50 py-2.5 pl-10 pr-4 font-mono text-sm text-[--color-light] placeholder:text-[--color-dim] focus:border-[--color-pulse] focus:outline-none focus:ring-1 focus:ring-[--color-pulse]"
                />
              </div>
              <Button onClick={handleSearch} disabled={loading}>
                {loading ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  'Search'
                )}
              </Button>
            </div>
          </CardContent>
        </Card>

        {/* Error message */}
        <AnimatePresence>
          {error && (
            <motion.div
              initial={{ opacity: 0, y: -10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
            >
              <Card className="border-[--color-danger]/50">
                <CardContent className="flex items-center gap-3 py-3">
                  <AlertCircle className="h-5 w-5 text-[--color-danger]" />
                  <p className="text-sm text-[--color-danger]">{error}</p>
                </CardContent>
              </Card>
            </motion.div>
          )}
        </AnimatePresence>

        {/* Content based on view mode */}
        <AnimatePresence mode="wait">
          {view.mode === 'list' && (
            <motion.div
              key="list"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
            >
              <BlockList
                blocks={blocks}
                loading={loading}
                loadingMore={loadingMore}
                onBlockClick={viewBlock}
                onLoadMore={loadMoreBlocks}
              />
            </motion.div>
          )}

          {view.mode === 'block' && selectedBlock && (
            <motion.div
              key="block"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
            >
              <BlockDetail
                block={selectedBlock}
                onBack={goBack}
                onBlockClick={viewBlock}
              />
            </motion.div>
          )}

          {view.mode === 'transaction' && selectedTx && (
            <motion.div
              key="transaction"
              initial={{ opacity: 0, x: 20 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -20 }}
            >
              <TransactionDetail
                transaction={selectedTx}
                onBack={goBack}
                onBlockClick={viewBlock}
              />
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </Layout>
  )
}

// Block list component
function BlockList({
  blocks,
  loading,
  loadingMore,
  onBlockClick,
  onLoadMore,
}: {
  blocks: Block[]
  loading: boolean
  loadingMore: boolean
  onBlockClick: (block: Block) => void
  onLoadMore: () => void
}) {
  if (loading && blocks.length === 0) {
    return (
      <Card>
        <CardContent className="flex items-center justify-center py-20">
          <Loader2 className="h-8 w-8 animate-spin text-[--color-pulse]" />
        </CardContent>
      </Card>
    )
  }

  if (blocks.length === 0) {
    return (
      <Card>
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
    <Card>
      <CardHeader>
        <div className="flex items-center gap-2">
          <Blocks className="h-4 w-4 text-[--color-pulse]" />
          <CardTitle>Recent Blocks</CardTitle>
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
              onClick={() => onBlockClick(block)}
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
            onClick={onLoadMore}
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

// Block detail component
function BlockDetail({
  block,
  onBack,
  onBlockClick,
}: {
  block: Block
  onBack: () => void
  onBlockClick: (hashOrHeight: string | number) => void
}) {
  return (
    <div className="space-y-4">
      {/* Back button */}
      <Button variant="ghost" onClick={onBack}>
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
              <p className="mt-1 font-mono text-xs text-[--color-dim]">
                {block.hash}
              </p>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          <div className="grid gap-4 sm:grid-cols-2">
            <DetailRow label="Height" value={block.height.toLocaleString()} />
            <DetailRow label="Timestamp" value={formatTime(block.timestamp)} />
            <DetailRow
              label="Transactions"
              value={block.transactionCount.toString()}
            />
            <DetailRow
              label="Size"
              value={`${(block.size / 1024).toFixed(2)} KB`}
            />
            <DetailRow
              label="Reward"
              value={`${formatAmount(block.reward)} BTH`}
              valueClass="text-[--color-success]"
            />
            <DetailRow
              label="Difficulty"
              value={formatDifficulty(block.difficulty)}
            />
            <div className="col-span-2">
              <DetailRow
                label="Previous Block"
                value={formatHash(block.previousHash, 16)}
                mono
                onClick={
                  block.previousHash !==
                  '0000000000000000000000000000000000000000000000000000000000000000'
                    ? () => onBlockClick(block.previousHash)
                    : undefined
                }
              />
            </div>
            {block.miner && (
              <div className="col-span-2">
                <DetailRow label="Miner" value={formatHash(block.miner, 16)} mono />
              </div>
            )}
          </div>
        </CardContent>
      </Card>

      {/* Transactions in block - placeholder */}
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
              No transactions in this block (mining reward only)
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

// Transaction detail component
function TransactionDetail({
  transaction,
  onBack,
  onBlockClick,
}: {
  transaction: Transaction
  onBack: () => void
  onBlockClick: (heightOrHash: number | string) => void
}) {
  const TypeIcon =
    transaction.type === 'send'
      ? ArrowUpRight
      : transaction.type === 'receive'
        ? ArrowDownLeft
        : Pickaxe

  const typeColors = {
    send: 'text-[--color-danger]',
    receive: 'text-[--color-success]',
    mining: 'text-[--color-pulse]',
  }

  return (
    <div className="space-y-4">
      {/* Back button */}
      <Button variant="ghost" onClick={onBack}>
        <ArrowLeft className="mr-2 h-4 w-4" />
        Back to blocks
      </Button>

      {/* Transaction header */}
      <Card>
        <CardHeader>
          <div className="flex items-center gap-3">
            <div
              className={`flex h-12 w-12 items-center justify-center rounded-lg ${
                transaction.type === 'send'
                  ? 'bg-[--color-danger]/10'
                  : transaction.type === 'receive'
                    ? 'bg-[--color-success]/10'
                    : 'bg-[--color-pulse]/10'
              }`}
            >
              <TypeIcon className={`h-6 w-6 ${typeColors[transaction.type]}`} />
            </div>
            <div>
              <CardTitle className="capitalize">{transaction.type}</CardTitle>
              <p className="mt-1 font-mono text-xs text-[--color-dim]">
                {transaction.id}
              </p>
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
            <DetailRow
              label="Fee"
              value={`${formatAmount(transaction.fee)} BTH`}
            />
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
            <DetailRow
              label="Confirmations"
              value={transaction.confirmations.toString()}
            />
            <DetailRow
              label="Privacy"
              value={transaction.privacyLevel}
              valueClass={
                transaction.privacyLevel === 'ring'
                  ? 'text-[--color-success]'
                  : 'text-[--color-ghost]'
              }
            />
            <DetailRow label="Time" value={formatTime(transaction.timestamp)} />
            {transaction.blockHeight && (
              <DetailRow
                label="Block"
                value={`#${transaction.blockHeight}`}
                onClick={() => onBlockClick(transaction.blockHeight!)}
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

// Helper component for detail rows
function DetailRow({
  label,
  value,
  valueClass,
  mono,
  onClick,
}: {
  label: string
  value: string
  valueClass?: string
  mono?: boolean
  onClick?: () => void
}) {
  return (
    <div>
      <p className="text-xs uppercase tracking-wider text-[--color-dim]">
        {label}
      </p>
      <p
        className={`mt-1 text-sm ${mono ? 'font-mono' : ''} ${valueClass || 'text-[--color-light]'} ${
          onClick
            ? 'cursor-pointer hover:text-[--color-pulse] hover:underline'
            : ''
        }`}
        onClick={onClick}
      >
        {value}
      </p>
    </div>
  )
}
