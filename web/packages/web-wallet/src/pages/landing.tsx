import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Logo } from '@botho/ui'
import { Shield, Zap, Lock, Globe, ArrowRight, Github, Menu, X } from 'lucide-react'

const features = [
  {
    icon: Shield,
    title: 'Privacy First',
    description: 'Stealth addresses ensure your transactions remain unlinkable and untraceable.',
  },
  {
    icon: Zap,
    title: 'Fast Consensus',
    description: 'Stellar Consensus Protocol enables fast finality without energy-intensive minting.',
  },
  {
    icon: Lock,
    title: 'Quantum Ready',
    description: 'Post-quantum cryptography protects your privacy against future threats.',
  },
  {
    icon: Globe,
    title: 'Decentralized',
    description: 'Self-organizing quorum with no central authority or trusted third parties.',
  },
]

const stats = [
  { label: 'Block Time', value: '~20s' },
  { label: 'Finality', value: 'Instant' },
  { label: 'Privacy', value: 'Stealth' },
  { label: 'Consensus', value: 'SCP' },
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
            Private Money for the{' '}
            <span className="text-gradient">Quantum Age</span>
          </h1>
          <p className="text-base sm:text-lg md:text-xl text-ghost mb-8 sm:mb-10 max-w-2xl mx-auto px-2">
            Botho is a privacy-focused cryptocurrency with stealth addresses,
            fast consensus, and post-quantum cryptography.
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
            </div>
          ))}
        </div>
      </section>

      {/* Features */}
      <section className="py-16 sm:py-24 px-4 sm:px-6">
        <div className="max-w-6xl mx-auto">
          <h2 className="font-display text-2xl sm:text-3xl md:text-4xl font-bold text-center mb-10 sm:mb-16">
            Built for Privacy
          </h2>
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
        <div className="max-w-6xl mx-auto flex flex-col sm:flex-row items-center justify-between gap-4 sm:gap-6">
          <div className="flex items-center gap-2 sm:gap-3">
            <Logo size="sm" showText={false} />
            <span className="text-ghost text-sm">Botho Project</span>
          </div>
          <div className="flex items-center gap-6 text-sm text-ghost">
            <Link to="/docs" className="hover:text-light transition-colors">
              Documentation
            </Link>
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
      </footer>
    </div>
  )
}
