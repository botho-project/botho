import { BrowserRouter, Routes, Route } from 'react-router-dom'
import { ConnectionProvider, useConnection } from './contexts/connection'
import { MiningProvider } from './contexts/mining'
import { WalletProvider } from './contexts/wallet'
import { SplashScreen } from './components/splash-screen'
import { DashboardPage } from './pages/dashboard'
import { WalletPage } from './pages/wallet'
import { LedgerPage } from './pages/ledger'
import { NetworkPage } from './pages/network'
import { MiningPage } from './pages/mining'
import { SettingsPage } from './pages/settings'

function AppRoutes() {
  const { connectedNode } = useConnection()

  if (!connectedNode) {
    return <SplashScreen />
  }

  return (
    <MiningProvider>
      <WalletProvider>
        <BrowserRouter>
          <Routes>
            <Route path="/" element={<DashboardPage />} />
            <Route path="/wallet" element={<WalletPage />} />
            <Route path="/ledger" element={<LedgerPage />} />
            <Route path="/blocks" element={<LedgerPage />} />
            <Route path="/transactions" element={<LedgerPage />} />
            <Route path="/network" element={<NetworkPage />} />
            <Route path="/mining" element={<MiningPage />} />
            <Route path="/settings" element={<SettingsPage />} />
          </Routes>
        </BrowserRouter>
      </WalletProvider>
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

export default App
