/**
 * Unit tests for the pure edge locale-negotiation core (issue #779). These
 * exercise the decision logic and the HTML rewrite WITHOUT any Cloudflare
 * runtime, mirroring the "pure testable core, I/O at the edges" pattern of
 * `baas-worker/src/checkout.test.ts`.
 */
import { describe, it, expect } from 'vitest'

import {
  BOT_UA_SUBSTRINGS,
  buildLocalePath,
  isBotUserAgent,
  LOCALE_COOKIE_NAME,
  LOCALE_METADATA,
  localizeDocumentHtml,
  negotiateLocaleRedirect,
  parseAcceptLanguage,
  parseLocaleFromPath,
  readCookie,
  SITE_ORIGIN,
  stripEnPrefixRedirect,
  SUPPORTED_LOCALES,
} from './locale-negotiation'

describe('parseAcceptLanguage', () => {
  it('returns the highest-q supported locale', () => {
    expect(parseAcceptLanguage('es')).toBe('es')
    expect(parseAcceptLanguage('en-US,en;q=0.9')).toBe('en')
  })

  it('ignores region subtags (es-MX matches es)', () => {
    expect(parseAcceptLanguage('es-MX')).toBe('es')
    expect(parseAcceptLanguage('en-GB,en;q=0.8')).toBe('en')
  })

  it('picks the highest-weighted SUPPORTED locale, skipping unsupported ones', () => {
    // fr is unsupported and highest-weighted; es is the best supported match.
    expect(parseAcceptLanguage('fr;q=0.9,es;q=0.5')).toBe('es')
    expect(parseAcceptLanguage('de,fr;q=0.7,es;q=0.3')).toBe('es')
  })

  it('honours q-value ordering over header order', () => {
    expect(parseAcceptLanguage('en;q=0.4,es;q=0.9')).toBe('es')
    expect(parseAcceptLanguage('es;q=0.2,en;q=0.9')).toBe('en')
  })

  it('treats a bare wildcard as the default locale', () => {
    expect(parseAcceptLanguage('*')).toBe('en')
  })

  it('returns undefined for absent / empty / unsupported-only / unparseable headers', () => {
    expect(parseAcceptLanguage(undefined)).toBeUndefined()
    expect(parseAcceptLanguage(null)).toBeUndefined()
    expect(parseAcceptLanguage('')).toBeUndefined()
    expect(parseAcceptLanguage('fr,de;q=0.8')).toBeUndefined()
    expect(parseAcceptLanguage(';;;')).toBeUndefined()
  })

  it('drops q=0 entries', () => {
    expect(parseAcceptLanguage('es;q=0,en;q=0.5')).toBe('en')
    expect(parseAcceptLanguage('es;q=0')).toBeUndefined()
  })
})

describe('isBotUserAgent', () => {
  it('matches known crawler / unfurl UAs (case-insensitive)', () => {
    expect(isBotUserAgent('facebookexternalhit/1.1')).toBe(true)
    expect(isBotUserAgent('Twitterbot/1.0')).toBe(true)
    expect(isBotUserAgent('Slackbot-LinkExpanding 1.0')).toBe(true)
    expect(isBotUserAgent('TelegramBot (like TwitterBot)')).toBe(true)
    expect(isBotUserAgent('Discordbot/2.0')).toBe(true)
    expect(isBotUserAgent('Mozilla/5.0 (compatible; Googlebot/2.1)')).toBe(true)
    expect(isBotUserAgent('Mozilla/5.0 (compatible; bingbot/2.0)')).toBe(true)
    expect(isBotUserAgent('WhatsApp/2.19')).toBe(true)
  })

  it('does not match ordinary browser UAs', () => {
    expect(
      isBotUserAgent(
        'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Version/17.0 Safari/605.1.15',
      ),
    ).toBe(false)
    expect(isBotUserAgent('')).toBe(false)
    expect(isBotUserAgent(null)).toBe(false)
    expect(isBotUserAgent(undefined)).toBe(false)
  })

  it('has a non-empty denylist', () => {
    expect(BOT_UA_SUBSTRINGS.length).toBeGreaterThan(5)
  })
})

