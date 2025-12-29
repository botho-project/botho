import {
  createContext,
  useContext,
  useState,
  useCallback,
  type ReactNode,
} from 'react'
import type { MiningStats, MiningStatus } from '@botho/core'

interface MiningContextValue {
  stats: MiningStats
  start: () => Promise<void>
  stop: () => Promise<void>
  pause: () => Promise<void>
}

const MiningContext = createContext<MiningContextValue | null>(null)

const initialStats: MiningStats = {
  status: 'idle',
  hashRate: 0,
  blocksFound: 0,
  totalRewards: 0n,
  currentDifficulty: 0n,
}

export function MiningProvider({ children }: { children: ReactNode }) {
  const [stats, setStats] = useState<MiningStats>(initialStats)

  const start = useCallback(async () => {
    // TODO: Call Tauri command to start mining
    setStats((s) => ({ ...s, status: 'mining' }))
  }, [])

  const stop = useCallback(async () => {
    // TODO: Call Tauri command to stop mining
    setStats((s) => ({ ...s, status: 'idle' }))
  }, [])

  const pause = useCallback(async () => {
    // TODO: Call Tauri command to pause mining
    setStats((s) => ({ ...s, status: 'paused' }))
  }, [])

  return (
    <MiningContext.Provider value={{ stats, start, stop, pause }}>
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
