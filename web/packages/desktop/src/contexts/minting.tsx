import {
  createContext,
  useContext,
  useState,
  useCallback,
  type ReactNode,
} from 'react'
import type { MintingStats } from '@botho/core'

interface MintingContextValue {
  stats: MintingStats
  start: () => Promise<void>
  stop: () => Promise<void>
  pause: () => Promise<void>
}

const MintingContext = createContext<MintingContextValue | null>(null)

const initialStats: MintingStats = {
  status: 'idle',
  hashRate: 0,
  blocksFound: 0,
  totalRewards: 0n,
  currentDifficulty: 0n,
}

export function MintingProvider({ children }: { children: ReactNode }) {
  const [stats, setStats] = useState<MintingStats>(initialStats)

  const start = useCallback(async () => {
    // TODO: Call Tauri command to start minting
    setStats((s) => ({ ...s, status: 'minting' }))
  }, [])

  const stop = useCallback(async () => {
    // TODO: Call Tauri command to stop minting
    setStats((s) => ({ ...s, status: 'idle' }))
  }, [])

  const pause = useCallback(async () => {
    // TODO: Call Tauri command to pause minting
    setStats((s) => ({ ...s, status: 'paused' }))
  }, [])

  return (
    <MintingContext.Provider value={{ stats, start, stop, pause }}>
      {children}
    </MintingContext.Provider>
  )
}

export function useMinting() {
  const context = useContext(MintingContext)
  if (!context) {
    throw new Error('useMinting must be used within a MintingProvider')
  }
  return context
}
