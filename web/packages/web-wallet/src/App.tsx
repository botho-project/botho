import { BrowserRouter, Routes, Route } from 'react-router-dom'
import { WalletProvider } from './contexts/wallet'
import { LandingPage } from './pages/landing'
import { WalletPage } from './pages/wallet'
import { DocsPage } from './pages/docs'
import { ExplorerPage } from './pages/explorer'

function App() {
  return (
    <WalletProvider>
      <BrowserRouter>
        <Routes>
          <Route path="/" element={<LandingPage />} />
          <Route path="/wallet" element={<WalletPage />} />
          <Route path="/explorer" element={<ExplorerPage />} />
          <Route path="/docs" element={<DocsPage />} />
          <Route path="/docs/*" element={<DocsPage />} />
        </Routes>
      </BrowserRouter>
    </WalletProvider>
  )
}

export default App
