/**
 * Pure, unit-testable core of the Cloudflare Pages edge locale layer (issue
 * #779). Two responsibilities, both expressed as side-effect-free functions so
 * they can be tested with vitest exactly like `baas-worker/src/checkout.ts`:
 *
 *  1. `negotiateLocaleRedirect(...)` — decide whether a first-visit navigation
 *     should be redirected to its `Accept-Language`-matched locale prefix.
 *  2. `localizeDocumentHtml(...)` — rewrite the static `index.html`'s OG/Twitter
 *     meta + inject `hreflang` alternates for the resolved locale of a request.
 *
 * The Cloudflare-specific glue (reading the real `Request`, calling
 * `context.next()`, attaching headers to the real `Response`) lives in the thin
 * adapter `functions/_middleware.ts`; NOTHING in this file imports Cloudflare
 * runtime types — inputs are plain strings so the logic is trivially testable.
 *
 * The supported-locale list and default are imported from the SPA's leaf
 * constants module (`../../src/lib/locales`) rather than duplicated, so the edge
 * layer and the client-side router can never drift (#779 Affected Files).
 */
import {
  DEFAULT_LOCALE,
  isSupportedLocale,
  SUPPORTED_LOCALES,
  type SupportedLocale,
} from '../../src/lib/locales'

export { SUPPORTED_LOCALES, DEFAULT_LOCALE, type SupportedLocale }

/** Canonical site origin used for absolute `og:url` / `hreflang` values. */
export const SITE_ORIGIN = 'https://botho.io'

/**
 * Per-locale document metadata used to rewrite the (English-baked) `index.html`
 * OG/Twitter tags. Keep the copy in sync with the landing hero
 * (`locales/<locale>/landing.json`) and with the English baseline in
 * `index.html` — the `en` entry MUST match `index.html` verbatim so the
 * default-locale response is byte-identical to the un-rewritten document.
 */
export interface LocaleMetadata {
  /** `<meta name="description">` + og/twitter description. */
  description: string
  /** og:title / twitter:title. */
  title: string
  /** `document.title` (the browser tab). */
  htmlTitle: string
}

export const LOCALE_METADATA: Record<SupportedLocale, LocaleMetadata> = {
  en: {
    title: 'Botho - Private Currency for the Quantum Era',
    description:
      'Instant SCP finality. Quantum-safe recipient addresses. Progressive economics that reward circulation over hoarding.',
    htmlTitle: 'Botho',
  },
  es: {
    title: 'Botho - Moneda Privada para la Era Cuántica',
    description:
      'Finalidad SCP instantánea. Direcciones de destinatario resistentes a la computación cuántica. Economía progresiva que premia la circulación sobre el acaparamiento.',
    htmlTitle: 'Botho',
  },
}

/**
 * Best-effort denylist of User-Agent substrings for link-preview unfurlers and
 * search-engine crawlers. Matched case-insensitively. Crawlers MUST NEVER be
 * redirected (#779): a redirect breaks OG/Twitter unfurls (most preview bots do
 * not follow redirects) and can look like cloaking to search engines. This is a
 * heuristic, not an exhaustive list — treat as one of several skip signals.
 */
export const BOT_UA_SUBSTRINGS: readonly string[] = [
  'bot',
  'crawl',
  'spider',
  'slurp',
  'facebookexternalhit',
  'facebookcatalog',
  'slackbot',
  'telegrambot',
  'discordbot',
  'whatsapp',
  'twitterbot',
  'linkedinbot',
  'pinterest',
  'redditbot',
  'embedly',
  'quora link preview',
  'google-inspectiontool',
  'chrome-lighthouse',
  'headlesschrome',
]

/** True when `userAgent` looks like a crawler / link-preview bot. */
export function isBotUserAgent(userAgent: string | null | undefined): boolean {
  if (!userAgent) return false
  const ua = userAgent.toLowerCase()
  return BOT_UA_SUBSTRINGS.some((needle) => ua.includes(needle))
}

/**
 * Parse an `Accept-Language` header and return the highest-q-weighted locale
 * that we actually support, or `undefined` if none match (or the header is
 * absent / unparseable). Only base language tags are compared (e.g. `es-MX`
 * matches `es`); region subtags are ignored since we key on base language.
 *
 * A ~q-value parser is sufficient for the tiny supported set — no dependency.
 * `q` defaults to 1.0 when omitted, and entries are stably ordered by descending
 * q so equal-weight ties preserve header order (matching browser precedence).
 */
