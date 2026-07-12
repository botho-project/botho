import { useEffect, useMemo, useState } from 'react'
import { Link, useLocation, useNavigate, useParams } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Logo } from '@botho/ui'
import { AlertTriangle, ArrowLeft, Menu, X } from 'lucide-react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

import { LocaleSwitcher } from '../components/LocaleSwitcher'
import { DEFAULT_LOCALE, isSupportedLocale } from '../lib/i18n'
import { SECTION_META, getSectionContent, type DocSectionId } from '../docs-content'


export function DocsPage() {
  const location = useLocation()
  const navigate = useNavigate()
  const { t, i18n } = useTranslation('docs')

  // Active locale for content selection + locale-preserving links. Derived from
  // the URL prefix (LocaleRoutes strips `/es` before matching, but `useLocation`
  // in this child still reports the real, prefixed pathname) with i18n.language
  // as the fallback. `id` slugs and the `#hash` scheme are locale-INVARIANT, so
  // only the markdown BODY and the nav/H1 TITLE change with the locale.
  const localeSegment = location.pathname.split('/')[1]
  const activeLocale = isSupportedLocale(localeSegment) ? localeSegment : DEFAULT_LOCALE
  // Non-default locales keep their `/es` prefix on generated links so hash
  // navigation and the path→hash redirect stay within the active language.
  const localePrefix = activeLocale === DEFAULT_LOCALE ? '' : `/${activeLocale}`

  // Build the render-order section list by zipping the locale-invariant meta
  // (id + icon) with the active-locale title and markdown body.
  const sections = useMemo(
    () =>
      SECTION_META.map((meta) => ({
        id: meta.id,
        icon: meta.icon,
        title: t(`sections.${meta.id}`),
        content: getSectionContent(meta.id, activeLocale),
      })),
    // i18n.language re-triggers when the active language changes mid-session.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [t, activeLocale, i18n.language],
  )

  // Path-style deep links (`/docs/<section>`) land here via the `/docs/*`
  // catchall route in App.tsx. The hash form (`/docs#<section>`) is the
  // canonical URL every internal link generates, so known path segments are
  // redirected to it (history `replace` keeps Back from bouncing through the
  // path form). Unknown segments fall through and render a visible
  // "section not found" hint instead of silently showing Getting Started (#656).
  const splat = useParams()['*'] ?? ''
  const pathSegment = splat.replace(/\/+$/, '')
  const normalizedSegment = pathSegment.toLowerCase()
  // A hash always wins over a path segment (e.g. /docs/consensus#privacy).
  const hasHash = location.hash.slice(1) !== ''
  const segmentIsKnown =
    normalizedSegment !== '' && SECTION_META.some((s) => s.id === normalizedSegment)
  const shouldRedirect = segmentIsKnown && !hasHash

  useEffect(() => {
    if (shouldRedirect) {
      navigate(`${localePrefix}/docs#${normalizedSegment}`, { replace: true })
    }
  }, [shouldRedirect, normalizedSegment, navigate, localePrefix])

  const hash = (location.hash.slice(1) || 'getting-started') as DocSectionId
  const currentSection = sections.find((s) => s.id === hash) || sections[0]
  const notFoundSegment = !hasHash && pathSegment !== '' && !segmentIsKnown ? pathSegment : null
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false)

  const handleNavClick = () => {
    setMobileMenuOpen(false)
  }

  return (
    <div className="min-h-screen flex flex-col md:flex-row">
      {/* Mobile header */}
      <header className="md:hidden sticky top-0 z-50 bg-abyss/95 backdrop-blur border-b border-steel">
        <div className="flex items-center justify-between px-4 py-3">
          <Link to={localePrefix || '/'} className="flex items-center gap-2">
            <Logo size="sm" showText={false} />
            <span className="font-display text-base font-semibold">{t('brand')}</span>
          </Link>
          <div className="flex items-center gap-2">
            <LocaleSwitcher />
            <button
              onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
              className="p-2 -mr-2 text-ghost hover:text-light transition-colors"
              aria-label={mobileMenuOpen ? t('closeMenu') : t('openMenu')}
            >
              {mobileMenuOpen ? <X size={24} /> : <Menu size={24} />}
            </button>
          </div>
        </div>
      </header>

      {/* Mobile menu overlay */}
      {mobileMenuOpen && (
        <div
          className="md:hidden fixed inset-0 z-40 bg-void/80 backdrop-blur-sm"
          onClick={() => setMobileMenuOpen(false)}
        />
      )}

      {/* Mobile slide-out menu */}
      <aside
        className={`
          md:hidden fixed top-0 left-0 bottom-0 z-50 w-72 bg-abyss border-r border-steel
          transform transition-transform duration-300 ease-in-out overflow-y-auto
          ${mobileMenuOpen ? 'translate-x-0' : '-translate-x-full'}
        `}
      >
        <div className="p-4 border-b border-steel flex items-center justify-between">
          <Link
            to={localePrefix || '/'}
            className="flex items-center gap-2"
            onClick={handleNavClick}
          >
            <Logo size="sm" showText={false} />
            <span className="font-display text-base font-semibold">{t('brandDocs')}</span>
          </Link>
          <button
            onClick={() => setMobileMenuOpen(false)}
            className="p-2 -mr-2 text-ghost hover:text-light transition-colors"
          >
            <X size={20} />
          </button>
        </div>
        <nav className="p-4 space-y-1">
          {sections.map((section) => (
            <Link
              key={section.id}
              to={`${localePrefix}/docs#${section.id}`}
              onClick={handleNavClick}
              className={`flex items-center gap-3 px-3 py-2.5 rounded-lg transition-colors ${
                currentSection.id === section.id
                  ? 'bg-pulse/10 text-pulse'
                  : 'text-ghost hover:text-light hover:bg-steel/50'
              }`}
            >
              <section.icon size={18} />
              {section.title}
            </Link>
          ))}
        </nav>
        <div className="p-4 border-t border-steel mt-auto">
          <Link
            to={localePrefix || '/'}
            onClick={handleNavClick}
            className="flex items-center gap-2 text-ghost hover:text-light transition-colors text-sm"
          >
            <ArrowLeft size={16} />
            {t('backToHome')}
          </Link>
        </div>
      </aside>

      {/* Desktop sidebar */}
      <aside className="hidden md:block w-64 border-r border-steel bg-abyss/50 fixed top-0 bottom-0 left-0 overflow-y-auto">
        <div className="p-6">
          <div className="flex items-center justify-between mb-8">
            <Link to={localePrefix || '/'} className="flex items-center gap-3">
              <Logo size="md" showText={false} />
              <span className="font-display text-lg font-semibold">{t('brand')}</span>
            </Link>
            <LocaleSwitcher />
          </div>
          <nav className="space-y-1">
            {sections.map((section) => (
              <Link
                key={section.id}
                to={`${localePrefix}/docs#${section.id}`}
                className={`flex items-center gap-3 px-3 py-2 rounded-lg transition-colors ${
                  currentSection.id === section.id
                    ? 'bg-pulse/10 text-pulse'
                    : 'text-ghost hover:text-light hover:bg-steel/50'
                }`}
              >
                <section.icon size={18} />
                {section.title}
              </Link>
            ))}
          </nav>
        </div>
        <div className="p-6 border-t border-steel">
          <Link
            to={localePrefix || '/'}
            className="flex items-center gap-2 text-ghost hover:text-light transition-colors text-sm"
          >
            <ArrowLeft size={16} />
            {t('backToHome')}
          </Link>
        </div>
      </aside>

      {/* Main content */}
      <main className="flex-1 md:ml-64">
        <div className="max-w-3xl mx-auto px-4 sm:px-8 md:px-12 py-8 md:py-16">
          {notFoundSegment && (
            <div
              role="status"
              className="mb-6 flex items-start gap-2 p-3 rounded-lg bg-amber-500/10 border border-amber-500/30 text-amber-200/90 text-sm"
            >
              <AlertTriangle size={16} className="shrink-0 mt-0.5 text-amber-400" />
              <span>{t('notFound', { segment: notFoundSegment })}</span>
            </div>
          )}
          <div className="flex items-center gap-3 mb-6 md:mb-8">
            <currentSection.icon className="text-pulse shrink-0" size={28} />
            <h1 className="font-display text-2xl md:text-3xl font-bold">{currentSection.title}</h1>
          </div>
          <div className="prose prose-invert max-w-none prose-headings:font-display prose-h2:text-xl prose-h2:mt-8 prose-h2:mb-4 prose-h3:text-lg prose-h3:mt-6 prose-h3:mb-3 prose-p:text-ghost prose-p:leading-relaxed prose-li:text-ghost prose-code:bg-steel/50 prose-code:px-1.5 prose-code:py-0.5 prose-code:rounded prose-code:text-pulse prose-code:before:content-none prose-code:after:content-none prose-pre:bg-void prose-pre:border prose-pre:border-steel prose-strong:text-light">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{currentSection.content.trim()}</ReactMarkdown>
          </div>
        </div>
      </main>
    </div>
  )
}
