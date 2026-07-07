import type { Block, Transaction } from '@botho/core'
import type { ClusterWealthEntry } from './wealth'

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

  /**
   * Get every tracked cluster's wealth + node-computed fee factor (#699).
   * Optional — when absent, the wealth-distribution tab shows an
   * "unavailable" state instead of breaking.
   */
  getAllClusterWealth?(): Promise<ClusterWealthEntry[]>
}

/**
 * Explorer view state
 */
export type ExplorerView =
  | { mode: 'list' }
  | { mode: 'block'; block: Block }
  | { mode: 'transaction'; transaction: Transaction }

/**
 * List-mode tabs: recent blocks, cluster-wealth distribution, lottery feed.
 */
export type ExplorerTab = 'blocks' | 'wealth' | 'lottery'

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

  /** Execute search. Optionally pass the query to search directly (avoids a
   *  race with the async searchQuery state update on rapid input + Enter). */
  search: (queryOverride?: string) => Promise<void>

  /** View a specific block */
  viewBlock: (blockOrHeightOrHash: Block | number | string) => Promise<void>

  /** View a specific transaction */
  viewTransaction: (tx: Transaction) => void

  /** Fetch a transaction by hash and view it (block-detail tx links, #699) */
  viewTransactionByHash: (txHash: string) => Promise<void>

  /** Go back to list view */
  goBack: () => void

  /** Load more blocks */
  loadMore: () => Promise<void>

  /** Refresh blocks */
  refresh: () => Promise<void>

  /** Active list-mode tab (#699) */
  activeTab: ExplorerTab

  /** Switch the list-mode tab (also returns to the list view) */
  setActiveTab: (tab: ExplorerTab) => void

  /** Cluster-wealth entries; null until the first fetch resolves */
  clusterWealth: ClusterWealthEntry[] | null

  /** Cluster-wealth fetch in flight */
  wealthLoading: boolean

  /** Cluster-wealth fetch error */
  wealthError: string | null

  /** Whether the data source supports cluster-wealth queries */
  wealthSupported: boolean

  /** Refresh the cluster-wealth data */
  refreshWealth: () => Promise<void>
}
