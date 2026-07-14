/**
 * Dependency-free locale constants — the single source of truth for the set of
 * locales the app supports and which one is the (unprefixed) default.
 *
 * This module intentionally has NO imports (no i18next, no JSON resource
 * bundles). It is split out of `i18n.ts` (which re-exports everything here so
 * existing importers are unchanged) so that build targets which must NOT pull in
 * the whole i18next runtime — notably the Cloudflare Pages Function under
 * `functions/` that does edge Accept-Language negotiation (issue #779) — can
 * import the locale list from here and stay in lockstep with the SPA without
 * duplicating the list. Add a new locale HERE (and its resource bundles in
 * `i18n.ts`) to light up an additional language everywhere at once.
 */

/**
 * The set of locales the app knows how to render. `en` MUST stay first — it is
 * the default and the unprefixed locale.
 */
export const SUPPORTED_LOCALES = ['en', 'es', 'zh'] as const

export type SupportedLocale = (typeof SUPPORTED_LOCALES)[number]

/** The default locale, served without a URL prefix. */
export const DEFAULT_LOCALE: SupportedLocale = 'en'

/** Type guard: is `value` one of the locales we actually support? */
export function isSupportedLocale(value: string | undefined | null): value is SupportedLocale {
  return value != null && (SUPPORTED_LOCALES as readonly string[]).includes(value)
}
