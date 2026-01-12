import { useMemo } from 'react'
import { Link } from 'react-router-dom'
import { Logo } from '@botho/ui'
import { ExplorerProvider, Explorer, type ExplorerDataSource } from '@botho/features'
import { useWallet, useAdapter } from '../contexts/wallet'
import { NetworkSelector } from '../components/NetworkSelector'
import { ArrowLeft } from 'lucide-react'

export function ExplorerPage() {
  const { isConnected } = useWallet()
  const adapter = useAdapter()

  // Create data source from adapter
  const dataSource = useMemo<ExplorerDataSource>(() => {
    return {
      getRecentBlocks: (options) => adapter.getRecentBlocks(options),
      getBlock: (heightOrHash) => adapter.getBlock(heightOrHash),
      getTransaction: (txHash) => adapter.getTransaction(txHash),
      onNewBlock: (callback) => adapter.onNewBlock(callback),
    }
  }, [adapter])

  return (
    <div className="min-h-screen">
      {/* Header */}
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">Block Explorer</span>
            <span className="font-display text-base font-semibold sm:hidden">Explorer</span>
          </Link>
          <NetworkSelector />
        </div>
      </header>

      {/* Main content */}
      <main className="py-6 sm:py-8 md:py-12 px-4 sm:px-6 max-w-6xl mx-auto">
        <ExplorerProvider dataSource={dataSource} isReady={isConnected}>
          <Explorer
            isConnected={isConnected}
            notConnectedMessage={
              <>
                <p className="mt-4 text-lg text-ghost">
                  Connecting to network...
                </p>
                <p className="mt-2 text-sm text-muted">
                  Please wait while we connect to Botho nodes
                </p>
              </>
            }
          />
        </ExplorerProvider>
      </main>
    </div>
  )
}
