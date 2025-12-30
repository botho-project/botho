import type { Block, Transaction } from '@botho/core'

/**
 * Explorer data source interface
 *
 * Implement this to connect the explorer to your data layer
 * (RPC adapter, mock data, etc.)
 */
export interface ExplorerDataSource {
  /** Get recent blocks */
  getRecentBlocks(options?: { limit?: number; startHeight?: number }): Promise<Block[]>

  /** Get a specific block by height or hash */
  getBlock(heightOrHash: number | string): Promise<Block | null>

  /** Get a transaction by hash */
  getTransaction(txHash: string): Promise<Transaction | null>

  /** Subscribe to new blocks (returns unsubscribe function) */
  onNewBlock?(callback: (block: Block) => void): () => void
}

/**
 * Explorer view state
 */
export type ExplorerView =
  | { mode: 'list' }
  | { mode: 'block'; block: Block }
  | { mode: 'transaction'; transaction: Transaction }

/**
 * Explorer context value
 */
export interface ExplorerContextValue {
  /** Current view state */
  view: ExplorerView

  /** List of blocks */
  blocks: Block[]

  /** Loading state */
  loading: boolean

  /** Loading more blocks */
  loadingMore: boolean

  /** Error message */
  error: string | null

  /** Search query */
  searchQuery: string

  /** Set search query */
  setSearchQuery: (query: string) => void

  /** Execute search */
  search: () => Promise<void>

  /** View a specific block */
  viewBlock: (blockOrHeightOrHash: Block | number | string) => Promise<void>

  /** View a specific transaction */
  viewTransaction: (tx: Transaction) => void

  /** Go back to list view */
  goBack: () => void

  /** Load more blocks */
  loadMore: () => Promise<void>

  /** Refresh blocks */
  refresh: () => Promise<void>
}
