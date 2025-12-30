import { Layout } from '../components/layout'
import { Card, CardHeader, CardTitle, CardContent, Button } from '@botho/ui'
import { useMinting } from '../contexts/minting'
import { motion } from 'motion/react'
import {
  Cpu,
  Play,
  Pause,
  Square,
  TrendingUp,
  Zap,
  Coins,
} from 'lucide-react'

export function MintingPage() {
  const { stats, start, stop, pause } = useMinting()

  const isActive = stats.status === 'minting'
  const isPaused = stats.status === 'paused'

  return (
    <Layout title="Minting" subtitle="Contribute to the network">
      <div className="grid gap-6 lg:grid-cols-3">
        {/* Minting controls */}
        <Card className="lg:col-span-2">
          <CardHeader>
            <div className="flex items-center gap-2">
              <Cpu className="h-4 w-4 text-[--color-pulse]" />
              <CardTitle>Minting Status</CardTitle>
            </div>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col items-center py-8">
              <motion.div
                animate={{
                  scale: isActive ? [1, 1.1, 1] : 1,
                }}
                transition={{
                  duration: 1,
                  repeat: isActive ? Infinity : 0,
                }}
                className={`flex h-32 w-32 items-center justify-center rounded-full ${
                  isActive
                    ? 'bg-[--color-pulse]/20 glow'
                    : isPaused
                    ? 'bg-[--color-warning]/20'
                    : 'bg-[--color-slate]'
                }`}
              >
                <Cpu
                  className={`h-16 w-16 ${
                    isActive
                      ? 'text-[--color-pulse]'
                      : isPaused
                      ? 'text-[--color-warning]'
                      : 'text-[--color-dim]'
                  }`}
                />
              </motion.div>

              <p className="mt-6 font-display text-2xl font-bold text-[--color-light]">
                {isActive ? 'Minting Active' : isPaused ? 'Minting Paused' : 'Minting Stopped'}
              </p>

              {isActive && (
                <p className="mt-2 text-lg text-[--color-pulse]">
                  {stats.hashRate.toLocaleString()} H/s
                </p>
              )}

              <div className="mt-8 flex gap-4">
                {!isActive && (
                  <Button onClick={start}>
                    <Play className="h-4 w-4" />
                    Start Minting
                  </Button>
                )}
                {isActive && (
                  <>
                    <Button variant="secondary" onClick={pause}>
                      <Pause className="h-4 w-4" />
                      Pause
                    </Button>
                    <Button variant="danger" onClick={stop}>
                      <Square className="h-4 w-4" />
                      Stop
                    </Button>
                  </>
                )}
                {isPaused && (
                  <>
                    <Button onClick={start}>
                      <Play className="h-4 w-4" />
                      Resume
                    </Button>
                    <Button variant="danger" onClick={stop}>
                      <Square className="h-4 w-4" />
                      Stop
                    </Button>
                  </>
                )}
              </div>
            </div>
          </CardContent>
        </Card>

        {/* Minting stats */}
        <div className="space-y-6">
          <Card>
            <CardContent className="space-y-4">
              <div className="flex items-center gap-3">
                <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-success]/10">
                  <Coins className="h-5 w-5 text-[--color-success]" />
                </div>
                <div>
                  <p className="text-sm text-[--color-dim]">Blocks Found</p>
                  <p className="font-display text-xl font-bold text-[--color-light]">
                    {stats.blocksFound}
                  </p>
                </div>
              </div>

              <div className="flex items-center gap-3">
                <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-pulse]/10">
                  <TrendingUp className="h-5 w-5 text-[--color-pulse]" />
                </div>
                <div>
                  <p className="text-sm text-[--color-dim]">Total Rewards</p>
                  <p className="font-display text-xl font-bold text-[--color-light]">
                    {Number(stats.totalRewards / 100000000n).toFixed(2)} BTH
                  </p>
                </div>
              </div>

              <div className="flex items-center gap-3">
                <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-[--color-purple]/10">
                  <Zap className="h-5 w-5 text-[--color-purple]" />
                </div>
                <div>
                  <p className="text-sm text-[--color-dim]">Difficulty</p>
                  <p className="font-display text-xl font-bold text-[--color-light]">
                    {stats.currentDifficulty.toString()}
                  </p>
                </div>
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardContent>
              <p className="text-sm text-[--color-ghost]">
                Minting contributes to network security and earns you BTH rewards.
                The difficulty adjusts to maintain ~60 second block times.
              </p>
            </CardContent>
          </Card>
        </div>
      </div>
    </Layout>
  )
}
