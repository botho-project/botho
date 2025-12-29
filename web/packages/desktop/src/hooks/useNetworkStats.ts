import { useState, useEffect, useCallback, useRef } from 'react'
import type { NetworkStats, Block } from '@botho/core'
import { useConnection } from '../contexts/connection'

interface NetworkStatsState {
  stats: NetworkStats | null
  recentBlocks: Block[]
  isLoading: boolean
  error: string | null
  lastUpdated: number | null
}

const POLL_INTERVAL = 10000 // 10 seconds

export function useNetworkStats() {
  const { adapter, connectedNode } = useConnection()
  const [state, setState] = useState<NetworkStatsState>({
    stats: null,
    recentBlocks: [],
    isLoading: true,
    error: null,
    lastUpdated: null,
  })

  const unsubscribeRef = useRef<(() => void) | null>(null)
  const pollIntervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  // Fetch stats from the node
  const fetchStats = useCallback(async () => {
    if (!adapter?.isConnected()) return

    try {
      const [stats, blocks] = await Promise.all([
        adapter.getNetworkStats(),
        adapter.getRecentBlocks({ limit: 10 }),
      ])

      setState((s) => ({
        ...s,
        stats,
        recentBlocks: blocks,
        isLoading: false,
        error: null,
        lastUpdated: Date.now(),
      }))
    } catch (err) {
      setState((s) => ({
        ...s,
        isLoading: false,
        error: err instanceof Error ? err.message : 'Failed to fetch stats',
      }))
    }
  }, [adapter])

  // Setup real-time subscriptions
  useEffect(() => {
    if (!adapter?.isConnected() || !connectedNode) {
      setState((s) => ({
        ...s,
        stats: null,
        recentBlocks: [],
        isLoading: false,
        error: null,
      }))
      return
    }

    // Initial fetch
    setState((s) => ({ ...s, isLoading: true }))
    fetchStats()

    // Subscribe to new blocks for real-time updates
    unsubscribeRef.current = adapter.onNewBlock((block) => {
      setState((s) => ({
        ...s,
        recentBlocks: [block, ...s.recentBlocks.slice(0, 9)],
        stats: s.stats
          ? {
              ...s.stats,
              blockHeight: block.height,
              difficulty: block.difficulty,
            }
          : null,
        lastUpdated: Date.now(),
      }))
    })

    // Poll for full stats periodically (hash rate, peers, mempool)
    pollIntervalRef.current = setInterval(fetchStats, POLL_INTERVAL)

    return () => {
      if (unsubscribeRef.current) {
        unsubscribeRef.current()
        unsubscribeRef.current = null
      }
      if (pollIntervalRef.current) {
        clearInterval(pollIntervalRef.current)
        pollIntervalRef.current = null
      }
    }
  }, [adapter, connectedNode, fetchStats])

  const refresh = useCallback(async () => {
    setState((s) => ({ ...s, isLoading: true }))
    await fetchStats()
  }, [fetchStats])

  return {
    ...state,
    refresh,
  }
}
