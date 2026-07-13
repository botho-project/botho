import { useState } from 'react'
import { Link } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Button, Logo } from '@botho/ui'
import {Activity, Shield, Scale, Atom, Zap, ArrowRight, Github, Menu, X, FileText, Blocks, Server} from 'lucide-react'
import { LocaleSwitcher } from '../components/LocaleSwitcher'

// Feature/stat metadata is locale-agnostic (icons + translation keys). The
// human-readable title/description/label/note text lives in the `landing`
// namespace resource bundles (issue #764) and is resolved at render time.
const FEATURES = [
  { key: 'private', icon: Shield },
  { key: 'economics', icon: Scale },
  { key: 'quantum', icon: Atom },
  { key: 'finality', icon: Zap },
] as const

const STATS = ['finality', 'ringSize', 'feeRange', 'addresses'] as const

export function LandingPage() {
  const { t } = useTranslation('landing')
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false)

  return (
    <div className="min-h-screen">
      {/* Header */}
      <header className="fixed top-0 left-0 right-0 z-50 backdrop-blur-md bg-void/80 border-b border-steel">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <Logo size="md" showText={false} />
            <span className="font-display text-lg sm:text-xl font-semibold">{t('brand')}</span>
          </Link>

          {/* Desktop nav */}
          {/*
            Collapse to the hamburger at `lg:` (1024px) rather than `md:` (768px):
            the six nav items + switcher + CTA leave no slack in the `max-w-6xl`
            row, so ES's longer labels ("Alojar un nodo", "Informe técnico")
            overflow/crush in the 768–1024px band where EN still just fits (#797).
            `whitespace-nowrap` on every link/CTA additionally forbids two-line
            wrapping (precedent: node.tsx:118).
          */}
          <nav className="hidden lg:flex items-center gap-8">
            <Link to="/explorer" className="text-ghost hover:text-light transition-colors flex items-center gap-2 whitespace-nowrap">
              <Blocks size={18} />
              {t('nav.explorer')}
            </Link>
            <Link to="/network" className="text-ghost hover:text-light transition-colors flex items-center gap-2 whitespace-nowrap">
              <Activity size={18} />
              {t('nav.network')}
            </Link>
            <Link to="/docs" className="text-ghost hover:text-light transition-colors whitespace-nowrap">
              {t('nav.docs')}
            </Link>
            <Link to="/node" className="text-ghost hover:text-light transition-colors flex items-center gap-2 whitespace-nowrap">
              <Server size={18} />
              {t('nav.hostNode')}
            </Link>
            <a
              href="/botho-whitepaper.pdf"
              target="_blank"
              rel="noopener noreferrer"
              className="text-ghost hover:text-light transition-colors flex items-center gap-2 whitespace-nowrap"
            >
              <FileText size={18} />
              {t('nav.whitepaper')}
            </a>
            <a
              href="https://github.com/botho-project/botho"
              target="_blank"
              rel="noopener noreferrer"
              className="text-ghost hover:text-light transition-colors flex items-center gap-2 whitespace-nowrap"
            >
              <Github size={18} />
              {t('nav.github')}
            </a>
            <LocaleSwitcher className="whitespace-nowrap" />
            <Link to="/wallet">
              <Button className="whitespace-nowrap">{t('nav.launchWallet')}</Button>
            </Link>
          </nav>

          {/* Mobile menu button */}
          <button
            onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
            className="lg:hidden p-2 -mr-2 text-ghost hover:text-light transition-colors"
            aria-label={mobileMenuOpen ? t('nav.closeMenu') : t('nav.openMenu')}
          >
            {mobileMenuOpen ? <X size={24} /> : <Menu size={24} />}
          </button>
        </div>

        {/* Mobile menu */}
        {mobileMenuOpen && (
          <div className="lg:hidden border-t border-steel bg-abyss/95 backdrop-blur-md">
            <nav className="px-4 py-4 space-y-1">
              <Link
                to="/explorer"
                onClick={() => setMobileMenuOpen(false)}
                className="flex items-center gap-2 px-4 py-3 rounded-lg text-ghost hover:text-light hover:bg-steel/50 transition-colors"
              >
                <Blocks size={18} />
                {t('nav.blockExplorer')}
              </Link>
              <Link
                to="/docs"
                onClick={() => setMobileMenuOpen(false)}
                className="block px-4 py-3 rounded-lg text-ghost hover:text-light hover:bg-steel/50 transition-colors"
              >
                {t('nav.documentation')}
              </Link>
              <Link
                to="/node"
                onClick={() => setMobileMenuOpen(false)}
                className="flex items-center gap-2 px-4 py-3 rounded-lg text-ghost hover:text-light hover:bg-steel/50 transition-colors"
              >
                <Server size={18} />
                {t('nav.hostNode')}
              </Link>
              <a
                href="/botho-whitepaper.pdf"
                target="_blank"
                rel="noopener noreferrer"
                onClick={() => setMobileMenuOpen(false)}
                className="flex items-center gap-2 px-4 py-3 rounded-lg text-ghost hover:text-light hover:bg-steel/50 transition-colors"
              >
                <FileText size={18} />
                {t('nav.whitepaper')}
              </a>
              <a
                href="https://github.com/botho-project/botho"
                target="_blank"
                rel="noopener noreferrer"
                onClick={() => setMobileMenuOpen(false)}
                className="flex items-center gap-2 px-4 py-3 rounded-lg text-ghost hover:text-light hover:bg-steel/50 transition-colors"
              >
                <Github size={18} />
                {t('nav.github')}
              </a>
              <div className="px-4 py-3">
                <LocaleSwitcher />
              </div>
              <div className="pt-2">
                <Link to="/wallet" onClick={() => setMobileMenuOpen(false)}>
                  <Button className="w-full justify-center whitespace-nowrap">{t('nav.launchWallet')}</Button>
                </Link>
              </div>
            </nav>
          </div>
        )}
      </header>

      {/* Hero */}
      <section className="pt-28 sm:pt-32 pb-16 sm:pb-20 px-4 sm:px-6">
        <div className="max-w-4xl mx-auto text-center">
          <div className="flex flex-col sm:flex-row items-center justify-center gap-2 sm:gap-3 mb-6 sm:mb-8">
            <div className="inline-flex items-center gap-2 px-3 sm:px-4 py-1.5 sm:py-2 rounded-full bg-steel/50 border border-muted text-xs sm:text-sm text-ghost">
              <span className="w-2 h-2 rounded-full bg-success animate-pulse" />
              {t('status.testnetLive')}
            </div>
            <div className="inline-flex items-center gap-2 px-3 sm:px-4 py-1.5 sm:py-2 rounded-full bg-steel/50 border border-muted text-xs sm:text-sm text-ghost">
              <span className="w-2 h-2 rounded-full bg-warning" />
              {t('status.productionPending')}
            </div>
          </div>
          <h1 className="font-display text-4xl sm:text-5xl md:text-7xl font-bold mb-4 sm:mb-6 leading-tight">
            {t('hero.titleLead')}{' '}
            <span className="text-gradient">{t('hero.titleHighlight')}</span>
          </h1>
          <p className="text-base sm:text-lg md:text-xl text-ghost mb-8 sm:mb-10 max-w-2xl mx-auto px-2">
            {t('hero.subtitle')}
          </p>
          <div className="flex flex-col sm:flex-row gap-3 sm:gap-4 justify-center px-4 sm:px-0">
            <Link to="/wallet" className="w-full sm:w-auto">
              <Button size="lg" className="w-full sm:w-auto justify-center">
                {t('hero.openWallet')}
                <ArrowRight className="ml-2" size={18} />
              </Button>
            </Link>
            <Link to="/docs" className="w-full sm:w-auto">
              <Button variant="secondary" size="lg" className="w-full sm:w-auto justify-center px-11">
                {t('hero.readDocs')}
              </Button>
            </Link>
          </div>
        </div>
      </section>

      {/* Stats */}
      <section className="py-8 sm:py-12 px-4 sm:px-6 border-y border-steel bg-abyss/50">
        <div className="max-w-4xl mx-auto grid grid-cols-2 md:grid-cols-4 gap-4 sm:gap-8">
          {STATS.map((stat) => (
            <div key={stat} className="text-center">
              <div className="font-display text-2xl sm:text-3xl font-bold text-pulse mb-0.5 sm:mb-1">
                {t(`stats.${stat}.value`)}
              </div>
              <div className="text-xs sm:text-sm text-ghost">{t(`stats.${stat}.label`)}</div>
              <div className="text-[10px] sm:text-xs text-ghost/80 mt-0.5">{t(`stats.${stat}.note`)}</div>
            </div>
          ))}
        </div>
      </section>

      {/* Features */}
      <section className="py-16 sm:py-24 px-4 sm:px-6">
        <div className="max-w-6xl mx-auto">
          <div className="grid sm:grid-cols-2 gap-4 sm:gap-6 md:gap-8">
            {FEATURES.map((feature) => (
              <div
                key={feature.key}
                className="p-5 sm:p-6 rounded-xl bg-slate/50 border border-steel card-hover"
              >
                <div className="w-10 h-10 sm:w-12 sm:h-12 rounded-lg bg-pulse/10 flex items-center justify-center mb-3 sm:mb-4">
                  <feature.icon className="text-pulse" size={22} />
                </div>
                <h3 className="font-display text-lg sm:text-xl font-semibold mb-1.5 sm:mb-2">
                  {t(`features.${feature.key}.title`)}
                </h3>
                <p className="text-sm sm:text-base text-ghost">{t(`features.${feature.key}.description`)}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* CTA */}
      <section className="py-16 sm:py-24 px-4 sm:px-6 border-t border-steel">
        <div className="max-w-2xl mx-auto text-center">
          <h2 className="font-display text-2xl sm:text-3xl md:text-4xl font-bold mb-4 sm:mb-6">
            {t('cta.title')}
          </h2>
          <p className="text-sm sm:text-base text-ghost mb-6 sm:mb-8 px-2">
            {t('cta.subtitle')}
          </p>
          <div className="flex flex-col sm:flex-row gap-3 sm:gap-4 justify-center px-4 sm:px-0">
            <Link to="/wallet" className="w-full sm:w-auto">
              <Button size="lg" className="w-full sm:w-auto justify-center">
                {t('cta.createWallet')}
                <ArrowRight className="ml-2" size={18} />
              </Button>
            </Link>
            <Link to="/node" className="w-full sm:w-auto">
              <Button variant="secondary" size="lg" className="w-full sm:w-auto justify-center px-8">
                <Server className="mr-2" size={18} />
                {t('cta.hostNode')}
              </Button>
            </Link>
          </div>
        </div>
      </section>

      {/* Footer */}
      <footer className="py-8 sm:py-12 px-4 sm:px-6 border-t border-steel">
        <div className="max-w-6xl mx-auto">
          <div className="flex flex-col sm:flex-row items-center justify-between gap-4 sm:gap-6 mb-6">
            <div className="flex items-center gap-2 sm:gap-3">
              <Logo size="sm" showText={false} />
              <span className="text-ghost text-sm">{t('footer.projectName')}</span>
            </div>
            <div className="flex items-center gap-6 text-sm text-ghost">
              <Link to="/explorer" className="hover:text-light transition-colors">
                {t('footer.explorer')}
              </Link>
              <Link to="/docs" className="hover:text-light transition-colors">
                {t('footer.documentation')}
              </Link>
              <a
                href="/botho-whitepaper.pdf"
                target="_blank"
                rel="noopener noreferrer"
                className="hover:text-light transition-colors"
              >
                {t('footer.whitepaper')}
              </a>
              <a
                href="https://github.com/botho-project/botho"
                target="_blank"
                rel="noopener noreferrer"
                className="hover:text-light transition-colors"
              >
                {t('footer.github')}
              </a>
            </div>
          </div>
          <div className="text-center pt-6 border-t border-steel/50">
            <p className="text-sm text-muted italic">
              {t('footer.motto')}
            </p>
          </div>
        </div>
      </footer>
    </div>
  )
}
