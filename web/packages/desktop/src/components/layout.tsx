import type { ReactNode } from 'react'
import { Link, useLocation } from 'react-router-dom'
import { Logo } from '@botho/ui'
import { cn } from '@botho/ui'
import { useConnection } from '../contexts/connection'
import {
  Cpu,
  Database,
  Home,
  LogOut,
  Network,
  Settings,
  Signal,
  Wallet,
} from 'lucide-react'

const navigation = [
  { name: 'Dashboard', href: '/', icon: Home },
  { name: 'Wallet', href: '/wallet', icon: Wallet },
  { name: 'Ledger', href: '/ledger', icon: Database },
  { name: 'Network', href: '/network', icon: Network },
  { name: 'Mining', href: '/mining', icon: Cpu },
]

interface LayoutProps {
  children: ReactNode
  title: string
  subtitle?: string
}

export function Layout({ children, title, subtitle }: LayoutProps) {
  return (
    <div className="min-h-screen">
      <Sidebar />
      <div className="pl-64">
        <Header title={title} subtitle={subtitle} />
        <main className="grid-pattern min-h-[calc(100vh-4rem)] p-6">
          {children}
        </main>
      </div>
    </div>
  )
}

function Header({ title, subtitle }: { title: string; subtitle?: string }) {
  return (
    <header className="sticky top-0 z-40 border-b border-[--color-steel] bg-[--color-void]/80 backdrop-blur-xl">
      <div className="flex h-16 items-center justify-between px-6">
        <div>
          <h1 className="font-display text-xl font-bold text-[--color-light]">{title}</h1>
          {subtitle && <p className="text-sm text-[--color-dim]">{subtitle}</p>}
        </div>
      </div>
    </header>
  )
}

function Sidebar() {
  const location = useLocation()
  const { connectedNode, disconnect } = useConnection()

  return (
    <aside className="fixed inset-y-0 left-0 z-50 w-64 border-r border-[--color-steel] bg-[--color-abyss]/90 backdrop-blur-xl">
      {/* Logo */}
      <div className="flex h-16 items-center border-b border-[--color-steel] px-6">
        <Logo />
      </div>

      {/* Navigation */}
      <nav className="flex-1 space-y-1 px-3 py-4">
        {navigation.map((item) => {
          const isActive = location.pathname === item.href
          return (
            <Link
              key={item.name}
              to={item.href}
              className={cn(
                'group flex items-center gap-3 rounded-lg px-3 py-2.5 text-sm font-medium transition-all duration-200',
                isActive
                  ? 'bg-[--color-pulse]/10 text-[--color-pulse] glow-sm'
                  : 'text-[--color-ghost] hover:bg-[--color-steel]/50 hover:text-[--color-soft]'
              )}
            >
              <item.icon
                className={cn(
                  'h-5 w-5 transition-colors',
                  isActive ? 'text-[--color-pulse]' : 'text-[--color-dim] group-hover:text-[--color-ghost]'
                )}
              />
              {item.name}
              {isActive && (
                <div className="ml-auto h-1.5 w-1.5 rounded-full bg-[--color-pulse] pulse-indicator" />
              )}
            </Link>
          )
        })}
      </nav>

      {/* Footer */}
      <div className="border-t border-[--color-steel] p-4">
        <Link
          to="/settings"
          className="flex items-center gap-3 rounded-lg px-3 py-2.5 text-sm font-medium text-[--color-ghost] transition-colors hover:bg-[--color-steel]/50 hover:text-[--color-soft]"
        >
          <Settings className="h-5 w-5 text-[--color-dim]" />
          Settings
        </Link>

        {/* Connected node info */}
        {connectedNode && (
          <div className="mt-4 rounded-lg bg-[--color-slate] p-3">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <div className="h-2 w-2 rounded-full bg-[--color-success] animate-pulse" />
                <span className="text-xs font-medium text-[--color-soft]">Connected</span>
              </div>
              <button
                onClick={disconnect}
                className="rounded p-1 text-[--color-dim] transition-colors hover:bg-[--color-abyss] hover:text-[--color-danger]"
                title="Disconnect"
              >
                <LogOut className="h-3.5 w-3.5" />
              </button>
            </div>
            <p className="mt-1 font-mono text-xs text-[--color-dim]">
              {connectedNode.host}:{connectedNode.port}
            </p>
            <div className="mt-2 flex items-center gap-2 text-xs text-[--color-dim]">
              <Signal className="h-3 w-3 text-[--color-success]" />
              <span>{connectedNode.latency}ms</span>
              {connectedNode.blockHeight && (
                <>
                  <span className="text-[--color-steel]">•</span>
                  <span>Block {connectedNode.blockHeight.toLocaleString()}</span>
                </>
              )}
            </div>
          </div>
        )}
      </div>
    </aside>
  )
}
