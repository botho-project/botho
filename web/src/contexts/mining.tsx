import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  type ReactNode,
} from 'react'
import { useConnection } from './connection'

export interface MiningReward {
  id: string
  blockHeight: number
  amount: number
  timestamp: number
  txHash: string
}

export interface MiningStats {
  hashRate: number // H/s
  networkHashRate: number // H/s
  difficulty: number
  targetTxPerBlock: number
  blocksFound: number
  totalRewards: number
}

interface MiningState {
  isRunning: boolean
  threads: number
  maxThreads: number
  intensity: 'low' | 'medium' | 'high'
  stats: MiningStats
  recentRewards: MiningReward[]
  rewards24h: number
  rewards7d: number
  rewards30d: number
  rewardsTotal: number
}

interface MiningContextValue extends MiningState {
  startMining: () => Promise<void>
  stopMining: () => Promise<void>
  setThreads: (threads: number) => void
  setIntensity: (intensity: 'low' | 'medium' | 'high') => void
}

const MiningContext = createContext<MiningContextValue | null>(null)

// Mock data for development - will be replaced with API calls
const mockRewards: MiningReward[] = [
  { id: 'r1', blockHeight: 1234590, amount: 2.50, timestamp: Date.now() / 1000 - 120, txHash: '0xabc123...' },
  { id: 'r2', blockHeight: 1234582, amount: 2.51, timestamp: Date.now() / 1000 - 1080, txHash: '0xdef456...' },
  { id: 'r3', blockHeight: 1234571, amount: 2.51, timestamp: Date.now() / 1000 - 2460, txHash: '0xghi789...' },
  { id: 'r4', blockHeight: 1234558, amount: 2.52, timestamp: Date.now() / 1000 - 3600, txHash: '0xjkl012...' },
  { id: 'r5', blockHeight: 1234542, amount: 2.52, timestamp: Date.now() / 1000 - 7200, txHash: '0xmno345...' },
  { id: 'r6', blockHeight: 1234521, amount: 2.53, timestamp: Date.now() / 1000 - 14400, txHash: '0xpqr678...' },
  { id: 'r7', blockHeight: 1234498, amount: 2.53, timestamp: Date.now() / 1000 - 28800, txHash: '0xstu901...' },
  { id: 'r8', blockHeight: 1234472, amount: 2.54, timestamp: Date.now() / 1000 - 43200, txHash: '0xvwx234...' },
]

export function MiningProvider({ children }: { children: ReactNode }) {
  const { connectedNode } = useConnection()
  const [isRunning, setIsRunning] = useState(false)
  const [threads, setThreadsState] = useState(4)
  const [maxThreads] = useState(navigator.hardwareConcurrency || 8)
  const [intensity, setIntensityState] = useState<'low' | 'medium' | 'high'>('medium')
  const [stats, setStats] = useState<MiningStats>({
    hashRate: 0,
    networkHashRate: 847_000_000,
    difficulty: 2_400_000,
    targetTxPerBlock: 5,
    blocksFound: 47,
    totalRewards: 1247.83,
  })
  const [recentRewards] = useState<MiningReward[]>(mockRewards)

  // Simulate hash rate when mining
  useEffect(() => {
    if (!isRunning) {
      setStats((s) => ({ ...s, hashRate: 0 }))
      return
    }

    const baseRate = threads * 310 // ~310 H/s per thread
    const intensityMultiplier = intensity === 'low' ? 0.5 : intensity === 'high' ? 1.2 : 1

    const interval = setInterval(() => {
      // Add some variance
      const variance = (Math.random() - 0.5) * 0.1
      const rate = baseRate * intensityMultiplier * (1 + variance)
      setStats((s) => ({ ...s, hashRate: Math.round(rate) }))
    }, 1000)

    return () => clearInterval(interval)
  }, [isRunning, threads, intensity])

  // Calculate reward summaries
  const now = Date.now() / 1000
  const rewards24h = recentRewards
    .filter((r) => now - r.timestamp < 86400)
    .reduce((sum, r) => sum + r.amount, 0)
  const rewards7d = recentRewards
    .filter((r) => now - r.timestamp < 604800)
    .reduce((sum, r) => sum + r.amount, 0)
  const rewards30d = recentRewards
    .filter((r) => now - r.timestamp < 2592000)
    .reduce((sum, r) => sum + r.amount, 0)

  const startMining = useCallback(async () => {
    if (!connectedNode) return

    // In production, this would call the API
    // await fetch(`http://${connectedNode.host}:${connectedNode.port}/api/mining/start`, { method: 'POST' })
    setIsRunning(true)
  }, [connectedNode])

  const stopMining = useCallback(async () => {
    if (!connectedNode) return

    // In production, this would call the API
    // await fetch(`http://${connectedNode.host}:${connectedNode.port}/api/mining/stop`, { method: 'POST' })
    setIsRunning(false)
  }, [connectedNode])

  const setThreads = useCallback((t: number) => {
    setThreadsState(Math.max(1, Math.min(maxThreads, t)))
  }, [maxThreads])

  const setIntensity = useCallback((i: 'low' | 'medium' | 'high') => {
    setIntensityState(i)
  }, [])

  return (
    <MiningContext.Provider
      value={{
        isRunning,
        threads,
        maxThreads,
        intensity,
        stats,
        recentRewards,
        rewards24h,
        rewards7d,
        rewards30d,
        rewardsTotal: stats.totalRewards,
        startMining,
        stopMining,
        setThreads,
        setIntensity,
      }}
    >
      {children}
    </MiningContext.Provider>
  )
}

export function useMining() {
  const context = useContext(MiningContext)
  if (!context) {
    throw new Error('useMining must be used within a MiningProvider')
  }
  return context
}
