/**
 * Cloudflare Pages Function middleware (issue #779) — the thin runtime adapter
 * over the pure logic in `lib/locale-negotiation.ts`. It runs on every request
 * to the `botho` Pages project and does two things for HTML document
 * navigations only:
 *
 *   Part A — localize the OG/Twitter/description meta + inject hreflang/canonical
 *            alternates into the served `index.html`, per the resolved locale.
 *   Part B — first-visit Accept-Language negotiation: 302-redirect a fresh
 *            visitor whose browser prefers a supported non-default locale to the
 *            locale-prefixed equivalent path, once (guarded by a cookie).
 *
 * Everything decision-shaped lives in the pure module and is unit-tested; this
 * file only reads headers, calls `next()`, and stitches the decision onto a
 * real `Response`. Non-document requests (assets, /rpc, /api, manifest, service
 * worker, the operator entry) pass straight through untouched.
 *
 * Caching: negotiated / rewritten HTML responses carry `Vary: Accept-Language,
 * Cookie` and `Cache-Control: private, no-store` so a shared cache can never
 * serve one visitor's locale-specific document or redirect to another visitor.
 */
import {
  LOCALE_COOKIE_NAME,
  localizeDocumentHtml,
  negotiateLocaleRedirect,
  parseLocaleFromPath,
} from './lib/locale-negotiation'

// Minimal shape of the Cloudflare Pages Functions context we use. Declared
// locally (rather than depending on `@cloudflare/workers-types`) to keep this
// package's devDependencies unchanged — the fields below are stable public API.
interface PagesContext {
  request: Request
  next: () => Promise<Response>
}

/** Cookie attributes: 1-year, site-wide, lax — a preference marker, not a secret. */
const COOKIE_MAX_AGE = 60 * 60 * 24 * 365
function localeCookie(locale: string): string {
  return (
    `${LOCALE_COOKIE_NAME}=${locale}; Path=/; Max-Age=${COOKIE_MAX_AGE}; ` +
    'SameSite=Lax; Secure'
  )
}

/**
 * Does this request look like a top-level HTML document navigation we should
 * localize / negotiate? We deliberately gate on the `Accept` header and path so
 * asset/API/manifest/service-worker requests pass straight through.
 */
function isDocumentRequest(request: Request, pathname: string): boolean {
  // Never touch the SRI-pinned operator entry or its assets (#772).
  if (pathname === '/operator' || pathname === '/operator.html' || pathname.startsWith('/operator/')) {
    return false
  }
  // Skip well-known non-document roots.
  if (
    pathname.startsWith('/rpc') ||
    pathname.startsWith('/api') ||
    pathname.startsWith('/pkg/') ||
    pathname.startsWith('/assets/') ||
    pathname === '/manifest.webmanifest' ||
    pathname === '/sw.js' ||
    pathname === '/registerSW.js'
  ) {
    return false
  }
  // Skip anything with a file extension (favicon.ico, *.png, *.svg, *.pdf, ...).
  const lastSegment = pathname.slice(pathname.lastIndexOf('/') + 1)
  if (lastSegment.includes('.')) {
    return false
  }
  // Only treat requests that accept HTML as document navigations.
  const accept = request.headers.get('Accept') ?? ''
  return accept.includes('text/html') || accept === '' || accept.includes('*/*')
}

export const onRequest = async (context: PagesContext): Promise<Response> => {
  const { request, next } = context
  const url = new URL(request.url)
  const pathname = url.pathname

  if (request.method !== 'GET' && request.method !== 'HEAD') {
    return next()
  }
  if (!isDocumentRequest(request, pathname)) {
    return next()
  }

  const headers = request.headers
  const decision = negotiateLocaleRedirect({
    pathname,
    acceptLanguage: headers.get('Accept-Language'),
    cookie: headers.get('Cookie'),
    userAgent: headers.get('User-Agent'),
  })

  // Part B: first-visit redirect.
  if (decision.kind === 'redirect') {
    const location = url.origin + decision.location + url.search
    const res = new Response(null, {
      status: 302,
      headers: {
        Location: location,
        'Set-Cookie': localeCookie(decision.cookieLocale),
        // A preference redirect must never be cached and shared across visitors.
        'Cache-Control': 'private, no-store',
        Vary: 'Accept-Language, Cookie',
      },
    })
    return res
  }

  // Fetch the underlying static asset (index.html for SPA routes).
  const response = await next()

  // Only rewrite HTML documents; pass everything else (e.g. the SW fallback
  // serving a non-HTML asset) through untouched.
  const contentType = response.headers.get('Content-Type') ?? ''
  if (!contentType.includes('text/html')) {
    return response
  }

  const { locale } = parseLocaleFromPath(pathname)
  const originalHtml = await response.text()
  const localized = localizeDocumentHtml(originalHtml, locale, pathname)

  const newHeaders = new Headers(response.headers)
  // Locale-specific document: keep shared caches from cross-serving it, and make
  // sure any cache keys on the negotiating inputs.
  newHeaders.set('Cache-Control', 'private, no-store')
  newHeaders.append('Vary', 'Accept-Language')
  newHeaders.append('Vary', 'Cookie')
  if (decision.kind === 'pass' && decision.setCookieLocale) {
    newHeaders.append('Set-Cookie', localeCookie(decision.setCookieLocale))
  }

  return new Response(localized, {
    status: response.status,
    statusText: response.statusText,
    headers: newHeaders,
  })
}
