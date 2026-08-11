/**
 * Language switcher (issue #764, phase 1).
 *
 * Toggles the active locale by re-mapping the current URL to the target
 * locale's prefix (default locale = no prefix) and persisting the explicit
 * choice to localStorage. The actual `i18n.changeLanguage` + `<html lang>`
 * update is driven off the URL by `LocaleRoutes`, so navigation is the single
 * source of truth for the active language.
 *
 * The displayed value comes from i18next, NOT from `useLocation()`: under a
 * non-default locale `LocaleRoutes` renders pages via
 * `<Routes location={strippedLocation}>`, and React Router provides that
 * locale-STRIPPED location to `useLocation()` in the subtree. Parsing it would
 * always yield `en`, showing "English" on `/es/...` and making the "switch back
 * to English" option a no-op. i18next's language is synced to the real URL by
 * `LocaleRoutes`, so it is correct in both the default and prefixed subtrees.
 * (`switchLocaleInPath` is unaffected — it strips any locale prefix itself, so
 * it maps stripped and unstripped pathnames to the same target.)
 */
import { useLocation, useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Globe } from 'lucide-react'
import {
  DEFAULT_LOCALE,
  isSupportedLocale,
  SUPPORTED_LOCALES,
  storeLocale,
  type SupportedLocale,
} from '../lib/i18n'
import { switchLocaleInPath } from '../lib/locale-path'

const LOCALE_LABELS: Record<SupportedLocale, string> = {
  en: 'English',
  es: 'Español',
  zh: '中文',
}

export function LocaleSwitcher({ className = '' }: { className?: string }) {
  const { t, i18n } = useTranslation('landing')
  const location = useLocation()
  const navigate = useNavigate()

  const language = i18n.resolvedLanguage ?? i18n.language
  const activeLocale: SupportedLocale = isSupportedLocale(language)
    ? language
    : DEFAULT_LOCALE

  function handleChange(next: SupportedLocale) {
    if (next === activeLocale) return
    storeLocale(next)
    const target = switchLocaleInPath(location.pathname, next)
    navigate(`${target}${location.search}${location.hash}`)
  }

  return (
    <label
      className={`inline-flex items-center gap-2 text-ghost ${className}`.trim()}
      aria-label={t('localeSwitcher.label')}
    >
      <Globe size={18} aria-hidden="true" />
      <span className="sr-only">{t('localeSwitcher.label')}</span>
      <select
        value={activeLocale}
        onChange={(e) => handleChange(e.target.value as SupportedLocale)}
        className="bg-transparent text-sm text-ghost hover:text-light focus:text-light focus:outline-none cursor-pointer"
      >
        {SUPPORTED_LOCALES.map((loc) => (
          <option key={loc} value={loc} className="bg-void text-light">
            {LOCALE_LABELS[loc]}
          </option>
        ))}
      </select>
    </label>
  )
}
