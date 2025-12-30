import type { ReactNode } from 'react'
import { Card, CardContent } from '@botho/ui'
import { motion, AnimatePresence } from 'motion/react'
import { Database } from 'lucide-react'
import { useExplorer } from '../context'
import { SearchBar } from './search-bar'
import { ErrorMessage } from './error-message'
import { BlockList } from './block-list'
import { BlockDetail } from './block-detail'
import { TransactionDetail } from './transaction-detail'

export interface ExplorerProps {
  /** Whether the data source is connected/ready */
  isConnected?: boolean
  /** Message to show when not connected */
  notConnectedMessage?: ReactNode
  /** Custom class name */
  className?: string
}

/**
 * Complete blockchain explorer component
 *
 * Renders search bar, block list, block details, and transaction details
 * based on the current view state from ExplorerContext.
 */
export function Explorer({
  isConnected = true,
  notConnectedMessage,
  className,
}: ExplorerProps) {
  const { view } = useExplorer()

  // Not connected state
  if (!isConnected) {
    return (
      <div className={className}>
        <Card>
          <CardContent className="flex flex-col items-center justify-center py-20">
            <Database className="h-16 w-16 text-[--color-dim]" />
            {notConnectedMessage || (
              <>
                <p className="mt-4 text-lg text-[--color-ghost]">
                  Not connected to a node
                </p>
                <p className="mt-2 text-sm text-[--color-dim]">
                  Connect to a Botho node to browse the blockchain
                </p>
              </>
            )}
          </CardContent>
        </Card>
      </div>
    )
  }

  return (
    <div className={`space-y-6 ${className || ''}`}>
      {/* Search bar */}
      <SearchBar />

      {/* Error message */}
      <ErrorMessage />

      {/* Content based on view mode */}
      <AnimatePresence mode="wait">
        {view.mode === 'list' && (
          <motion.div
            key="list"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
          >
            <BlockList />
          </motion.div>
        )}

        {view.mode === 'block' && (
          <motion.div
            key="block"
            initial={{ opacity: 0, x: 20 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -20 }}
          >
            <BlockDetail block={view.block} />
          </motion.div>
        )}

        {view.mode === 'transaction' && (
          <motion.div
            key="transaction"
            initial={{ opacity: 0, x: 20 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -20 }}
          >
            <TransactionDetail transaction={view.transaction} />
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}
