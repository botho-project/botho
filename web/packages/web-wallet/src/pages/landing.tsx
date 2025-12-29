import { Link } from 'react-router-dom'
import { Button, Logo } from '@botho/ui'
import { Shield, Zap, Lock, Globe, ArrowRight, Github } from 'lucide-react'

const features = [
  {
    icon: Shield,
    title: 'Privacy First',
    description: 'Stealth addresses ensure your transactions remain unlinkable and untraceable.',
  },
  {
    icon: Zap,
    title: 'Fast Consensus',
    description: 'Stellar Consensus Protocol enables fast finality without energy-intensive mining.',
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
  return (
    <div className="min-h-screen">
      {/* Header */}
      <header className="fixed top-0 left-0 right-0 z-50 backdrop-blur-md bg-void/80 border-b border-steel">
        <div className="max-w-6xl mx-auto px-6 py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-3">
            <Logo size="lg" showText={false} />
            <span className="font-display text-xl font-semibold">Botho</span>
          </Link>
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
        </div>
      </header>

      {/* Hero */}
      <section className="pt-32 pb-20 px-6">
        <div className="max-w-4xl mx-auto text-center">
          <div className="inline-flex items-center gap-2 px-4 py-2 rounded-full bg-steel/50 border border-muted text-sm text-ghost mb-8">
            <span className="w-2 h-2 rounded-full bg-success animate-pulse" />
            Network is live
          </div>
          <h1 className="font-display text-5xl md:text-7xl font-bold mb-6 leading-tight">
            Private Money for the{' '}
            <span className="text-gradient">Quantum Age</span>
          </h1>
          <p className="text-xl text-ghost mb-10 max-w-2xl mx-auto">
            Botho is a privacy-focused cryptocurrency with stealth addresses,
            fast consensus, and post-quantum cryptography.
          </p>
          <div className="flex flex-col sm:flex-row gap-4 justify-center">
            <Link to="/wallet">
              <Button size="lg" className="w-full sm:w-auto">
                Open Wallet
                <ArrowRight className="ml-2" size={18} />
              </Button>
            </Link>
            <Link to="/docs">
              <Button variant="secondary" size="lg" className="w-full sm:w-auto">
                Read the Docs
              </Button>
            </Link>
          </div>
        </div>
      </section>

      {/* Stats */}
      <section className="py-12 px-6 border-y border-steel bg-abyss/50">
        <div className="max-w-4xl mx-auto grid grid-cols-2 md:grid-cols-4 gap-8">
          {stats.map((stat) => (
            <div key={stat.label} className="text-center">
              <div className="font-display text-3xl font-bold text-pulse mb-1">
                {stat.value}
              </div>
              <div className="text-sm text-ghost">{stat.label}</div>
            </div>
          ))}
        </div>
      </section>

      {/* Features */}
      <section className="py-24 px-6">
        <div className="max-w-6xl mx-auto">
          <h2 className="font-display text-3xl md:text-4xl font-bold text-center mb-16">
            Built for Privacy
          </h2>
          <div className="grid md:grid-cols-2 gap-8">
            {features.map((feature) => (
              <div
                key={feature.title}
                className="p-6 rounded-xl bg-slate/50 border border-steel card-hover"
              >
                <div className="w-12 h-12 rounded-lg bg-pulse/10 flex items-center justify-center mb-4">
                  <feature.icon className="text-pulse" size={24} />
                </div>
                <h3 className="font-display text-xl font-semibold mb-2">
                  {feature.title}
                </h3>
                <p className="text-ghost">{feature.description}</p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* CTA */}
      <section className="py-24 px-6 border-t border-steel">
        <div className="max-w-2xl mx-auto text-center">
          <h2 className="font-display text-3xl md:text-4xl font-bold mb-6">
            Ready to Get Started?
          </h2>
          <p className="text-ghost mb-8">
            Create a wallet in seconds. No email, no KYC, no tracking.
          </p>
          <Link to="/wallet">
            <Button size="lg">
              Create Wallet
              <ArrowRight className="ml-2" size={18} />
            </Button>
          </Link>
        </div>
      </section>

      {/* Footer */}
      <footer className="py-12 px-6 border-t border-steel">
        <div className="max-w-6xl mx-auto flex flex-col md:flex-row items-center justify-between gap-6">
          <div className="flex items-center gap-3">
            <Logo size="sm" showText={false} />
            <span className="text-ghost">Botho Project</span>
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