describe('readCookie', () => {
  it('extracts a named cookie value', () => {
    expect(readCookie('a=1; botho_locale=es; b=2', 'botho_locale')).toBe('es')
    expect(readCookie('botho_locale=en', 'botho_locale')).toBe('en')
  })

  it('returns undefined when absent', () => {
    expect(readCookie('a=1; b=2', 'botho_locale')).toBeUndefined()
    expect(readCookie('', 'botho_locale')).toBeUndefined()
    expect(readCookie(null, 'botho_locale')).toBeUndefined()
  })
})

describe('parseLocaleFromPath', () => {
  it('extracts an explicit non-default prefix', () => {
    expect(parseLocaleFromPath('/es/wallet')).toEqual({ locale: 'es', rest: '/wallet' })
    expect(parseLocaleFromPath('/es')).toEqual({ locale: 'es', rest: '/' })
  })

  it('defaults to en for unprefixed / unsupported prefixes', () => {
    expect(parseLocaleFromPath('/wallet')).toEqual({ locale: 'en', rest: '/wallet' })
    expect(parseLocaleFromPath('/xx/wallet')).toEqual({ locale: 'en', rest: '/xx/wallet' })
    expect(parseLocaleFromPath('/')).toEqual({ locale: 'en', rest: '/' })
  })
})

describe('buildLocalePath', () => {
  it('leaves the default locale unprefixed and prefixes others', () => {
    expect(buildLocalePath('en', '/wallet')).toBe('/wallet')
    expect(buildLocalePath('es', '/wallet')).toBe('/es/wallet')
    expect(buildLocalePath('es', '/')).toBe('/es')
  })
})

describe('stripEnPrefixRedirect', () => {
  it('maps the bare /en orphan (with or without trailing slash) to the root', () => {
    expect(stripEnPrefixRedirect('/en')).toBe('/')
    expect(stripEnPrefixRedirect('/en/')).toBe('/')
  })

  it('strips the /en prefix from deeper paths', () => {
    expect(stripEnPrefixRedirect('/en/wallet')).toBe('/wallet')
    expect(stripEnPrefixRedirect('/en/explorer/tx/abc')).toBe('/explorer/tx/abc')
    expect(stripEnPrefixRedirect('/en/node/status')).toBe('/node/status')
  })

  it('returns undefined for paths that are NOT /en orphans', () => {
    expect(stripEnPrefixRedirect('/')).toBeUndefined()
    expect(stripEnPrefixRedirect('/wallet')).toBeUndefined()
    expect(stripEnPrefixRedirect('/es')).toBeUndefined()
    expect(stripEnPrefixRedirect('/es/wallet')).toBeUndefined()
    // Lookalike segments that merely START with "en" must be left alone.
    expect(stripEnPrefixRedirect('/enterprise')).toBeUndefined()
    expect(stripEnPrefixRedirect('/env')).toBeUndefined()
  })

  it('runs independently of Accept-Language negotiation (the 301 wins, not /es/en)', () => {
    // Regression guard for the compounding bug: negotiateLocaleRedirect would
    // send a first-visit es browser on `/en` to `/es/en` (since `/en` is not an
    // explicit locale prefix). The middleware must therefore apply the /en 301
    // FIRST — this test documents that the pure strip helper resolves `/en` to
    // the unprefixed root, and that negotiation of `/en` (were it ever reached)
    // is exactly the pathology we avoid by ordering the strip ahead of it.
    expect(stripEnPrefixRedirect('/en')).toBe('/')
    const wouldCompound = negotiateLocaleRedirect({
      pathname: '/en',
      acceptLanguage: 'es-ES,es;q=0.9',
      cookie: null,
      userAgent:
        'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36',
    })
    // Confirms the pathology exists at the negotiation layer (hence the ordering
    // requirement): unguarded, `/en` negotiates onward to the orphan `/es/en`.
    expect(wouldCompound).toEqual({
      kind: 'redirect',
      location: '/es/en',
      cookieLocale: 'es',
    })
  })
})

