import { BrowserRouter, Routes, Route } from 'react-router-dom'
import { ConnectionProvider, useConnection } from './contexts/connection'
import { MintingProvider } from './contexts/minting'
import { WalletProvider } from './contexts/wallet'
import { SplashScreen } from './components/splash-screen'
import { DashboardPage } from './pages/dashboard'
import { WalletPage } from './pages/wallet'
import { LedgerPage } from './pages/ledger'
import { NetworkPage } from './pages/network'
import { MintingPage } from './pages/minting'
import { SettingsPage } from './pages/settings'

function AppRoutes() {
  const { connectedNode } = useConnection()

  if (!connectedNode) {
    return <SplashScreen />
  }

  return (
    <MintingProvider>
      <WalletProvider>
        <BrowserRouter>
          <Routes>
            <Route path="/" element={<DashboardPage />} />
            <Route path="/wallet" element={<WalletPage />} />
            <Route path="/ledger" element={<LedgerPage />} />
            <Route path="/blocks" element={<LedgerPage />} />
            <Route path="/transactions" element={<LedgerPage />} />
            <Route path="/network" element={<NetworkPage />} />
            <Route path="/minting" element={<MintingPage />} />
            <Route path="/settings" element={<SettingsPage />} />
          </Routes>
        </BrowserRouter>
      </WalletProvider>
    </MintingProvider>
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