export function parseAcceptLanguage(
  header: string | null | undefined,
): SupportedLocale | undefined {
  if (!header) return undefined

  const entries: { tag: string; q: number }[] = []
  for (const part of header.split(',')) {
    const [rawTag, ...params] = part.trim().split(';')
    const tag = rawTag.trim().toLowerCase()
    if (!tag) continue
    let q = 1
    for (const param of params) {
      const [k, v] = param.split('=').map((s) => s.trim())
      if (k === 'q') {
        const parsed = Number.parseFloat(v)
        if (Number.isFinite(parsed)) q = parsed
      }
    }
    // A wildcard `*` means "any language"; only honour it as the default locale.
    entries.push({ tag, q })
  }

  // Stable sort by descending q (Array#sort is stable in modern engines, and we
  // additionally track original index to be explicit about tie-breaking).
  const ordered = entries
    .map((e, i) => ({ ...e, i }))
    .filter((e) => e.q > 0)
    .sort((a, b) => (b.q === a.q ? a.i - b.i : b.q - a.q))

  for (const { tag } of ordered) {
    if (tag === '*') return DEFAULT_LOCALE
    const base = tag.split('-')[0]
    if (isSupportedLocale(base)) return base
  }
  return undefined
}

/**
 * Read a single cookie value from a `Cookie` request-header string, or
 * `undefined` when absent. Whitespace-tolerant; does not decode values (our
 * locale cookie values are plain ASCII locale codes).
 */
export function readCookie(
  cookieHeader: string | null | undefined,
  name: string,
): string | undefined {
  if (!cookieHeader) return undefined
  for (const pair of cookieHeader.split(';')) {
    const eq = pair.indexOf('=')
    if (eq === -1) continue
    if (pair.slice(0, eq).trim() === name) {
      return pair.slice(eq + 1).trim()
    }
  }
  return undefined
}

/** Cookie set once per visitor to mark "locale already negotiated / chosen". */
export const LOCALE_COOKIE_NAME = 'botho_locale'

/**
 * Split a pathname into its (optional) leading locale segment and the rest.
 * Mirrors `src/lib/locale-path.ts#parseLocalePath` semantics (an unsupported or
 * absent prefix resolves to the default locale with the path unchanged) but is
 * re-implemented here so the Pages Function does not import the SPA's routing
 * module (which is bundled for the browser, not the edge).
 */
export function parseLocaleFromPath(pathname: string): {
  locale: SupportedLocale
  rest: string
} {
  const segments = pathname.split('/')
  const first = segments[1]
  if (isSupportedLocale(first) && first !== DEFAULT_LOCALE) {
    const rest = '/' + segments.slice(2).join('/')
    return { locale: first, rest: rest === '/' ? '/' : rest.replace(/\/$/, '') || '/' }
  }
  return { locale: DEFAULT_LOCALE, rest: pathname || '/' }
}

/** Build a locale-prefixed pathname (default locale is unprefixed). */
export function buildLocalePath(locale: SupportedLocale, rest: string): string {
  const normalizedRest = rest.startsWith('/') ? rest : `/${rest}`
  if (locale === DEFAULT_LOCALE) return normalizedRest
  if (normalizedRest === '/') return `/${locale}`
  return `/${locale}${normalizedRest}`
}

/** Inputs to the redirect decision — plain primitives, no runtime types. */
export interface NegotiationInput {
  /** Request path (no query string), e.g. `/wallet`. */
  pathname: string
  /** Raw `Accept-Language` header value (or null/undefined if absent). */
  acceptLanguage: string | null | undefined
  /** Raw `Cookie` header value (or null/undefined). */
  cookie: string | null | undefined
  /** Raw `User-Agent` header value (or null/undefined). */
  userAgent: string | null | undefined
}

export type NegotiationDecision =
  | {
      /** Redirect the visitor to `location` (302) and set the seen-cookie. */
      kind: 'redirect'
      location: string
      /** Locale the cookie should be stamped with. */
      cookieLocale: SupportedLocale
    }
  | {
      /**
       * No redirect — pass the request through. `setCookieLocale` is set when we
       * should still stamp the first-visit cookie on the pass-through response
       * (marks the visitor "seen" so we don't re-evaluate on every navigation);
       * `undefined` means don't touch cookies (bot / already-cookied / asset).
       */
      kind: 'pass'
      setCookieLocale?: SupportedLocale
    }

/**
 * Decide whether a first-visit HTML navigation should be redirected to its
 * `Accept-Language`-matched locale prefix. Pure: given the request primitives,
 * returns a `redirect` or `pass` decision. See the acceptance criteria in #779.
 *
 * Order of checks (each is a "never redirect" gate except the last):
 *  1. Bot/crawler UA           → pass, no cookie (unfurl/SEO must see canonical URL)
 *  2. Cookie already set       → pass, respect the prior explicit/negotiated choice
 *  3. Path already has an       → pass, but stamp the cookie for that locale
 *     explicit locale prefix         (visiting /es/* is itself a choice)
 *  4. Accept-Language resolves  → REDIRECT (302) to the prefixed equivalent
 *     to a non-default locale        and stamp the cookie
 *  5. otherwise (default match, → pass, stamp default cookie
 *     no/absent header, etc.)
 */
