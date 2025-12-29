import { BrowserRouter, Routes, Route } from 'react-router-dom'
import { Layout } from '@/components/layout'
import { SplashScreen } from '@/components/connection'
import { ConnectionProvider, useConnection } from '@/contexts/connection'
import { MiningProvider } from '@/contexts/mining'
import { DashboardPage } from '@/pages/dashboard'
import { WalletPage } from '@/pages/wallet'
import { LedgerPage } from '@/pages/ledger'
import { NetworkPage } from '@/pages/network'
import { MiningPage } from '@/pages/mining'

function AppRoutes() {
  const { connectedNode } = useConnection()

  if (!connectedNode) {
    return <SplashScreen />
  }

  return (
    <MiningProvider>
      <BrowserRouter>
        <Routes>
          <Route path="/" element={<DashboardPage />} />
          <Route path="/wallet" element={<WalletPage />} />
          <Route path="/ledger" element={<LedgerPage />} />
          <Route path="/blocks" element={<LedgerPage />} />
          <Route path="/transactions" element={<LedgerPage />} />
          <Route path="/network" element={<NetworkPage />} />
          <Route path="/mining" element={<MiningPage />} />
          <Route path="/settings" element={<PlaceholderPage title="Settings" />} />
        </Routes>
      </BrowserRouter>
    </MiningProvider>
  )
}

function App() {
  return (
    <ConnectionProvider>
      <AppRoutes />
    </ConnectionProvider>
  )
}

// Placeholder for pages not yet implemented
function PlaceholderPage({ title }: { title: string }) {
  return (
    <Layout title={title} subtitle="Coming soon">
      <div className="flex h-96 items-center justify-center rounded-xl border border-dashed border-[--color-steel]">
        <div className="text-center">
          <p className="font-display text-2xl font-bold text-[--color-soft]">{title}</p>
          <p className="mt-2 text-[--color-dim]">This page is under construction</p>
        </div>
      </div>
    </Layout>
  )
}

export default App
