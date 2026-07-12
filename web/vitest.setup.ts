/**
 * Global vitest setup.
 *
 * Initializes the web-wallet i18next instance (side-effect import) so any
 * component test that renders a page calling `useTranslation()` resolves real
 * translations instead of bare keys. Page tests default to the `en` locale, so
 * existing assertions against English copy keep passing without per-file
 * boilerplate (issue #777, i18n phase 2). Tests that need Spanish call
 * `i18n.changeLanguage('es')` explicitly.
 *
 * The import is wrapped so it is a no-op in the non-jsdom (node) test
 * environment where the React/i18n stack is not needed.
 */
import { beforeAll } from 'vitest'

beforeAll(async () => {
  if (typeof document !== 'undefined') {
    await import('./packages/web-wallet/src/lib/i18n')
  }
})
