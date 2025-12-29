import { Layout } from '@/components/layout'
import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card'
import { useMining } from '@/contexts/mining'
import { motion } from 'motion/react'
import {
  Activity,
  Cpu,
  Flame,
  Gauge,
  Hash,
  Pause,
  Play,
  TrendingUp,
  Zap,
  Clock,
  Award,
  Target,
  BarChart3,
} from 'lucide-react'
import { cn, formatNumber, timeAgo } from '@/lib/utils'

// Generate mock chart data
function generateHashRateData(points: number = 24) {
  const data: number[] = []
  let value = 1200
  for (let i = 0; i < points; i++) {
    value += (Math.random() - 0.5) * 200
    value = Math.max(800, Math.min(1600, value))
    data.push(Math.round(value))
  }
  return data
}

function generateDifficultyData(points: number = 24) {
  const data: number[] = []
  let value = 2_200_000
  for (let i = 0; i < points; i++) {
    value += (Math.random() - 0.3) * 100_000
    value = Math.max(1_800_000, Math.min(2_800_000, value))
    data.push(Math.round(value))
  }
  return data
}

const hashRateHistory = generateHashRateData()
const difficultyHistory = generateDifficultyData()

function MiniChart({ data, color, height = 40 }: { data: number[]; color: string; height?: number }) {
  const max = Math.max(...data)
  const min = Math.min(...data)
  const range = max - min || 1

  const points = data
    .map((v, i) => {
      const x = (i / (data.length - 1)) * 100
      const y = height - ((v - min) / range) * height
      return `${x},${y}`
    })
    .join(' ')

  return (
    <svg width="100%" height={height} className="overflow-visible">
      <defs>
        <linearGradient id={`gradient-${color}`} x1="0%" y1="0%" x2="0%" y2="100%">
          <stop offset="0%" stopColor={`var(--color-${color})`} stopOpacity="0.3" />
          <stop offset="100%" stopColor={`var(--color-${color})`} stopOpacity="0" />
        </linearGradient>
      </defs>
      <polygon
        points={`0,${height} ${points} 100,${height}`}
        fill={`url(#gradient-${color})`}
      />
      <polyline
        points={points}
        fill="none"
        stroke={`var(--color-${color})`}
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}

export function MiningPage() {
  const {
    isRunning,
    threads,
    maxThreads,
    intensity,
    stats,
    recentRewards,
    rewards24h,
    rewards7d,
    rewards30d,
    rewardsTotal,
    startMining,
    stopMining,
    setThreads,
    setIntensity,
  } = useMining()

  const formatHashRate = (rate: number) => {
    if (rate >= 1_000_000_000) return `${(rate / 1_000_000_000).toFixed(2)} GH/s`
    if (rate >= 1_000_000) return `${(rate / 1_000_000).toFixed(2)} MH/s`
    if (rate >= 1_000) return `${(rate / 1_000).toFixed(2)} kH/s`
    return `${rate} H/s`
  }

  const formatDifficulty = (diff: number) => {
    if (diff >= 1_000_000_000) return `${(diff / 1_000_000_000).toFixed(2)}B`
    if (diff >= 1_000_000) return `${(diff / 1_000_000).toFixed(2)}M`
    if (diff >= 1_000) return `${(diff / 1_000).toFixed(2)}K`
    return diff.toString()
  }

  return (
    <Layout title="Mining" subtitle="Transaction-level proof-of-work mining">
      <div className="space-y-6">
        {/* Main mining control panel */}
        <div className="grid grid-cols-3 gap-6">
          {/* Mining controls */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            className="relative overflow-hidden rounded-2xl border border-[--color-steel] bg-gradient-to-br from-[--color-abyss] to-[--color-slate] p-6"
          >
            <div className="absolute -right-16 -top-16 h-48 w-48 rounded-full bg-[--color-pulse]/5 blur-3xl" />

            <div className="relative">
              {/* Status indicator */}
              <div className="flex items-center gap-3">
                <div className={cn(
                  'flex h-12 w-12 items-center justify-center rounded-xl',
                  isRunning ? 'bg-[--color-success]/20' : 'bg-[--color-slate]'
                )}>
                  {isRunning ? (
                    <Flame className="h-6 w-6 text-[--color-success] animate-pulse" />
                  ) : (
                    <Cpu className="h-6 w-6 text-[--color-dim]" />
                  )}
                </div>
                <div>
                  <h3 className="font-display text-lg font-bold text-[--color-light]">
                    {isRunning ? 'Mining Active' : 'Mining Paused'}
                  </h3>
                  <p className="text-sm text-[--color-dim]">
                    {isRunning ? `${threads} threads @ ${intensity} intensity` : 'Click to start mining'}
                  </p>
                </div>
              </div>

              {/* Start/Stop button */}
              <button
                onClick={isRunning ? stopMining : startMining}
                className={cn(
                  'mt-6 flex w-full items-center justify-center gap-2 rounded-xl px-4 py-4 font-display text-lg font-semibold transition-all',
                  isRunning
                    ? 'bg-[--color-danger]/20 text-[--color-danger] hover:bg-[--color-danger]/30'
                    : 'bg-[--color-success] text-[--color-void] hover:bg-[--color-success]/90 hover:shadow-[0_0_30px_rgba(34,197,94,0.3)]'
                )}
              >
                {isRunning ? (
                  <>
                    <Pause className="h-5 w-5" />
                    Stop Mining
                  </>
                ) : (
                  <>
                    <Play className="h-5 w-5" />
                    Start Mining
                  </>
                )}
              </button>

              {/* Thread control */}
              <div className="mt-6">
                <div className="flex items-center justify-between text-sm">
                  <span className="text-[--color-dim]">CPU Threads</span>
                  <span className="font-mono text-[--color-light]">{threads} / {maxThreads}</span>
                </div>
                <input
                  type="range"
                  min={1}
                  max={maxThreads}
                  value={threads}
                  onChange={(e) => setThreads(parseInt(e.target.value))}
                  className="mt-2 w-full accent-[--color-pulse]"
                />
              </div>

              {/* Intensity control */}
              <div className="mt-4">
                <span className="text-sm text-[--color-dim]">Intensity</span>
                <div className="mt-2 grid grid-cols-3 gap-2">
                  {(['low', 'medium', 'high'] as const).map((level) => (
                    <button
                      key={level}
                      onClick={() => setIntensity(level)}
                      className={cn(
                        'rounded-lg px-3 py-2 text-sm font-medium capitalize transition-all',
                        intensity === level
                          ? 'bg-[--color-pulse] text-[--color-void]'
                          : 'bg-[--color-slate] text-[--color-ghost] hover:bg-[--color-steel]'
                      )}
                    >
                      {level}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          </motion.div>

          {/* Hash rate display */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.05 }}
            className="rounded-2xl border border-[--color-steel] bg-[--color-abyss]/80 p-6"
          >
            <div className="flex items-center gap-2">
              <Hash className="h-4 w-4 text-[--color-pulse]" />
              <span className="text-sm font-medium text-[--color-dim]">Hash Rate</span>
            </div>

            <div className="mt-4">
              <div className="flex items-baseline gap-2">
                <motion.span
                  key={stats.hashRate}
                  initial={{ opacity: 0, y: 10 }}
                  animate={{ opacity: 1, y: 0 }}
                  className="font-display text-4xl font-bold text-[--color-light]"
                >
                  {formatHashRate(stats.hashRate)}
                </motion.span>
                <span className="text-sm text-[--color-dim]">local</span>
              </div>
              <p className="mt-1 text-sm text-[--color-ghost]">
                Network: {formatHashRate(stats.networkHashRate)}
              </p>
            </div>

            <div className="mt-6">
              <MiniChart data={hashRateHistory} color="pulse" height={60} />
              <p className="mt-2 text-center text-xs text-[--color-dim]">Last 24 hours</p>
            </div>
          </motion.div>

          {/* Difficulty display */}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.1 }}
            className="rounded-2xl border border-[--color-steel] bg-[--color-abyss]/80 p-6"
          >
            <div className="flex items-center gap-2">
              <Target className="h-4 w-4 text-[--color-warning]" />
              <span className="text-sm font-medium text-[--color-dim]">Difficulty</span>
            </div>

            <div className="mt-4">
              <span className="font-display text-4xl font-bold text-[--color-light]">
                {formatDifficulty(stats.difficulty)}
              </span>
              <p className="mt-1 text-sm text-[--color-ghost]">
                Target: {stats.targetTxPerBlock} mining TX/block
              </p>
            </div>

            <div className="mt-6">
              <MiniChart data={difficultyHistory} color="warning" height={60} />
              <p className="mt-2 text-center text-xs text-[--color-dim]">Last 24 hours</p>
            </div>
          </motion.div>
        </div>

        {/* Stats grid */}
        <div className="grid grid-cols-5 gap-4">
          {[
            { label: 'Blocks Found', value: stats.blocksFound, icon: Award, color: 'success' },
            { label: '24h Rewards', value: `${rewards24h.toFixed(2)} CAD`, icon: Clock, color: 'pulse' },
            { label: '7d Rewards', value: `${rewards7d.toFixed(2)} CAD`, icon: BarChart3, color: 'purple' },
            { label: '30d Rewards', value: `${rewards30d.toFixed(2)} CAD`, icon: TrendingUp, color: 'warning' },
            { label: 'Total Earned', value: `${formatNumber(rewardsTotal)} CAD`, icon: Zap, color: 'success' },
          ].map((stat, i) => (
            <motion.div
              key={stat.label}
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ delay: 0.15 + i * 0.03 }}
              className="rounded-xl border border-[--color-steel] bg-[--color-abyss]/80 p-4"
            >
              <div className="flex items-center gap-2">
                <stat.icon className={cn('h-4 w-4', `text-[--color-${stat.color}]`)} />
                <span className="text-xs text-[--color-dim]">{stat.label}</span>
              </div>
              <p className="mt-1 font-display text-xl font-bold text-[--color-light]">
                {stat.value}
              </p>
            </motion.div>
          ))}
        </div>

        {/* Recent rewards and performance */}
        <div className="grid grid-cols-3 gap-6">
          {/* Recent rewards */}
          <div className="col-span-2">
            <Card>
              <CardHeader>
                <div className="flex items-center gap-2">
                  <Award className="h-4 w-4 text-[--color-success]" />
                  <CardTitle>Recent Mining Rewards</CardTitle>
                </div>
              </CardHeader>
              <CardContent className="p-0">
                <div className="divide-y divide-[--color-steel]">
                  {recentRewards.map((reward, i) => (
                    <motion.div
                      key={reward.id}
                      initial={{ opacity: 0, x: -20 }}
                      animate={{ opacity: 1, x: 0 }}
                      transition={{ delay: i * 0.03 }}
                      className="flex items-center justify-between px-5 py-4 transition-colors hover:bg-[--color-slate]/50"
                    >
                      <div className="flex items-center gap-4">
                        <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-success]/10">
                          <Zap className="h-5 w-5 text-[--color-success]" />
                        </div>
                        <div>
                          <div className="flex items-center gap-2">
                            <span className="font-medium text-[--color-light]">
                              Block #{reward.blockHeight.toLocaleString()}
                            </span>
                          </div>
                          <div className="flex items-center gap-2 text-xs text-[--color-dim]">
                            <span>{timeAgo(reward.timestamp)}</span>
                            <span>•</span>
                            <span className="font-mono">{reward.txHash}</span>
                          </div>
                        </div>
                      </div>
                      <div className="text-right">
                        <p className="font-mono text-lg font-medium text-[--color-success]">
                          +{reward.amount.toFixed(2)} CAD
                        </p>
                      </div>
                    </motion.div>
                  ))}
                </div>
              </CardContent>
            </Card>
          </div>

          {/* Performance tips / info */}
          <div className="space-y-6">
            <Card>
              <CardHeader>
                <div className="flex items-center gap-2">
                  <Gauge className="h-4 w-4 text-[--color-pulse]" />
                  <CardTitle>Performance</CardTitle>
                </div>
              </CardHeader>
              <CardContent className="space-y-4">
                <div>
                  <div className="flex justify-between text-sm">
                    <span className="text-[--color-dim]">Efficiency</span>
                    <span className="text-[--color-success]">Optimal</span>
                  </div>
                  <div className="mt-2 h-2 overflow-hidden rounded-full bg-[--color-slate]">
                    <motion.div
                      initial={{ width: 0 }}
                      animate={{ width: '87%' }}
                      transition={{ duration: 1, delay: 0.5 }}
                      className="h-full rounded-full bg-gradient-to-r from-[--color-pulse] to-[--color-success]"
                    />
                  </div>
                </div>

                <div className="rounded-lg bg-[--color-slate] p-3">
                  <div className="flex items-center gap-2">
                    <Activity className="h-4 w-4 text-[--color-pulse]" />
                    <span className="text-sm font-medium text-[--color-light]">RandomX</span>
                  </div>
                  <p className="mt-1 text-xs text-[--color-dim]">
                    CPU-optimized, ASIC-resistant proof-of-work algorithm
                  </p>
                </div>

                <div className="space-y-2 text-sm">
                  <div className="flex justify-between">
                    <span className="text-[--color-dim]">Algorithm</span>
                    <span className="text-[--color-light]">RandomX</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-[--color-dim]">Block Reward</span>
                    <span className="text-[--color-light]">~2.5 CAD</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-[--color-dim]">Mining Type</span>
                    <span className="text-[--color-purple]">Transaction-level</span>
                  </div>
                </div>
              </CardContent>
            </Card>

            <Card>
              <CardHeader>
                <div className="flex items-center gap-2">
                  <TrendingUp className="h-4 w-4 text-[--color-warning]" />
                  <CardTitle>Emission</CardTitle>
                </div>
              </CardHeader>
              <CardContent>
                <div className="space-y-3">
                  <div>
                    <div className="flex justify-between text-sm">
                      <span className="text-[--color-dim]">Current Reward</span>
                      <span className="text-[--color-light]">2.50 CAD/block</span>
                    </div>
                  </div>
                  <div>
                    <div className="flex justify-between text-sm">
                      <span className="text-[--color-dim]">Next Adjustment</span>
                      <span className="text-[--color-light]">~2,400 blocks</span>
                    </div>
                  </div>
                  <div>
                    <div className="flex justify-between text-sm">
                      <span className="text-[--color-dim]">Tail Emission</span>
                      <span className="text-[--color-ghost]">0.6 CAD/block</span>
                    </div>
                  </div>

                  <div className="mt-4 rounded-lg bg-[--color-slate] p-3">
                    <p className="text-xs text-[--color-dim]">
                      Smooth emission curve with tail emission ensures perpetual mining rewards while
                      controlling inflation.
                    </p>
                  </div>
                </div>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </Layout>
  )
}
