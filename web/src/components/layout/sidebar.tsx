import { cn } from '@/lib/utils'
import {
  Cpu,
  Database,
  Home,
  Network,
  Settings,
  Wallet,
} from 'lucide-react'
import { Link, useLocation } from 'react-router-dom'

const navigation = [
  { name: 'Dashboard', href: '/', icon: Home },
  { name: 'Wallet', href: '/wallet', icon: Wallet },
  { name: 'Ledger', href: '/ledger', icon: Database },
  { name: 'Network', href: '/network', icon: Network },
  { name: 'Mining', href: '/mining', icon: Cpu },
]

export function Sidebar() {
  const location = useLocation()

  return (
    <aside className="fixed inset-y-0 left-0 z-50 w-64 border-r border-[--color-steel] bg-[--color-abyss]/90 backdrop-blur-xl">
      {/* Logo */}
      <div className="flex h-16 items-center gap-3 border-b border-[--color-steel] px-6">
        <div className="relative h-8 w-8">
          <svg viewBox="0 0 32 32" className="h-8 w-8">
            <defs>
              <linearGradient id="logo-gradient" x1="0%" y1="0%" x2="100%" y2="100%">
                <stop offset="0%" stopColor="var(--color-pulse)" />
                <stop offset="100%" stopColor="var(--color-purple)" />
              </linearGradient>
            </defs>
            <circle cx="16" cy="16" r="14" fill="none" stroke="url(#logo-gradient)" strokeWidth="2" />
            <path
              d="M8 16 Q12 10, 16 16 T24 16"
              fill="none"
              stroke="url(#logo-gradient)"
              strokeWidth="2"
              strokeLinecap="round"
            >
              <animate
                attributeName="d"
                dur="2s"
                repeatCount="indefinite"
                values="M8 16 Q12 10, 16 16 T24 16;M8 16 Q12 22, 16 16 T24 16;M8 16 Q12 10, 16 16 T24 16"
              />
            </path>
          </svg>
        </div>
        <div>
          <h1 className="font-display text-lg font-bold tracking-tight text-gradient">
            Cadence
          </h1>
          <p className="text-xs text-[--color-dim]">Ledger Browser</p>
        </div>
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
        <div className="mt-4 rounded-lg bg-[--color-slate] p-3">
          <div className="flex items-center gap-2">
            <div className="h-2 w-2 rounded-full bg-[--color-success]" />
            <span className="text-xs font-medium text-[--color-soft]">Node Connected</span>
          </div>
          <p className="mt-1 text-xs text-[--color-dim]">localhost:8080</p>
        </div>
      </div>
    </aside>
  )
}
