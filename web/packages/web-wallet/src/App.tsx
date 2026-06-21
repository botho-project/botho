import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom'
import { NetworkProvider } from './contexts/network'
import { WalletProvider } from './contexts/wallet'
import { LandingPage } from './pages/landing'
import { WalletPage } from './pages/wallet'
import { ClaimPage } from './pages/claim'
import { PayPage } from './pages/pay'
import { DocsPage } from './pages/docs'
import { ExplorerPage } from './pages/explorer'

/**
 * Decide what `/` should render.
 *
 * The same SPA bundle is deployed to two hostnames: the marketing site
 * (botho.io) and the wallet subdomain (wallet.botho.io). On the wallet
 * subdomain users expect the wallet, not the marketing homepage (#459), so we
 * redirect `/` -> `/wallet` there. Everywhere else (botho.io, localhost, the
 * Playwright preview) `/` keeps rendering the landing page, which also keeps the
 * existing e2e smoke tests (`a[href="/"]` -> landing) green.
 *
 * The landing page is always reachable at `/` on the marketing host and at
 * `/home` / `/about` on any host, and is linked from the wallet header.
 */
function isWalletHost(): boolean {
  if (typeof window === 'undefined') return false
  return window.location.hostname.startsWith('wallet.')
}

function RootRoute() {
  if (isWalletHost()) {
    return <Navigate to="/wallet" replace />
  }
  return <LandingPage />
}

function App() {
  return (
    <NetworkProvider>
      <WalletProvider>
        <BrowserRouter>
          <Routes>
            <Route path="/" element={<RootRoute />} />
            {/* Landing is always reachable directly, regardless of host. */}
            <Route path="/home" element={<LandingPage />} />
            <Route path="/about" element={<LandingPage />} />
            <Route path="/wallet" element={<WalletPage />} />
            <Route path="/claim" element={<ClaimPage />} />
            <Route path="/pay" element={<PayPage />} />
            <Route path="/explorer" element={<ExplorerPage />} />
            <Route path="/explorer/tx/:hash" element={<ExplorerPage />} />
            <Route path="/explorer/block/:hash" element={<ExplorerPage />} />
            <Route path="/docs" element={<DocsPage />} />
            <Route path="/docs/*" element={<DocsPage />} />
          </Routes>
        </BrowserRouter>
      </WalletProvider>
    </NetworkProvider>
  )
}

export default App
