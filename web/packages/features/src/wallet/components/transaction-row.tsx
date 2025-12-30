import type { Transaction } from '@botho/core'
import { formatBTH } from '@botho/core'
import { motion } from 'motion/react'
import {
  ArrowDownLeft,
  ArrowUpRight,
  Check,
  ChevronRight,
  Clock,
  Sparkles,
  X,
} from 'lucide-react'
import { PrivacyBadge } from './privacy-badge'

export interface TransactionRowProps {
  /** Transaction data */
  transaction: Transaction
  /** Animation index for staggered entrance */
  index?: number
  /** Whether to show privacy badge */
  showPrivacy?: boolean
  /** Whether to show chevron for clickable rows */
  showChevron?: boolean
  /** Click handler */
  onClick?: (tx: Transaction) => void
  /** Custom class name */
  className?: string
}

/**
 * Single transaction row with icon, details, amount, and status.
 */
export function TransactionRow({
  transaction: tx,
  index = 0,
  showPrivacy = true,
  showChevron = true,
  onClick,
  className = '',
}: TransactionRowProps) {
  const isReceive = tx.type === 'receive' || tx.type === 'minting'
  const Icon = tx.type === 'minting' ? Sparkles : isReceive ? ArrowDownLeft : ArrowUpRight

  const statusConfig = {
    pending: { color: 'text-[--color-warning]', icon: Clock },
    confirmed: { color: 'text-[--color-success]', icon: Check },
    failed: { color: 'text-[--color-danger]', icon: X },
  }[tx.status]

  const StatusIcon = statusConfig.icon

  const iconBg =
    tx.type === 'minting'
      ? 'bg-[--color-purple]/20 text-[--color-purple]'
      : isReceive
        ? 'bg-[--color-success]/20 text-[--color-success]'
        : 'bg-[--color-danger]/20 text-[--color-danger]'

  const label =
    tx.type === 'minting' ? 'Minting Reward' : isReceive ? 'Received' : 'Sent'

  return (
    <motion.div
      initial={{ opacity: 0, x: -20 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ delay: index * 0.05 }}
      onClick={onClick ? () => onClick(tx) : undefined}
      className={`group flex items-center gap-4 rounded-lg border border-transparent bg-[--color-slate]/50 p-4 transition-all hover:border-[--color-steel] hover:bg-[--color-slate] ${onClick ? 'cursor-pointer' : ''} ${className}`}
    >
      {/* Icon */}
      <div
        className={`flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-lg ${iconBg}`}
      >
        <Icon className="h-5 w-5" />
      </div>

      {/* Details */}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="font-display font-medium text-[--color-light]">{label}</span>
          {showPrivacy && <PrivacyBadge level={tx.privacyLevel} />}
        </div>
        <div className="mt-0.5 flex items-center gap-2">
          <span className="max-w-[180px] truncate font-mono text-xs text-[--color-dim]">
            {tx.counterparty || tx.id}
          </span>
          <span className="text-[--color-muted]">•</span>
          <span className="text-xs text-[--color-dim]">
            {new Date(tx.timestamp * 1000).toLocaleDateString()}
          </span>
        </div>
      </div>

      {/* Amount & Status */}
      <div className="text-right">
        <div
          className={`font-mono font-semibold ${isReceive ? 'text-[--color-success]' : 'text-[--color-light]'}`}
        >
          {isReceive ? '+' : '-'}
          {formatBTH(tx.amount)} BTH
        </div>
        <div className={`flex items-center justify-end gap-1 text-xs ${statusConfig.color}`}>
          <StatusIcon className="h-3 w-3" />
          {tx.status === 'confirmed' ? `${tx.confirmations} conf` : tx.status}
        </div>
      </div>

      {/* Chevron */}
      {showChevron && (
        <ChevronRight className="h-4 w-4 text-[--color-dim] opacity-0 transition-opacity group-hover:opacity-100" />
      )}
    </motion.div>
  )
}
