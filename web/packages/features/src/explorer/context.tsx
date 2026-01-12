import { createContext, useContext, useState, useCallback, useEffect, type ReactNode } from 'react'
import type { Block, Transaction } from '@botho/core'
import type { ExplorerDataSource, ExplorerView, ExplorerContextValue } from './types'

const ExplorerContext = createContext<ExplorerContextValue | null>(null)

export interface ExplorerProviderProps {
  /** Data source for fetching blocks and transactions */
  dataSource: ExplorerDataSource
  /** Whether the data source is ready (e.g., connected) */
  isReady?: boolean
  /** Initial search query (e.g., from URL parameter) */
  initialQuery?: string
  /** Callback when view changes (for URL sync) */
  onViewChange?: (view: ExplorerView) => void
  /** Children */
  children: ReactNode
}

export function ExplorerProvider({ dataSource, isReady = true, initialQuery, onViewChange, children }: ExplorerProviderProps) {
  const [view, setViewInternal] = useState<ExplorerView>({ mode: 'list' })
  const [blocks, setBlocks] = useState<Block[]>([])
  const [loading, setLoading] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [searchQuery, setSearchQuery] = useState(initialQuery ?? '')
  const [initialQueryProcessed, setInitialQueryProcessed] = useState(false)

  // Wrapper to notify parent of view changes
  const setView = useCallback((newView: ExplorerView) => {
    setViewInternal(newView)
    onViewChange?.(newView)
  }, [onViewChange])

  // Load recent blocks
  const refresh = useCallback(async () => {
    if (!isReady) return

    setLoading(true)
    setError(null)

    try {
      const recentBlocks = await dataSource.getRecentBlocks({ limit: 20 })
      setBlocks(recentBlocks)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load blocks')
    } finally {
      setLoading(false)
    }
  }, [dataSource, isReady])

  // Load more blocks
  const loadMore = useCallback(async () => {
    if (!isReady || blocks.length === 0 || loadingMore) return

    setLoadingMore(true)
    try {
      const lastBlock = blocks[blocks.length - 1]
      const moreBlocks = await dataSource.getRecentBlocks({
        limit: 20,
        startHeight: lastBlock.height - 1,
      })
      setBlocks((prev) => [...prev, ...moreBlocks])
    } catch (err) {
      console.error('Failed to load more blocks:', err)
    } finally {
      setLoadingMore(false)
    }
  }, [dataSource, isReady, blocks, loadingMore])

  // Search by height or hash
  const search = useCallback(async () => {
    if (!isReady || !searchQuery.trim()) return

    setLoading(true)
    setError(null)

    const query = searchQuery.trim()

    try {
      // Check if it's a block height (number)
      if (/^\d+$/.test(query)) {
        const block = await dataSource.getBlock(parseInt(query, 10))
        if (block) {
          setView({ mode: 'block', block })
        } else {
          setError(`Block at height ${query} not found`)
        }
      }
      // Check if it's a hash (64 hex chars)
      else if (/^[0-9a-fA-F]{64}$/.test(query)) {
        // Try as block hash first
        const block = await dataSource.getBlock(query)
        if (block) {
          setView({ mode: 'block', block })
        } else {
          // Try as transaction hash
          const tx = await dataSource.getTransaction(query)
          if (tx) {
            setView({ mode: 'transaction', transaction: tx })
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
  }, [dataSource, isReady, searchQuery])

  // View a specific block
  const viewBlock = useCallback(
    async (blockOrHeightOrHash: Block | number | string) => {
      if (!isReady) return

      setLoading(true)
      setError(null)

      try {
        let block: Block | null
        if (typeof blockOrHeightOrHash === 'object') {
          block = blockOrHeightOrHash
        } else {
          block = await dataSource.getBlock(blockOrHeightOrHash)
        }

        if (block) {
          setView({ mode: 'block', block })
        } else {
          setError('Block not found')
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to load block')
      } finally {
        setLoading(false)
      }
    },
    [dataSource, isReady]
  )

  // View a transaction
  const viewTransaction = useCallback((transaction: Transaction) => {
    setView({ mode: 'transaction', transaction })
    setError(null)
  }, [])

  // Go back to list
  const goBack = useCallback(() => {
    setView({ mode: 'list' })
    setError(null)
  }, [])

  // Initial load
  useEffect(() => {
    if (isReady) {
      refresh()
    }
  }, [isReady, refresh])

  // Process initial query from URL parameter
  useEffect(() => {
    if (isReady && initialQuery && !initialQueryProcessed) {
      setInitialQueryProcessed(true)
      // Trigger search with the initial query
      search()
    }
  }, [isReady, initialQuery, initialQueryProcessed, search])

  // Subscribe to new blocks
  useEffect(() => {
    if (!isReady || !dataSource.onNewBlock) return

    const unsubscribe = dataSource.onNewBlock((block) => {
      setBlocks((prev) => {
        // Add new block at the beginning, remove duplicates
        const filtered = prev.filter((b) => b.hash !== block.hash)
        return [block, ...filtered].slice(0, 50)
      })
    })

    return unsubscribe
  }, [dataSource, isReady])

  const value: ExplorerContextValue = {
    view,
    blocks,
    loading,
    loadingMore,
    error,
    searchQuery,
    setSearchQuery,
    search,
    viewBlock,
    viewTransaction,
    goBack,
    loadMore,
    refresh,
  }

  return <ExplorerContext.Provider value={value}>{children}</ExplorerContext.Provider>
}

export function useExplorer(): ExplorerContextValue {
  const context = useContext(ExplorerContext)
  if (!context) {
    throw new Error('useExplorer must be used within an ExplorerProvider')
  }
  return context
}
