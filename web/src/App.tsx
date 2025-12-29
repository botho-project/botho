import { BrowserRouter, Routes, Route } from 'react-router-dom'
import { Layout } from '@/components/layout'
import { DashboardPage } from '@/pages/dashboard'

function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<DashboardPage />} />
        {/* Additional routes will be added here */}
        <Route path="/blocks" element={<PlaceholderPage title="Blocks" />} />
        <Route path="/transactions" element={<PlaceholderPage title="Transactions" />} />
        <Route path="/network" element={<PlaceholderPage title="Network" />} />
        <Route path="/mining" element={<PlaceholderPage title="Mining" />} />
        <Route path="/wallet" element={<PlaceholderPage title="Wallet" />} />
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
