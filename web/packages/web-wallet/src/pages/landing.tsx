import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Logo } from '@botho/ui'
import { Shield, Scale, Atom, Zap, ArrowRight, Github, Menu, X, FileText } from 'lucide-react'

const features = [
  {
    icon: Shield,
    title: 'Private by Default',
    description: 'Every transaction uses ring signatures. No transparent mode, no T-addr mistake. Privacy is the baseline, not a premium feature.',
  },
  {
    icon: Scale,
    title: 'Progressive Economics',
    description: 'Fees based on coin provenance, not identity. Fresh mints pay more; well-traded coins pay less. 80% of fees redistributed via lottery that favors small holders. Splitting doesn\'t help—tags track origin, not amount.',
  },
  {
    icon: Atom,
    title: 'Quantum-Safe Where It Matters',
    description: 'Recipient addresses use ML-KEM-768—quantum computers can\'t trace who received funds. Amount commitments are information-theoretically secure. Sender privacy via efficient CLSAG ring signatures.',
  },
  {
    icon: Zap,
    title: 'Instant Finality',
    description: 'SCP consensus means transactions are final in seconds. No reorgs, no 6-block waits, no double-spend risk.',
  },
]

const stats = [
  { label: 'Finality', value: '<5s', note: 'SCP consensus' },
  { label: 'Ring Size', value: '20', note: 'CLSAG signatures' },
  { label: 'Fee Range', value: '1-6x', note: 'Based on provenance' },
  { label: 'Addresses', value: 'ML-KEM', note: 'Quantum-safe' },
]

