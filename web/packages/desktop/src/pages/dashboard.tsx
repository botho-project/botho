import { Layout } from '../components/layout'
import { Card, CardHeader, CardTitle, CardContent } from '@botho/ui'
import { motion, AnimatePresence } from 'motion/react'
import {
  Blocks,
  Clock,
  Cpu,
  Database,
  RefreshCw,
  TrendingUp,
  Users,
} from 'lucide-react'
import { useNetworkStats } from '../hooks/useNetworkStats'
import { useConnection } from '../contexts/connection'

function formatNumber(n: number): string {
  return new Intl.NumberFormat().format(n)
}

function formatHashRate(hashRate: string | number): string {
  const rate = typeof hashRate === 'string' ? parseFloat(hashRate) : hashRate
  if (isNaN(rate)) return hashRate.toString()
  if (rate >= 1e12) return `${(rate / 1e12).toFixed(2)} TH/s`
  if (rate >= 1e9) return `${(rate / 1e9).toFixed(2)} GH/s`
  if (rate >= 1e6) return `${(rate / 1e6).toFixed(2)} MH/s`
  if (rate >= 1e3) return `${(rate / 1e3).toFixed(2)} KH/s`
  return `${rate.toFixed(2)} H/s`
}

function formatDifficulty(difficulty: bigint): string {
  const n = Number(difficulty)
  if (n >= 1e12) return `${(n / 1e12).toFixed(2)}T`
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)}G`
  if (n >= 1e6) return `${(n / 1e6).toFixed(2)}M`
  if (n >= 1e3) return `${(n / 1e3).toFixed(2)}K`
  return n.toString()
}

function formatRelativeTime(timestamp: number): string {
  const now = Date.now()
  const diff = now - timestamp
  const seconds = Math.floor(diff / 1000)
  const minutes = Math.floor(seconds / 60)
  const hours = Math.floor(minutes / 60)
  const days = Math.floor(hours / 24)

  if (seconds < 60) return 'just now'
  if (minutes < 60) return `${minutes}m ago`
  if (hours < 24) return `${hours}h ago`
  return `${days}d ago`
}

function formatBlockTime(timestamp: number): string {
  return formatRelativeTime(timestamp * 1000)
}

function truncateHash(hash: string): string {
  return `${hash.slice(0, 8)}...${hash.slice(-8)}`
}

export function DashboardPage() {
  const { connectedNode } = useConnection()
  const { stats, recentBlocks, isLoading, error, lastUpdated, refresh } = useNetworkStats()

  const statsItems = [
    {
      label: 'Block Height',
      value: stats ? formatNumber(stats.blockHeight) : '—',
      icon: Blocks,
      trend: null,
      loading: isLoading && !stats,
    },
    {
      label: 'Hash Rate',
      value: stats ? formatHashRate(stats.hashRate) : '—',
      icon: Cpu,
      trend: null,
      loading: isLoading && !stats,
    },
    {
      label: 'Connected Peers',
      value: stats ? formatNumber(stats.connectedPeers) : '—',
      icon: Users,
      trend: null,
      loading: isLoading && !stats,
    },
    {
      label: 'Mempool',
      value: stats ? `${formatNumber(stats.mempoolSize)} txs` : '—',
      icon: Database,
      trend: null,
      loading: isLoading && !stats,
    },
  ]

  return (
    <Layout title="Dashboard" subtitle="Network overview">
      <div className="space-y-6">
        {/* Connection status */}
        {error && (
          <div className="rounded-lg bg-red-500/10 border border-red-500/20 p-4 text-red-400">
            {error}
          </div>
        )}

        {/* Stats grid */}
        <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
          {statsItems.map((stat, i) => (
            <motion.div
              key={stat.label}
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: i * 0.1 }}
            >
              <Card>
                <CardContent className="flex items-center gap-4">
                  <div className="flex h-12 w-12 items-center justify-center rounded-lg bg-[--color-pulse]/10">
                    <stat.icon className="h-6 w-6 text-[--color-pulse]" />
                  </div>
                  <div className="flex-1">
                    <p className="text-sm text-[--color-dim]">{stat.label}</p>
                    {stat.loading ? (
                      <div className="h-7 w-24 animate-pulse rounded bg-[--color-dim]/20" />
                    ) : (
                      <p className="font-display text-xl font-bold text-[--color-light]">
                        {stat.value}
                      </p>
                    )}
                    {stat.trend && (
                      <p className="flex items-center gap-1 text-xs text-[--color-success]">
                        <TrendingUp className="h-3 w-3" />
                        {stat.trend}
                      </p>
                    )}
                  </div>
                </CardContent>
              </Card>
            </motion.div>
          ))}
        </div>

        {/* Recent blocks */}
        <Card>
          <CardHeader>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Clock className="h-4 w-4 text-[--color-pulse]" />
                <CardTitle>Recent Blocks</CardTitle>
              </div>
              <div className="flex items-center gap-4">
                {lastUpdated && (
                  <span className="text-xs text-[--color-dim]">
                    Updated {formatRelativeTime(lastUpdated)}
                  </span>
                )}
                <button
                  onClick={refresh}
                  disabled={isLoading}
                  className="rounded p-1.5 text-[--color-dim] hover:bg-[--color-pulse]/10 hover:text-[--color-pulse] disabled:opacity-50 transition-colors"
                >
                  <RefreshCw className={`h-4 w-4 ${isLoading ? 'animate-spin' : ''}`} />
                </button>
              </div>
            </div>
          </CardHeader>
          <CardContent>
            {!connectedNode ? (
              <div className="text-center py-12 text-[--color-dim]">
                Connect to a node to see block data
              </div>
            ) : recentBlocks.length === 0 && isLoading ? (
              <div className="space-y-3">
                {[...Array(5)].map((_, i) => (
                  <div
                    key={i}
                    className="h-16 animate-pulse rounded-lg bg-[--color-dim]/10"
                  />
                ))}
              </div>
            ) : recentBlocks.length === 0 ? (
              <div className="text-center py-12 text-[--color-dim]">
                No blocks yet
              </div>
            ) : (
              <div className="space-y-2">
                <AnimatePresence mode="popLayout">
                  {recentBlocks.map((block) => (
                    <motion.div
                      key={block.hash}
                      layout
                      initial={{ opacity: 0, x: -20 }}
                      animate={{ opacity: 1, x: 0 }}
                      exit={{ opacity: 0, x: 20 }}
                      className="flex items-center justify-between rounded-lg bg-[--color-dark]/50 p-4 border border-[--color-dim]/10"
                    >
                      <div className="flex items-center gap-4">
                        <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-pulse]/10 font-mono text-sm font-bold text-[--color-pulse]">
                          {block.height}
                        </div>
                        <div>
                          <p className="font-mono text-sm text-[--color-light]">
                            {truncateHash(block.hash)}
                          </p>
                          <p className="text-xs text-[--color-dim]">
                            {block.transactionCount} transactions • {formatBlockTime(block.timestamp)}
                          </p>
                        </div>
                      </div>
                      <div className="text-right">
                        <p className="text-sm text-[--color-light]">
                          {formatDifficulty(block.difficulty)}
                        </p>
                        <p className="text-xs text-[--color-dim]">difficulty</p>
                      </div>
                    </motion.div>
                  ))}
                </AnimatePresence>
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </Layout>
  )
}
