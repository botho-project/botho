import { useState } from 'react'
import { Link, useLocation } from 'react-router-dom'
import { Logo } from '@botho/ui'
import { ArrowLeft, Book, Code, Shield, Zap, Globe, Terminal, Menu, X } from 'lucide-react'

const sections = [
  {
    id: 'getting-started',
    title: 'Getting Started',
    icon: Book,
    content: `
## Getting Started with Botho

Botho is a privacy-focused cryptocurrency that uses stealth addresses and the Stellar Consensus Protocol (SCP) for fast, secure transactions.

### Creating a Wallet

1. Visit the Wallet page
2. Click "Create Wallet"
3. Write down your 24-word recovery phrase
4. Store it securely - this is the only way to recover your funds

### Receiving Funds

Your wallet address looks like: botho://1/...

Share this address with anyone who wants to send you funds. Each transaction creates a unique one-time address, so your transaction history remains private.
    `,
  },
  {
    id: 'privacy',
    title: 'Privacy Features',
    icon: Shield,
    content: `
## Privacy Features

Botho uses several cryptographic techniques to protect your privacy.

### Stealth Addresses

Every transaction creates a unique one-time address. Even if you share your public address, no one can link your incoming transactions together.

### Post-Quantum Cryptography

Botho supports hybrid quantum-safe transactions using ML-KEM-768 and ML-DSA-65.
    `,
  },
  {
    id: 'consensus',
    title: 'Consensus',
    icon: Zap,
    content: `
## Stellar Consensus Protocol

Botho uses the Stellar Consensus Protocol (SCP) for consensus, providing fast finality without proof-of-work minting.

### Key Properties

- Fast finality: Transactions are final in seconds
- Low energy: No energy-intensive minting
- Decentralized: Each node chooses its own quorum
- Byzantine fault tolerant: Survives malicious nodes
    `,
  },
  {
    id: 'running-node',
    title: 'Running a Node',
    icon: Terminal,
    content: `
## Running a Botho Node

You can run your own Botho node for maximum privacy and to support the network.

### Installation

Clone the repository, build with cargo, and run:

git clone https://github.com/botho-project/botho.git
cd botho
cargo build --release
./target/release/botho init
./target/release/botho run

### CLI Commands

- botho init: Create wallet with 24-word mnemonic
- botho run: Start node and sync blockchain
- botho run --mint: Start node with minting enabled
- botho status: Show sync and wallet status
- botho balance: Show wallet balance
- botho address: Show receiving address
- botho send <addr> <amt>: Send credits
    `,
  },
  {
    id: 'api',
    title: 'API Reference',
    icon: Code,
    content: `
## JSON-RPC API

Botho nodes expose a JSON-RPC 2.0 API on port 8080.

### Endpoints

- getBlockByHeight: Get block by height
- getChainInfo: Get chain information
- getMempoolInfo: Get mempool information
    `,
  },
  {
    id: 'network',
    title: 'Network',
    icon: Globe,
    content: `
## Network Information

### Seed Nodes

- seed.botho.io - Primary seed node

### Network Parameters

- Block time: ~20 seconds
- Consensus: SCP (Federated Byzantine Agreement)
- Privacy: Stealth addresses
- Quantum safety: Optional (ML-KEM + ML-DSA)

### Ports

- 8443: P2P gossip (libp2p)
- 8080: JSON-RPC API
    `,
  },
]

export function DocsPage() {
  const location = useLocation()
  const hash = location.hash.slice(1) || 'getting-started'
  const currentSection = sections.find((s) => s.id === hash) || sections[0]
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false)

  const handleNavClick = () => {
    setMobileMenuOpen(false)
  }

  return (
    <div className="min-h-screen flex flex-col md:flex-row">
      {/* Mobile header */}
      <header className="md:hidden sticky top-0 z-50 bg-abyss/95 backdrop-blur border-b border-steel">
        <div className="flex items-center justify-between px-4 py-3">
          <Link to="/" className="flex items-center gap-2">
            <Logo size="sm" showText={false} />
            <span className="font-display text-base font-semibold">Botho</span>
          </Link>
          <button
            onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
            className="p-2 -mr-2 text-ghost hover:text-light transition-colors"
            aria-label={mobileMenuOpen ? 'Close menu' : 'Open menu'}
          >
            {mobileMenuOpen ? <X size={24} /> : <Menu size={24} />}
          </button>
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
          <Link to="/" className="flex items-center gap-2" onClick={handleNavClick}>
            <Logo size="sm" showText={false} />
            <span className="font-display text-base font-semibold">Botho Docs</span>
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
              to={`/docs#${section.id}`}
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
            to="/"
            onClick={handleNavClick}
            className="flex items-center gap-2 text-ghost hover:text-light transition-colors text-sm"
          >
            <ArrowLeft size={16} />
            Back to home
          </Link>
        </div>
      </aside>

      {/* Desktop sidebar */}
      <aside className="hidden md:block w-64 border-r border-steel bg-abyss/50 fixed top-0 bottom-0 left-0 overflow-y-auto">
        <div className="p-6">
          <Link to="/" className="flex items-center gap-3 mb-8">
            <Logo size="md" showText={false} />
            <span className="font-display text-lg font-semibold">Botho</span>
          </Link>
          <nav className="space-y-1">
            {sections.map((section) => (
              <Link
                key={section.id}
                to={`/docs#${section.id}`}
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
            to="/"
            className="flex items-center gap-2 text-ghost hover:text-light transition-colors text-sm"
          >
            <ArrowLeft size={16} />
            Back to home
          </Link>
        </div>
      </aside>

      {/* Main content */}
      <main className="flex-1 md:ml-64">
        <div className="max-w-3xl mx-auto px-4 sm:px-8 md:px-12 py-8 md:py-16">
          <div className="flex items-center gap-3 mb-6 md:mb-8">
            <currentSection.icon className="text-pulse shrink-0" size={28} />
            <h1 className="font-display text-2xl md:text-3xl font-bold">{currentSection.title}</h1>
          </div>
          <div className="prose prose-invert max-w-none">
            <pre className="whitespace-pre-wrap text-ghost leading-relaxed text-sm md:text-base">
              {currentSection.content.trim()}
            </pre>
          </div>
        </div>
      </main>
    </div>
  )
}
