import type { Transaction } from '@botho/core'
import { formatBTH, formatRelativeTime, formatAbsoluteTime } from '@botho/core'
import { motion } from 'motion/react'
import {
  ArrowDownLeft,
  ArrowUpRight,
  Check,
  ChevronRight,
  Loader2,
  Sparkles,
  X,
} from 'lucide-react'
import { PrivacyBadge } from './privacy-badge'

/**
 * Number of confirmations a transaction needs before it is treated as fully
 * settled. Below this (but already in a block) we surface a "Confirming" state.
 */
const CONFIRMED_THRESHOLD = 6

export interface TransactionRowProps {
  /** Transaction data */
  transaction: Transaction
  /** Animation index for staggered entrance */
  index?: number
  /**
   * Whether to show a per-row privacy badge. Defaults to `false`: every Botho
   * transfer is private, so a per-transaction "Private" chip is redundant noise
   * in the wallet's transaction history.
   */
  showPrivacy?: boolean
  /** Whether to show chevron for clickable rows */
  showChevron?: boolean
  /**
   * Optional lookup that maps a counterparty address to a friendly display name
   * (e.g. an address-book contact). Return `undefined` when the address is not
   * known so the row falls back to the truncated address. Kept optional so
   * existing callers without an address book stay unchanged.
   */
  resolveName?: (address: string) => string | undefined
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
  showPrivacy = false,
  showChevron = true,
  resolveName,
  onClick,
  className = '',
}: TransactionRowProps) {
  const isReceive = tx.type === 'receive' || tx.type === 'minting'
  const Icon = tx.type === 'minting' ? Sparkles : isReceive ? ArrowDownLeft : ArrowUpRight

  // Treat an in-block-but-not-yet-deep transaction as "confirming". A pending
  // transaction is still propagating / unmined.
  const isConfirming =
    tx.status === 'confirmed' && tx.confirmations < CONFIRMED_THRESHOLD

  const statusConfig = {
    pending: { color: 'text-[--color-warning]', icon: Loader2, label: 'Pending', spin: true },
    confirming: { color: 'text-[--color-warning]', icon: Loader2, label: 'Confirming', spin: true },
    confirmed: {
      color: 'text-[--color-success]',
      icon: Check,
      label: `${tx.confirmations} conf`,
      spin: false,
    },
    failed: { color: 'text-[--color-danger]', icon: X, label: 'Failed', spin: false },
  }[tx.status === 'confirmed' && isConfirming ? 'confirming' : tx.status]

  const StatusIcon = statusConfig.icon

  const iconBg =
    tx.type === 'minting'
      ? 'bg-[--color-purple]/20 text-[--color-purple]'
      : isReceive
        ? 'bg-[--color-success]/20 text-[--color-success]'
        : 'bg-[--color-danger]/20 text-[--color-danger]'

  const label =
    tx.type === 'minting' ? 'Minting Reward' : isReceive ? 'Received' : 'Sent'

  // Prefer a friendly contact name for the counterparty, falling back to the
  // raw address (truncated via CSS). No fallback to tx.id: netted history
  // rows (#675) carry synthetic ids, and the ring hides the real
  // counterparty anyway — an empty slot beats a meaningless token.
  const counterpartyName = tx.counterparty ? resolveName?.(tx.counterparty) : undefined
  const counterparty = counterpartyName || tx.counterparty || ''

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
          {showPrivacy && <PrivacyBadge cryptoType={tx.cryptoType} />}
        </div>
        <div className="mt-0.5 flex items-center gap-2">
          {counterparty && (
            <>
              <span
                className={`max-w-[180px] truncate text-xs text-[--color-dim] ${counterpartyName ? '' : 'font-mono'}`}
                title={tx.counterparty}
              >
                {counterparty}
              </span>
              <span className="text-[--color-muted]">•</span>
            </>
          )}
          {/* A zero timestamp means "no wall-clock time known" (client-side
              history has only block heights, #675): show the height rather
              than fabricating a relative time. */}
          <span
            className="text-xs text-[--color-dim]"
            title={tx.timestamp > 0 ? formatAbsoluteTime(tx.timestamp) : undefined}
          >
            {tx.timestamp > 0
              ? formatRelativeTime(tx.timestamp)
              : tx.blockHeight != null
                ? `Block #${tx.blockHeight}`
                : tx.status === 'pending'
                  ? 'Pending'
                  : '—'}
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
        <div
          className={`flex items-center justify-end gap-1 text-xs ${statusConfig.color}`}
        >
          <StatusIcon className={`h-3 w-3 ${statusConfig.spin ? 'animate-spin' : ''}`} />
          {statusConfig.label}
        </div>
      </div>

      {/* Chevron */}
      {showChevron && (
        <ChevronRight className="h-4 w-4 text-[--color-dim] opacity-0 transition-opacity group-hover:opacity-100" />
      )}
    </motion.div>
  )
}