export function negotiateLocaleRedirect(input: NegotiationInput): NegotiationDecision {
  const { pathname, acceptLanguage, cookie, userAgent } = input

  // 1. Never redirect crawlers / link-preview bots.
  if (isBotUserAgent(userAgent)) {
    return { kind: 'pass' }
  }

  // 2. Respect a prior choice recorded in the cookie — no redirect, no re-stamp.
  if (readCookie(cookie, LOCALE_COOKIE_NAME) !== undefined) {
    return { kind: 'pass' }
  }

  const { locale: pathLocale } = parseLocaleFromPath(pathname)

  // 3. An explicit locale prefix in the URL is itself a choice: never redirect
  //    away from it, but stamp the cookie so subsequent visits are stable.
  if (pathLocale !== DEFAULT_LOCALE) {
    return { kind: 'pass', setCookieLocale: pathLocale }
  }

  // 4. First visit to an unprefixed (default-locale) path: negotiate.
  const preferred = parseAcceptLanguage(acceptLanguage)
  if (preferred && preferred !== DEFAULT_LOCALE) {
    const { rest } = parseLocaleFromPath(pathname)
    return {
      kind: 'redirect',
      location: buildLocalePath(preferred, rest),
      cookieLocale: preferred,
    }
  }

  // 5. Default locale is the right answer (matched, absent, or unsupported
  //    header) — pass through, but stamp the cookie so we stop re-negotiating.
  return { kind: 'pass', setCookieLocale: DEFAULT_LOCALE }
}

/** Escape a string for safe insertion into a double-quoted HTML attribute. */
function escapeAttr(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
}

/**
 * Rewrite the static `index.html` document for `locale` at request `pathname`:
 *  - localize og:title / twitter:title / og:description / twitter:description /
 *    the plain `<meta name="description">` and `<title>` to the locale's copy,
 *  - set og:url to the actual requested (locale-prefixed) absolute URL rather
 *    than the hardcoded `https://botho.io/`,
 *  - inject `<link rel="alternate" hreflang="...">` alternates (en / es /
 *    x-default) pointing at the sibling URLs for the current page, and a
 *    self-referential `<link rel="canonical">`.
 *
 * Pure string transform (no DOM, no I/O) so it is unit-testable and cheap at the
 * edge. The English (default-locale) case is returned effectively unchanged
 * except for the injected hreflang/canonical block and the corrected og:url,
 * satisfying the "no metadata regression" acceptance criterion.
 */
export function localizeDocumentHtml(
  html: string,
  locale: SupportedLocale,
  pathname: string,
): string {
  const meta = LOCALE_METADATA[locale]
  const { rest } = parseLocaleFromPath(pathname)

  const canonicalPath = buildLocalePath(locale, rest)
  const canonicalUrl = `${SITE_ORIGIN}${canonicalPath === '/' ? '/' : canonicalPath}`

  let out = html

  // Rewrite meta *content* by matching each tag's stable attribute selector.
  const setMetaContent = (
    selectorAttr: 'property' | 'name',
    selectorValue: string,
    content: string,
  ): void => {
    const re = new RegExp(
      `(<meta\\s+${selectorAttr}="${selectorValue}"\\s+content=")([^"]*)(")`,
      'i',
    )
    out = out.replace(re, `$1${escapeAttr(content)}$3`)
  }

  setMetaContent('name', 'description', meta.description)
  setMetaContent('property', 'og:title', meta.title)
  setMetaContent('property', 'og:description', meta.description)
  setMetaContent('property', 'og:url', canonicalUrl)
  setMetaContent('name', 'twitter:title', meta.title)
  setMetaContent('name', 'twitter:description', meta.description)

  // Localize the <html lang="..."> attribute.
  out = out.replace(/(<html\s+lang=")[^"]*(")/i, `$1${locale}$2`)

  // Localize the document <title>.
  out = out.replace(/<title>[^<]*<\/title>/i, `<title>${escapeAttr(meta.htmlTitle)}</title>`)

  // Build the hreflang alternates + canonical block for the current page.
  const alternates = SUPPORTED_LOCALES.map((loc) => {
    const p = buildLocalePath(loc, rest)
    const href = `${SITE_ORIGIN}${p === '/' ? '/' : p}`
    return `    <link rel="alternate" hreflang="${loc}" href="${escapeAttr(href)}" />`
  })
  const xDefaultPath = buildLocalePath(DEFAULT_LOCALE, rest)
  const xDefaultHref = `${SITE_ORIGIN}${xDefaultPath === '/' ? '/' : xDefaultPath}`
  alternates.push(
    `    <link rel="alternate" hreflang="x-default" href="${escapeAttr(xDefaultHref)}" />`,
  )
  const linkBlock =
    `    <link rel="canonical" href="${escapeAttr(canonicalUrl)}" />\n` +
    alternates.join('\n') +
    '\n'

  // Insert the link block just before </head> (idempotent-ish: only added once
  // per response since we operate on the freshly-fetched static document).
  out = out.replace(/(\s*)<\/head>/i, `\n${linkBlock}$1</head>`)

  return out
}