export function LandingPage() {
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false)

  return (
    <div className="min-h-screen">
      {/* Header */}
      <header className="fixed top-0 left-0 right-0 z-50 backdrop-blur-md bg-void/80 border-b border-steel">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <Logo size="md" showText={false} />
            <span className="font-display text-lg sm:text-xl font-semibold">Botho</span>
          </Link>

          {/* Desktop nav */}
          <nav className="hidden md:flex items-center gap-8">
            <Link to="/docs" className="text-ghost hover:text-light transition-colors">
              Docs
            </Link>
            <a
              href="/botho-whitepaper.pdf"
              target="_blank"
              rel="noopener noreferrer"
              className="text-ghost hover:text-light transition-colors flex items-center gap-2"
            >
              <FileText size={18} />
              Whitepaper
            </a>
            <a
              href="https://github.com/botho-project/botho"
              target="_blank"
              rel="noopener noreferrer"
              className="text-ghost hover:text-light transition-colors flex items-center gap-2"
            >
              <Github size={18} />
              GitHub
            </a>
            <Link to="/wallet">
              <Button>Launch Wallet</Button>
            </Link>
          </nav>

          {/* Mobile menu button */}
          <button
            onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
            className="md:hidden p-2 -mr-2 text-ghost hover:text-light transition-colors"
            aria-label={mobileMenuOpen ? 'Close menu' : 'Open menu'}
          >
            {mobileMenuOpen ? <X size={24} /> : <Menu size={24} />}
          </button>
        </div>

        {/* Mobile menu */}
        {mobileMenuOpen && (
          <div className="md:hidden border-t border-steel bg-abyss/95 backdrop-blur-md">
            <nav className="px-4 py-4 space-y-1">
              <Link
                to="/docs"
                onClick={() => setMobileMenuOpen(false)}
                className="block px-4 py-3 rounded-lg text-ghost hover:text-light hover:bg-steel/50 transition-colors"
              >
                Documentation
              </Link>
              <a
                href="/botho-whitepaper.pdf"
                target="_blank"
                rel="noopener noreferrer"
                onClick={() => setMobileMenuOpen(false)}
                className="flex items-center gap-2 px-4 py-3 rounded-lg text-ghost hover:text-light hover:bg-steel/50 transition-colors"
              >
                <FileText size={18} />
                Whitepaper
              </a>
              <a
                href="https://github.com/botho-project/botho"
                target="_blank"
                rel="noopener noreferrer"
                onClick={() => setMobileMenuOpen(false)}
                className="flex items-center gap-2 px-4 py-3 rounded-lg text-ghost hover:text-light hover:bg-steel/50 transition-colors"
              >
                <Github size={18} />
                GitHub
              </a>
              <div className="pt-2">
                <Link to="/wallet" onClick={() => setMobileMenuOpen(false)}>
                  <Button className="w-full justify-center">Launch Wallet</Button>
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
              Test network is live
            </div>
            <div className="inline-flex items-center gap-2 px-3 sm:px-4 py-1.5 sm:py-2 rounded-full bg-steel/50 border border-muted text-xs sm:text-sm text-ghost">
              <span className="w-2 h-2 rounded-full bg-warning" />
              Production network pending
            </div>
          </div>
          <h1 className="font-display text-4xl sm:text-5xl md:text-7xl font-bold mb-4 sm:mb-6 leading-tight">
            Privacy Currency for the{' '}
            <span className="text-gradient">Quantum Era</span>
          </h1>
          <p className="text-base sm:text-lg md:text-xl text-ghost mb-8 sm:mb-10 max-w-2xl mx-auto px-2">
            Instant SCP finality. Quantum-safe recipient addresses. Progressive economics that reward circulation over hoarding.
          </p>
          <div className="flex flex-col sm:flex-row gap-3 sm:gap-4 justify-center px-4 sm:px-0">
            <Link to="/wallet" className="w-full sm:w-auto">
              <Button size="lg" className="w-full sm:w-auto justify-center">
                Open Wallet
                <ArrowRight className="ml-2" size={18} />
              </Button>
            </Link>
            <Link to="/docs" className="w-full sm:w-auto">
              <Button variant="secondary" size="lg" className="w-full sm:w-auto justify-center px-11">
                Read the Docs
              </Button>
            </Link>
          </div>
        </div>
      </section>

      {/* Stats */}
      <section className="py-8 sm:py-12 px-4 sm:px-6 border-y border-steel bg-abyss/50">
        <div className="max-w-4xl mx-auto grid grid-cols-2 md:grid-cols-4 gap-4 sm:gap-8">
          {stats.map((stat) => (
            <div key={stat.label} className="text-center">
              <div className="font-display text-2xl sm:text-3xl font-bold text-pulse mb-0.5 sm:mb-1">
                {stat.value}
              </div>
              <div className="text-xs sm:text-sm text-ghost">{stat.label}</div>
              {stat.note && (
                <div className="text-[10px] sm:text-xs text-ghost/80 mt-0.5">{stat.note}</div>
              )}
            </div>
          ))}
        </div>
      </section>

      {/* Features */}
      <section className="py-16 sm:py-24 px-4 sm:px-6">
        <div className="max-w-6xl mx-auto">
          <div className="grid sm:grid-cols-2 gap-4 sm:gap-6 md:gap-8">
            {features.map((feature) => (
              <div
                key={feature.title}
                className="p-5 sm:p-6 rounded-xl bg-slate/50 border border-steel card-hover"
              >
                <div className="w-10 h-10 sm:w-12 sm:h-12 rounded-lg bg-pulse/10 flex items-center justify-center mb-3 sm:mb-4">
                  <feature.icon className="text-pulse" size={22} />
                </div>
                <h3 className="font-display text-lg sm:text-xl font-semibold mb-1.5 sm:mb-2">
                  {feature.title}
                </h3>
                <p className="text-sm sm:text-base text-ghost">{feature.description}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* CTA */}
      <section className="py-16 sm:py-24 px-4 sm:px-6 border-t border-steel">
        <div className="max-w-2xl mx-auto text-center">
          <h2 className="font-display text-2xl sm:text-3xl md:text-4xl font-bold mb-4 sm:mb-6">
            Ready to Get Started?
          </h2>
          <p className="text-sm sm:text-base text-ghost mb-6 sm:mb-8 px-2">
            Create a wallet in seconds. No email, no KYC, no tracking.
          </p>
          <Link to="/wallet">
            <Button size="lg" className="w-full sm:w-auto justify-center">
              Create Wallet
              <ArrowRight className="ml-2" size={18} />
            </Button>
          </Link>
        </div>
      </section>

      {/* Footer */}
      <footer className="py-8 sm:py-12 px-4 sm:px-6 border-t border-steel">
        <div className="max-w-6xl mx-auto">
          <div className="flex flex-col sm:flex-row items-center justify-between gap-4 sm:gap-6 mb-6">
            <div className="flex items-center gap-2 sm:gap-3">
              <Logo size="sm" showText={false} />
              <span className="text-ghost text-sm">Botho Project</span>
            </div>
            <div className="flex items-center gap-6 text-sm text-ghost">
              <Link to="/docs" className="hover:text-light transition-colors">
                Documentation
              </Link>
              <a
                href="/botho-whitepaper.pdf"
                target="_blank"
                rel="noopener noreferrer"
                className="hover:text-light transition-colors"
              >
                Whitepaper
              </a>
              <a
                href="https://github.com/botho-project/botho"
                target="_blank"
                rel="noopener noreferrer"
                className="hover:text-light transition-colors"
              >
                GitHub
              </a>
            </div>
          </div>
          <div className="text-center pt-6 border-t border-steel/50">
            <p className="text-sm text-muted italic">
              "Motho ke motho ka batho" — A person is a person through other people
            </p>
          </div>
        </div>
      </footer>
    </div>
  )
}
