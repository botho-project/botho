// Context and hooks
export { ExplorerProvider, useExplorer } from './context'
export type { ExplorerProviderProps } from './context'

// Types
export type {
  ExplorerDataSource,
  ExplorerView,
  ExplorerTab,
  ExplorerContextValue,
} from './types'

// Components
export {
  Explorer,
  SearchBar,
  BlockList,
  BlockDetail,
  TransactionDetail,
  ErrorMessage,
  DetailRow,
  ClusterWealth,
  LotteryFeed,
} from './components'

export type {
  ExplorerProps,
  SearchBarProps,
  BlockListProps,
  BlockDetailProps,
  TransactionDetailProps,
  ErrorMessageProps,
  DetailRowProps,
  ClusterWealthProps,
  LotteryFeedProps,
} from './components'

// Utilities
export {
  formatTime,
  formatHash,
  formatAmount,
  formatDifficulty,
  formatSize,
  isValidHash,
  isValidBlockHeight,
  ZERO_HASH,
} from './utils'

// Wealth-distribution derivations (#699)
export {
  FACTOR_BANDS,
  PICO_PER_BTH,
  factorBand,
  formatFactor,
  wealthBucketIndex,
  bucketLabel,
  bucketClusters,
  summarizeWealth,
} from './wealth'
export type {
  ClusterWealthEntry,
  FactorBand,
  WealthBucket,
  WealthSummary,
} from './wealth'

// Lottery-feed derivations (#699)
export { selectLotteryBlocks } from './lottery'
