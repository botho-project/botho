/**
 * Self-contained, SES-safe i18n for the Snap dialogs (issue #1095).
 *
 * WHY NOT i18next / react-i18next: the Snap bundle runs inside the MetaMask
 * Snaps SES (hardened) executor. `Intl` is not reliably endowed there — the same
 * casualty `src/format.ts` documents for `Number#toLocaleString`. `i18next`
 * pulls in runtime plural/interpolation machinery (`Intl.PluralRules`) and
 * expects a mutable global singleton, so it is fragile-to-broken under SES
 * lockdown and would bloat the bundle for ~30 static strings. This module
 * mirrors the `format.ts` pattern instead: a plain frozen `locale -> key ->
 * string` lookup plus an Intl-free `{placeholder}` substitution (`String#split`/
 * `join`, no `Intl`, no `RegExp` replacement pitfalls).
 *
 * Locale is chosen at runtime from the MetaMask user's preference
 * (`snap_getPreferences.locale`; a restricted permission, so the manifest
 * declares `snap_getPreferences` in `initialPermissions`), narrowed to the
 * Snap's supported set, defaulting to `en`.
 */
import { en } from './locales/en';
import { es } from './locales/es';
import { zh } from './locales/zh';

/** Locales the Snap ships — matches the web wallet's `SUPPORTED_LOCALES`. */
export const SUPPORTED_LOCALES = ['en', 'es', 'zh'] as const;

/** A locale the Snap can render. */
export type Locale = (typeof SUPPORTED_LOCALES)[number];

/** Fallback locale for any MetaMask locale the Snap does not ship. */
export const DEFAULT_LOCALE: Locale = 'en';

/** Every message key; `en` is the source of truth (`es`/`zh` are typed to it). */
export type MessageKey = keyof typeof en;

/** Values interpolated into a `{placeholder}` in a message. */
export type TParams = Record<string, string | number>;

/** Frozen message tables, keyed by locale. */
const MESSAGES: Record<Locale, Record<MessageKey, string>> = {
  en,
  es,
  zh,
};

/**
 * Narrow an arbitrary MetaMask locale string to a supported Snap locale.
 * Accepts region subtags (e.g. `zh-CN` -> `zh`) and is case-insensitive.
 * Anything unsupported (or absent) falls back to {@link DEFAULT_LOCALE}.
 */
export function narrowLocale(raw: unknown): Locale {
  if (typeof raw !== 'string') return DEFAULT_LOCALE;
  const base = raw.toLowerCase().split('-')[0];
  return (SUPPORTED_LOCALES as readonly string[]).includes(base) ? (base as Locale) : DEFAULT_LOCALE;
}

/**
 * Intl-free `{placeholder}` substitution. Uses `split`/`join` so replacement
 * values are inserted literally (no `RegExp` special-char interpretation) and
 * no `Intl` is touched — safe inside the SES sandbox.
 */
function interpolate(template: string, params?: TParams): string {
  if (!params) return template;
  let out = template;
  for (const key of Object.keys(params)) {
    out = out.split(`{${key}}`).join(String(params[key]));
  }
  return out;
}

/**
 * Look up `key` in `locale` (falling back to `en` for an unsupported locale or a
 * missing key) and substitute any `{placeholder}` params. Pure and Intl-free.
 */
export function t(key: MessageKey, locale: Locale, params?: TParams): string {
  const table = MESSAGES[locale] ?? MESSAGES[DEFAULT_LOCALE];
  const template = table[key] ?? MESSAGES[DEFAULT_LOCALE][key];
  return interpolate(template, params);
}

/**
 * Render a count-aware confirmation phrase without `Intl.PluralRules`: picks the
 * `One` message when `count === 1`, else the `Other` message (Chinese ships an
 * identical form since it has no grammatical plural). `count` is exposed as the
 * `{count}` placeholder.
 */
export function confirmationsPhrase(count: number, locale: Locale): string {
  const key: MessageKey = count === 1 ? 'history.confirmationsOne' : 'history.confirmationsOther';
  return t(key, locale, { count });
}

/** Minimal shape of the MetaMask `snap` global used for the preference read. */
declare const snap: {
  request(args: { method: string; params?: unknown }): Promise<unknown>;
};

/**
 * Resolve the dialog locale from the MetaMask user's UI-language preference
 * (`snap_getPreferences.locale`), narrowed to the Snap's supported set and
 * defaulting to `en`. `snap_getPreferences` is a restricted permission declared
 * in the manifest's `initialPermissions`. Any failure (older host that doesn't
 * implement the method, denied request) degrades gracefully to
 * {@link DEFAULT_LOCALE} so dialogs always render.
 */
export async function resolveLocale(): Promise<Locale> {
  try {
    const prefs = (await snap.request({ method: 'snap_getPreferences' })) as
      | { locale?: unknown }
      | null
      | undefined;
    return narrowLocale(prefs?.locale);
  } catch {
    return DEFAULT_LOCALE;
  }
}