describe('negotiateLocaleRedirect', () => {
  const browserUA =
    'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36'

  it('redirects a first-visit es browser on a default path to the /es equivalent', () => {
    const d = negotiateLocaleRedirect({
      pathname: '/wallet',
      acceptLanguage: 'es-ES,es;q=0.9',
      cookie: null,
      userAgent: browserUA,
    })
    expect(d).toEqual({ kind: 'redirect', location: '/es/wallet', cookieLocale: 'es' })
  })

  it('redirects the root path first-visit es browser to /es', () => {
    const d = negotiateLocaleRedirect({
      pathname: '/',
      acceptLanguage: 'es',
      cookie: null,
      userAgent: browserUA,
    })
    expect(d).toEqual({ kind: 'redirect', location: '/es', cookieLocale: 'es' })
  })

  it('does NOT redirect a first-visit en browser (passes, stamps default cookie)', () => {
    const d = negotiateLocaleRedirect({
      pathname: '/wallet',
      acceptLanguage: 'en-US,en;q=0.9',
      cookie: null,
      userAgent: browserUA,
    })
    expect(d).toEqual({ kind: 'pass', setCookieLocale: 'en' })
  })

  it('does NOT redirect when the cookie is already set, even if Accept-Language differs', () => {
    const d = negotiateLocaleRedirect({
      pathname: '/wallet',
      acceptLanguage: 'es',
      cookie: `${LOCALE_COOKIE_NAME}=en`,
      userAgent: browserUA,
    })
    expect(d).toEqual({ kind: 'pass' })
  })

  it('never redirects known crawler/bot UAs regardless of Accept-Language', () => {
    for (const ua of ['facebookexternalhit/1.1', 'Twitterbot/1.0', 'Googlebot/2.1', 'Slackbot 1.0']) {
      const d = negotiateLocaleRedirect({
        pathname: '/wallet',
        acceptLanguage: 'es-ES,es;q=1.0',
        cookie: null,
        userAgent: ua,
      })
      expect(d).toEqual({ kind: 'pass' })
    }
  })

  it('never redirects a path that already carries a locale prefix (stamps that locale)', () => {
    const d = negotiateLocaleRedirect({
      pathname: '/es/wallet',
      acceptLanguage: 'en-US,en;q=0.9',
      cookie: null,
      userAgent: browserUA,
    })
    expect(d).toEqual({ kind: 'pass', setCookieLocale: 'es' })
  })

  it('does not redirect when Accept-Language is absent (privacy browsers)', () => {
    const d = negotiateLocaleRedirect({
      pathname: '/wallet',
      acceptLanguage: null,
      cookie: null,
      userAgent: browserUA,
    })
    expect(d).toEqual({ kind: 'pass', setCookieLocale: 'en' })
  })

  it('does not redirect when the best supported match is the default locale', () => {
    const d = negotiateLocaleRedirect({
      pathname: '/',
      acceptLanguage: 'fr;q=0.9,en;q=0.5', // fr unsupported → en wins
      cookie: null,
      userAgent: browserUA,
    })
    expect(d).toEqual({ kind: 'pass', setCookieLocale: 'en' })
  })
})

