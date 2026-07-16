import { useEffect } from 'react'
import {
  BrowserRouter,
  Routes,
  Route,
  Navigate,
  useLocation,
} from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { NetworkProvider } from './contexts/network'
import { WalletProvider } from './contexts/wallet'
import { LandingPage } from './pages/landing'
import { WalletPage } from './pages/wallet'
import { ClaimPage } from './pages/claim'
import { PayPage } from './pages/pay'
import { ContactsPage } from './pages/contacts'
import { DocsPage } from './pages/docs'
import { ExplorerPage } from './pages/explorer'
import { NetworkPage } from './pages/network'
import { TradePage } from './pages/trade'
import { OperatorPage } from './pages/operator'
import { NodePage, NodeSuccessPage, NodeStatusPage } from './pages/node'
import { parseLocalePath } from './lib/locale-path'
import { DEFAULT_LOCALE, storeLocale } from './lib/i18n'

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

/**
 * The application's route table, expressed in locale-agnostic (unprefixed)
 * paths — `/`, `/wallet`, `/explorer`, ...
 *
 * Locale routing (issue #764) is handled one level up by `LocaleRoutes`, which
 * strips any leading `/:locale` segment from the URL before matching here. That
 * keeps every route literal (`/wallet`, not `/:locale/wallet`), so all existing
 * absolute links and e2e expectations (`a[href="/wallet"]`, the wallet-host
 * `/` -> `/wallet` redirect) keep working unchanged, and a non-default locale
 * like `/es/wallet` resolves to exactly the same route as `/wallet`.
 */
function AppRoutes() {
  return (
    <Routes>
      <Route path="/" element={<RootRoute />} />
      {/* Landing is always reachable directly, regardless of host. */}
      <Route path="/home" element={<LandingPage />} />
      <Route path="/about" element={<LandingPage />} />
      <Route path="/wallet" element={<WalletPage />} />
      <Route path="/claim" element={<ClaimPage />} />
      <Route path="/pay" element={<PayPage />} />
      <Route path="/contacts" element={<ContactsPage />} />
      <Route path="/explorer" element={<ExplorerPage />} />
      <Route path="/network" element={<NetworkPage />} />
      {/* wBTH discovery / bridge scaffold — Tier 0 (#1030, epic #1029). */}
      <Route path="/trade" element={<TradePage />} />
      {/* Operator dashboard — public read surface (#706, #695 P4.1). */}
      <Route path="/operator" element={<OperatorPage />} />
      <Route path="/explorer/tx/:hash" element={<ExplorerPage />} />
      <Route path="/explorer/block/:hash" element={<ExplorerPage />} />
      <Route path="/docs" element={<DocsPage />} />
      <Route path="/docs/*" element={<DocsPage />} />
      {/* Botho-as-a-Service "Get a node" surface (#458 §4 / #504). */}
      <Route path="/node" element={<NodePage />} />
      <Route path="/node/success" element={<NodeSuccessPage />} />
      {/* Node status page reached via magic link (#458 §4 / #507). */}
      <Route path="/node/status" element={<NodeStatusPage />} />
      {/*
        `/en` is an orphan namespace — English is the unprefixed default, so
        `/en` / `/en/*` match no route above and render blank (#797). The edge
        301s these on a fresh load; this is client-side defense-in-depth for any
        in-app navigation that constructs an `/en` URL without a full reload.
      */}
      <Route path="/en" element={<Navigate to="/" replace />} />
      <Route path="/en/*" element={<EnPrefixRedirect />} />
    </Routes>
  )
}

/**
 * Redirect a stale `/en/<rest>` URL to its unprefixed `/<rest>` equivalent,
 * preserving the query string and hash. English is the unprefixed default, so
 * `/en/wallet` is an orphan alias for `/wallet` (#797, item 4c).
 */
function EnPrefixRedirect() {
  const location = useLocation()
  const rest = location.pathname.slice('/en'.length) || '/'
  return <Navigate to={`${rest}${location.search}${location.hash}`} replace />
}

/**
 * Locale-routing shell (issue #764, phase 1).
 *
 * Reads the real URL, splits off any leading supported-locale segment (`/es`),
 * and renders the locale-agnostic `AppRoutes` against the *remaining* path via
 * the `location` prop. English (the default) is unprefixed; an unsupported or
 * absent prefix falls back to the default locale (so `/xx/...` renders in
 * English rather than 404-ing).
 *
 * As a side effect it keeps i18next's active language, the persisted choice,
 * and the document's `<html lang>` attribute in sync with the URL — navigation
 * is the single source of truth for the active language.
 */
function LocaleRoutes() {
  const { i18n } = useTranslation()
  const location = useLocation()
  const { locale, rest } = parseLocalePath(location.pathname)

  useEffect(() => {
    if (i18n.language !== locale) {
      void i18n.changeLanguage(locale)
    }
    storeLocale(locale)
    if (typeof document !== 'undefined') {
      document.documentElement.lang = locale
    }
  }, [i18n, locale])

  // For the default locale the URL is already unprefixed; render it directly so
  // `useLocation()` in child pages continues to report the real path. For a
  // non-default locale, present the locale-stripped path to `AppRoutes`.
  if (locale === DEFAULT_LOCALE) {
    return <AppRoutes />
  }

  const strippedLocation = { ...location, pathname: rest }
  return (
    <Routes location={strippedLocation}>
      <Route path="/*" element={<AppRoutes />} />
    </Routes>
  )
}

function App() {
  return (
    <NetworkProvider>
      <WalletProvider>
        <BrowserRouter>
          <LocaleRoutes />
        </BrowserRouter>
      </WalletProvider>
    </NetworkProvider>
  )
}

export default App
