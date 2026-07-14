/**
 * i18next initialization for the web-wallet SPA (issue #764, phase 1).
 *
 * Phase 1 scope: framework setup + the marketing landing page translated into
 * one additional language (Spanish). Phase 2 (issue #777) added the
 * transactional surfaces — the `wallet`, `pay`, `claim`, and `contacts`
 * namespaces. Phase 3 (issue #778) added the `docs` namespace: section titles /
 * nav labels live here as JSON keys, while the large markdown bodies live in
 * per-locale `.md` files under `../docs-content/` (imported with Vite `?raw`)
 * rather than as escaped JSON strings. Metadata/OG surfaces remain out of scope
 * and land in a later phase.
 *
 * Locale is driven by a URL path prefix (`/es/...`) rather than a subdomain, so
 * the existing `botho.io` / `wallet.botho.io` host-switch logic in `App.tsx`
 * stays orthogonal to language. English is the default and is served WITHOUT a
 * prefix (`/`, `/wallet`, ...) so all existing absolute-path routes and e2e
 * smoke tests keep working unchanged.
 *
 * Resource bundles are imported statically (not lazy-loaded) because they are
 * tiny and bundling them eliminates the "flash of English" that a network fetch
 * on the non-default locale would otherwise cause on first paint.
 */
import i18n from 'i18next'
import { initReactI18next } from 'react-i18next'

import {
  DEFAULT_LOCALE,
  isSupportedLocale,
  SUPPORTED_LOCALES,
  type SupportedLocale,
} from './locales'

import enLanding from '../locales/en/landing.json'
import esLanding from '../locales/es/landing.json'
import enWallet from '../locales/en/wallet.json'
import esWallet from '../locales/es/wallet.json'
import enPay from '../locales/en/pay.json'
import esPay from '../locales/es/pay.json'
import enClaim from '../locales/en/claim.json'
import esClaim from '../locales/es/claim.json'
import enContacts from '../locales/en/contacts.json'
import esContacts from '../locales/es/contacts.json'
import enDocs from '../locales/en/docs.json'
import esDocs from '../locales/es/docs.json'
import enNode from '../locales/en/node.json'
import esNode from '../locales/es/node.json'
import zhLanding from '../locales/zh/landing.json'
import zhWallet from '../locales/zh/wallet.json'
import zhPay from '../locales/zh/pay.json'
import zhClaim from '../locales/zh/claim.json'
import zhContacts from '../locales/zh/contacts.json'
import zhDocs from '../locales/zh/docs.json'
import zhNode from '../locales/zh/node.json'

/**
 * Locale constants live in the dependency-free `./locales` leaf module so build
 * targets that must not import the i18next runtime (the edge Pages Function,
 * #779) can share them. Re-exported here so existing `./i18n` importers are
 * unchanged.
 */
export {
  SUPPORTED_LOCALES,
  DEFAULT_LOCALE,
  isSupportedLocale,
  type SupportedLocale,
}

/** localStorage key used to persist the visitor's explicit locale choice. */
export const LOCALE_STORAGE_KEY = 'botho:locale'

/**
 * Read a previously persisted locale choice from localStorage, falling back to
 * `undefined` when none is stored or storage is unavailable (SSR/tests).
 */
export function getStoredLocale(): SupportedLocale | undefined {
  try {
    const stored = globalThis.localStorage?.getItem(LOCALE_STORAGE_KEY)
    return isSupportedLocale(stored) ? stored : undefined
  } catch {
    return undefined
  }
}

/** Persist the visitor's explicit locale choice; no-op if storage is unavailable. */
export function storeLocale(locale: SupportedLocale): void {
  try {
    globalThis.localStorage?.setItem(LOCALE_STORAGE_KEY, locale)
  } catch {
    // Ignore — private browsing / disabled storage should not break navigation.
  }
}

const resources = {
  en: {
    landing: enLanding,
    wallet: enWallet,
    pay: enPay,
    claim: enClaim,
    contacts: enContacts,
    docs: enDocs,
    node: enNode,
  },
  es: {
    landing: esLanding,
    wallet: esWallet,
    pay: esPay,
    claim: esClaim,
    contacts: esContacts,
    docs: esDocs,
    node: esNode,
  },
  zh: {
    landing: zhLanding,
    wallet: zhWallet,
    pay: zhPay,
    claim: zhClaim,
    contacts: zhContacts,
    docs: zhDocs,
    node: zhNode,
  },
} as const

// Initialize once. Guard against double-init under React StrictMode / HMR.
if (!i18n.isInitialized) {
  void i18n.use(initReactI18next).init({
    resources,
    lng: getStoredLocale() ?? DEFAULT_LOCALE,
    fallbackLng: DEFAULT_LOCALE,
    supportedLngs: SUPPORTED_LOCALES as unknown as string[],
    defaultNS: 'landing',
    ns: ['landing', 'wallet', 'pay', 'claim', 'contacts', 'docs', 'node'],
    interpolation: {
      // React already escapes interpolated values.
      escapeValue: false,
    },
    react: {
      useSuspense: false,
    },
  })
}

export default i18n
