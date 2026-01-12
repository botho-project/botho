import type { ReactNode } from 'react'
import type { Balance } from '@botho/core'
import { formatBTH } from '@botho/core'
import { Card, CardContent } from '@botho/ui'
import { motion } from 'motion/react'
import { Wallet, Check, Copy, Wifi, WifiOff, RefreshCw } from 'lucide-react'
import { useCopyToClipboard } from '../hooks'

export interface BalanceCardProps {
  /** Current balance */
  balance: Balance | null
  /** Wallet address */
  address: string | null
  /** Whether balance is loading */
  isLoading?: boolean
  /** Whether connected to blockchain */
  isConnected?: boolean
  /** Whether currently syncing */
  isSyncing?: boolean
  /** Action buttons (Send, Receive, etc.) */
  actions?: ReactNode
  /** Whether to show the address (default: false) */
  showAddress?: boolean
  /** Custom class name */
  className?: string
}

/**
 * Card displaying wallet balance with address and actions.
 */
export function BalanceCard({
  balance,
  address,
  isLoading = false,
  isConnected = true,
  isSyncing = false,
  actions,
  showAddress = false,
  className = '',
}: BalanceCardProps) {
  const { copied, copy } = useCopyToClipboard()

  return (
    <motion.div initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }}>
      <Card className={`overflow-hidden ${className}`}>
        {/* Gradient background */}
        <div className="absolute inset-0 bg-gradient-to-br from-[--color-pulse]/5 via-transparent to-[--color-purple]/5" />

        <CardContent className="relative">
          <div className="flex flex-col gap-6 md:flex-row md:items-center md:justify-between">
            {/* Balance */}
            <div>
              <div className="flex items-center gap-2 text-sm text-[--color-ghost]">
                <Wallet className="h-4 w-4" />
                <span>Total Balance</span>
                <span className="mx-1 text-[--color-steel]">·</span>
                {isSyncing ? (
                  <span className="flex items-center gap-1 text-[--color-warning]">
                    <RefreshCw className="h-3 w-3 animate-spin" />
                    Syncing
                  </span>
                ) : isConnected ? (
                  <span className="flex items-center gap-1 text-[--color-success]">
                    <Wifi className="h-3 w-3" />
                    Synced
                  </span>
                ) : (
                  <span className="flex items-center gap-1 text-[--color-danger]">
                    <WifiOff className="h-3 w-3" />
                    Offline
                  </span>
                )}
              </div>
              <div className="mt-1 font-display text-4xl font-bold tracking-tight text-[--color-light]">
                {balance ? (
                  <motion.span
                    key={balance.total.toString()}
                    initial={{ opacity: 0, y: 10 }}
                    animate={{ opacity: 1, y: 0 }}
                  >
                    {formatBTH(balance.total)}
                    <span className="ml-2 text-xl text-[--color-ghost]">BTH</span>
                  </motion.span>
                ) : (
                  <span className="animate-pulse text-[--color-dim]">
                    {isLoading ? 'Loading...' : '—'}
                  </span>
                )}
              </div>

              {/* Sub-balances and wallet ID */}
              <div className="mt-3 flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
                {balance && (
                  <>
                    <div>
                      <span className="text-[--color-dim]">Available: </span>
                      <span className="font-mono text-[--color-success]">
                        {formatBTH(balance.available)}
                      </span>
                    </div>
                    {balance.pending > 0 && (
                      <div>
                        <span className="text-[--color-dim]">Pending: </span>
                        <span className="font-mono text-[--color-warning]">
                          {formatBTH(balance.pending)}
                        </span>
                      </div>
                    )}
                  </>
                )}
                {address && (
                  <button
                    onClick={() => copy(address)}
                    className="flex items-center gap-1.5 rounded-md px-2 py-0.5 text-[--color-dim] transition-colors hover:bg-[--color-steel] hover:text-[--color-ghost]"
                    title={address}
                  >
                    <code className="font-mono text-xs">
                      {address.slice(0, 6)}...{address.slice(-4)}
                    </code>
                    {copied ? (
                      <Check className="h-3 w-3 text-[--color-success]" />
                    ) : (
                      <Copy className="h-3 w-3" />
                    )}
                  </button>
                )}
              </div>
            </div>

            {/* Actions */}
            {actions && <div className="flex gap-3">{actions}</div>}
          </div>

          {/* Address (optional) */}
          {showAddress && address && (
            <div className="mt-6 flex items-center gap-2 rounded-lg border border-[--color-steel] bg-[--color-slate]/50 p-3">
              <span className="text-sm text-[--color-dim]">Address:</span>
              <code className="flex-1 truncate font-mono text-sm text-[--color-ghost]">
                {address}
              </code>
              <button
                onClick={() => copy(address)}
                className="rounded-md p-1.5 text-[--color-dim] transition-colors hover:bg-[--color-steel] hover:text-[--color-light]"
              >
                {copied ? (
                  <Check className="h-4 w-4 text-[--color-success]" />
                ) : (
                  <Copy className="h-4 w-4" />
                )}
              </button>
            </div>
          )}
        </CardContent>
      </Card>
    </motion.div>
  )
}
