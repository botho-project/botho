/**
 * Language switcher (issue #764, phase 1).
 *
 * Toggles the active locale by re-mapping the current URL to the target
 * locale's prefix (default locale = no prefix) and persisting the explicit
 * choice to localStorage. The actual `i18n.changeLanguage` + `<html lang>`
 * update is driven off the URL by `LocaleSync`, so navigation is the single
 * source of truth for the active language.
 */
import { useLocation, useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Globe } from 'lucide-react'
import {
  SUPPORTED_LOCALES,
  storeLocale,
  type SupportedLocale,
} from '../lib/i18n'
import { parseLocalePath, switchLocaleInPath } from '../lib/locale-path'

const LOCALE_LABELS: Record<SupportedLocale, string> = {
  en: 'English',
  es: 'Español',
}

export function LocaleSwitcher({ className = '' }: { className?: string }) {
  const { t } = useTranslation('landing')
  const location = useLocation()
  const navigate = useNavigate()

  const { locale: activeLocale } = parseLocalePath(location.pathname)

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
