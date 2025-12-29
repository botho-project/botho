import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export function formatNumber(num: number): string {
  if (num >= 1_000_000_000) {
    return (num / 1_000_000_000).toFixed(2) + 'B'
  }
  if (num >= 1_000_000) {
    return (num / 1_000_000).toFixed(2) + 'M'
  }
  if (num >= 1_000) {
    return (num / 1_000).toFixed(2) + 'K'
  }
  return num.toLocaleString()
}

export function formatHash(hash: string, chars = 8): string {
  if (hash.length <= chars * 2) return hash
  return `${hash.slice(0, chars)}...${hash.slice(-chars)}`
}

export function formatTimestamp(timestamp: number): string {
  const date = new Date(timestamp * 1000)
  return date.toLocaleString()
}

export function timeAgo(timestamp: number): string {
  const seconds = Math.floor(Date.now() / 1000 - timestamp)

  if (seconds < 60) return `${seconds}s ago`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`
  return `${Math.floor(seconds / 86400)}d ago`
}
