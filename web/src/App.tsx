import { BrowserRouter, Routes, Route } from 'react-router-dom'
import { Layout } from '@/components/layout'
import { DashboardPage } from '@/pages/dashboard'
import { WalletPage } from '@/pages/wallet'
import { LedgerPage } from '@/pages/ledger'
import { NetworkPage } from '@/pages/network'

function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<DashboardPage />} />
        <Route path="/wallet" element={<WalletPage />} />
        <Route path="/ledger" element={<LedgerPage />} />
        <Route path="/blocks" element={<LedgerPage />} />
        <Route path="/transactions" element={<LedgerPage />} />
        <Route path="/network" element={<NetworkPage />} />
        <Route path="/mining" element={<PlaceholderPage title="Mining" />} />
        <Route path="/settings" element={<PlaceholderPage title="Settings" />} />
      </Routes>
    </BrowserRouter>
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