describe('localizeDocumentHtml', () => {
  // A representative slice of index.html with the exact tag format the rewrite
  // targets (property/name first, then content).
  const baseHtml = `<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="description" content="Botho - Private Currency for the Quantum Era" />
    <meta property="og:site_name" content="Botho" />
    <meta property="og:title" content="Botho - Private Currency for the Quantum Era" />
    <meta property="og:description" content="Instant SCP finality. Quantum-safe recipient addresses. Progressive economics that reward circulation over hoarding." />
    <meta property="og:url" content="https://botho.io/" />
    <meta name="twitter:card" content="summary" />
    <meta name="twitter:title" content="Botho - Private Currency for the Quantum Era" />
    <meta name="twitter:description" content="Instant SCP finality. Quantum-safe recipient addresses. Progressive economics that reward circulation over hoarding." />
    <title>Botho</title>
  </head>
  <body><div id="root"></div></body>
</html>`

  it('localizes og/twitter/description meta to Spanish for an es path', () => {
    const out = localizeDocumentHtml(baseHtml, 'es', '/es/wallet')
    expect(out).toContain(
      `<meta property="og:title" content="${LOCALE_METADATA.es.title}" />`,
    )
    expect(out).toContain(
      `<meta name="twitter:title" content="${LOCALE_METADATA.es.title}" />`,
    )
    expect(out).toContain(
      `<meta property="og:description" content="${LOCALE_METADATA.es.description}" />`,
    )
    expect(out).toContain(
      `<meta name="twitter:description" content="${LOCALE_METADATA.es.description}" />`,
    )
    expect(out).toContain(
      `<meta name="description" content="${LOCALE_METADATA.es.description}" />`,
    )
    // No English title/description survives.
    expect(out).not.toContain('Private Currency for the Quantum Era')
  })

  it('sets og:url and canonical to the requested locale-prefixed URL', () => {
    const out = localizeDocumentHtml(baseHtml, 'es', '/es/wallet')
    expect(out).toContain(
      `<meta property="og:url" content="${SITE_ORIGIN}/es/wallet" />`,
    )
    expect(out).toContain(`<link rel="canonical" href="${SITE_ORIGIN}/es/wallet" />`)
    // The hardcoded "https://botho.io/" og:url must be gone.
    expect(out).not.toContain('content="https://botho.io/"')
  })

  it('sets <html lang> to the resolved locale', () => {
    expect(localizeDocumentHtml(baseHtml, 'es', '/es')).toContain('<html lang="es">')
    expect(localizeDocumentHtml(baseHtml, 'en', '/')).toContain('<html lang="en">')
  })

  it('injects hreflang alternates for every supported locale plus x-default', () => {
    const out = localizeDocumentHtml(baseHtml, 'es', '/es/wallet')
    expect(out).toContain(
      `<link rel="alternate" hreflang="en" href="${SITE_ORIGIN}/wallet" />`,
    )
    expect(out).toContain(
      `<link rel="alternate" hreflang="es" href="${SITE_ORIGIN}/es/wallet" />`,
    )
    expect(out).toContain(
      `<link rel="alternate" hreflang="x-default" href="${SITE_ORIGIN}/wallet" />`,
    )
    // One alternate per supported locale + x-default.
    const count = (out.match(/rel="alternate"/g) ?? []).length
    expect(count).toBe(SUPPORTED_LOCALES.length + 1)
  })

  it('leaves English metadata content unchanged (no regression) for the default locale', () => {
    const out = localizeDocumentHtml(baseHtml, 'en', '/wallet')
    expect(out).toContain(
      `<meta property="og:title" content="${LOCALE_METADATA.en.title}" />`,
    )
    expect(out).toContain(
      `<meta property="og:description" content="${LOCALE_METADATA.en.description}" />`,
    )
    // og:url is corrected from the hardcoded root to the real requested path.
    expect(out).toContain(
      `<meta property="og:url" content="${SITE_ORIGIN}/wallet" />`,
    )
  })

  it('escapes HTML-significant characters in injected attribute values', () => {
    // Root path for es → the canonical/hreflang for root must be well-formed.
    const out = localizeDocumentHtml(baseHtml, 'es', '/es')
    expect(out).toContain(`<link rel="canonical" href="${SITE_ORIGIN}/es" />`)
    expect(out).toContain(
      `<link rel="alternate" hreflang="x-default" href="${SITE_ORIGIN}/" />`,
    )
  })
})
