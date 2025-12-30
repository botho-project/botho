import { useMemo } from 'react'
import { Link } from 'react-router-dom'
import { Logo } from '@botho/ui'
import { ExplorerProvider, Explorer, type ExplorerDataSource } from '@botho/features'
import { useWallet, useAdapter } from '../contexts/wallet'
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
    <div className="min-h-screen bg-[--color-void]">
      {/* Header */}
      <header className="sticky top-0 z-40 border-b border-[--color-steel] bg-[--color-void]/80 backdrop-blur-xl">
        <div className="mx-auto flex h-16 max-w-6xl items-center justify-between px-6">
          <div className="flex items-center gap-4">
            <Link
              to="/"
              className="flex items-center gap-2 text-sm text-[--color-ghost] transition-colors hover:text-[--color-light]"
            >
              <ArrowLeft className="h-4 w-4" />
              Back
            </Link>
            <div className="h-6 w-px bg-[--color-steel]" />
            <Logo />
          </div>
          <div>
            <h1 className="font-display text-xl font-bold text-[--color-light]">
              Block Explorer
            </h1>
          </div>
        </div>
      </header>

      {/* Main content */}
      <main className="mx-auto max-w-6xl p-6">
        <ExplorerProvider dataSource={dataSource} isReady={isConnected}>
          <Explorer
            isConnected={isConnected}
            notConnectedMessage={
              <>
                <p className="mt-4 text-lg text-[--color-ghost]">
                  Connecting to network...
                </p>
                <p className="mt-2 text-sm text-[--color-dim]">
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
