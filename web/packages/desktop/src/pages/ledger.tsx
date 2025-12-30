import { useMemo } from 'react'
import { Layout } from '../components/layout'
import { useConnection } from '../contexts/connection'
import { ExplorerProvider, Explorer, type ExplorerDataSource } from '@botho/features'

export function LedgerPage() {
  const { adapter, connectedNode } = useConnection()

  // Create data source from adapter
  const dataSource = useMemo<ExplorerDataSource | null>(() => {
    if (!adapter) return null

    return {
      getRecentBlocks: (options) => adapter.getRecentBlocks(options),
      getBlock: (heightOrHash) => adapter.getBlock(heightOrHash),
      getTransaction: (txHash) => adapter.getTransaction(txHash),
      onNewBlock: (callback) => adapter.onNewBlock(callback),
    }
  }, [adapter])

  const isConnected = !!connectedNode && !!dataSource

  return (
    <Layout title="Ledger" subtitle="Browse blocks and transactions">
      {dataSource ? (
        <ExplorerProvider dataSource={dataSource} isReady={isConnected}>
          <Explorer isConnected={isConnected} />
        </ExplorerProvider>
      ) : (
        <Explorer isConnected={false} />
      )}
    </Layout>
  )
}
