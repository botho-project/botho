/**
 * Locale-aware path helpers for URL-prefix locale routing (issue #764, phase 1).
 *
 * English (the default) is served WITHOUT a prefix, so `/`, `/wallet`, ... keep
 * their existing meaning and every existing e2e test (`a[href="/wallet"]`, the
 * wallet-host redirect, etc.) continues to pass. Non-default locales get a
 * leading segment, e.g. `/es`, `/es/wallet`.
 */
import {
  DEFAULT_LOCALE,
  isSupportedLocale,
  type SupportedLocale,
} from './i18n'

/**
 * Split a pathname into its (optional) leading locale segment and the rest of
 * the path. An unsupported or absent leading segment yields the default locale
 * and the path unchanged.
 *
 * `parseLocalePath('/es/wallet')  -> { locale: 'es', rest: '/wallet' }`
 * `parseLocalePath('/wallet')     -> { locale: 'en', rest: '/wallet' }`
 * `parseLocalePath('/es')         -> { locale: 'es', rest: '/' }`
 * `parseLocalePath('/xx/wallet')  -> { locale: 'en', rest: '/xx/wallet' }`
 */
export function parseLocalePath(pathname: string): {
  locale: SupportedLocale
  rest: string
} {
  const segments = pathname.split('/')
  // segments[0] is always '' for an absolute path; the first real segment is [1].
  const first = segments[1]
  if (isSupportedLocale(first) && first !== DEFAULT_LOCALE) {
    const rest = '/' + segments.slice(2).join('/')
    return { locale: first, rest: rest === '/' ? '/' : rest.replace(/\/$/, '') || '/' }
  }
  return { locale: DEFAULT_LOCALE, rest: pathname || '/' }
}

/**
 * Build a pathname for `rest` under `locale`. The default locale is unprefixed;
 * every other locale gets a `/<locale>` prefix.
 *
 * `buildLocalePath('en', '/wallet') -> '/wallet'`
 * `buildLocalePath('es', '/wallet') -> '/es/wallet'`
 * `buildLocalePath('es', '/')       -> '/es'`
 */
export function buildLocalePath(locale: SupportedLocale, rest: string): string {
  const normalizedRest = rest.startsWith('/') ? rest : `/${rest}`
  if (locale === DEFAULT_LOCALE) {
    return normalizedRest
  }
  if (normalizedRest === '/') {
    return `/${locale}`
  }
  return `/${locale}${normalizedRest}`
}

/**
 * Re-map a full pathname from its current locale to `targetLocale`, preserving
 * the non-locale part of the path. Used by the locale switcher so toggling the
 * language keeps the visitor on the same page.
 */
export function switchLocaleInPath(pathname: string, targetLocale: SupportedLocale): string {
  const { rest } = parseLocalePath(pathname)
  return buildLocalePath(targetLocale, rest)
}
