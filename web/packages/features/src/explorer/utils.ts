/**
 * Format a timestamp as relative time or date
 */
export function formatTime(timestamp: number): string {
  const now = Math.floor(Date.now() / 1000)
  const diff = now - timestamp

  if (diff < 60) return `${diff}s ago`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`

  return new Date(timestamp * 1000).toLocaleDateString()
}

/**
 * Format a hash for display (truncated)
 */
export function formatHash(hash: string, length = 8): string {
  if (hash.length <= length * 2) return hash
  return `${hash.slice(0, length)}...${hash.slice(-length)}`
}

/**
 * Format amount (12 decimal places, like Monero)
 */
export function formatAmount(amount: bigint): string {
  const credits = Number(amount) / 1_000_000_000_000
  return credits.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 6,
  })
}

/**
 * Format difficulty as human readable
 */
export function formatDifficulty(difficulty: bigint): string {
  const num = Number(difficulty)
  if (num >= 1e12) return `${(num / 1e12).toFixed(2)}T`
  if (num >= 1e9) return `${(num / 1e9).toFixed(2)}G`
  if (num >= 1e6) return `${(num / 1e6).toFixed(2)}M`
  if (num >= 1e3) return `${(num / 1e3).toFixed(2)}K`
  return num.toString()
}

/**
 * Format file size
 */
export function formatSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(2)} MB`
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(2)} KB`
  return `${bytes} B`
}

/**
 * Check if a string is a valid hash (64 hex characters)
 */
export function isValidHash(str: string): boolean {
  return /^[0-9a-fA-F]{64}$/.test(str)
}

/**
 * Check if a string is a valid block height
 */
export function isValidBlockHeight(str: string): boolean {
  return /^\d+$/.test(str)
}

/**
 * Zero hash constant (used for genesis block's previous hash)
 */
export const ZERO_HASH = '0000000000000000000000000000000000000000000000000000000000000000'
