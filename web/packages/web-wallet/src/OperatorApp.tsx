import { useEffect } from 'react'
import {
  BrowserRouter,
  Routes,
  Route,
  Navigate,
  useLocation,
} from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { OperatorPage } from './pages/operator'
import { parseLocalePath } from './lib/locale-path'
import { DEFAULT_LOCALE, storeLocale } from './lib/i18n'

/**
 * Standalone operator-dashboard shell (#772, §8.3.1 option (a)).
 *
 * The operator dashboard is served from its own Vite build entry
 * (`operator.html` -> `operator-main.tsx`) so its HTML document references only
 * its own hashed chunks and can pin `integrity=` (SRI) on them. This shell is
 * the operator analogue of `App.tsx`, trimmed to just the routes the operator
 * document needs.
 *
 * Only `/operator` is a real route here. It is reached at `/operator` and, for
 * a non-default locale, at `/:locale/operator` (e.g. `/es/operator`) — the same
 * locale-stripping contract `App.tsx` uses for the main SPA, reusing
 * `parseLocalePath`. Any other path redirects to `/operator` so a stray
 * navigation within this document lands somewhere valid rather than a blank
 * screen; cross-surface links inside the page (to `/`, `/network`, `/explorer`)
 * are full-document navigations back into the main SPA and are unaffected.
 */
function OperatorRoutes() {
  return (
    <Routes>
      <Route path="/operator" element={<OperatorPage />} />
      {/* Any other path within the operator document falls back to /operator. */}
      <Route path="*" element={<Navigate to="/operator" replace />} />
    </Routes>
  )
}

/**
 * Locale-routing shell for the operator entry, mirroring `LocaleRoutes` in
 * `App.tsx`: strip any leading supported-locale segment before matching, and
 * keep i18next's active language, the persisted choice, and `<html lang>` in
 * sync with the URL.
 */
function OperatorLocaleRoutes() {
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

  if (locale === DEFAULT_LOCALE) {
    return <OperatorRoutes />
  }

  const strippedLocation = { ...location, pathname: rest }
  return (
    <Routes location={strippedLocation}>
      <Route path="/*" element={<OperatorRoutes />} />
    </Routes>
  )
}

function OperatorApp() {
  return (
    <BrowserRouter>
      <OperatorLocaleRoutes />
    </BrowserRouter>
  )
}

export default OperatorApp
