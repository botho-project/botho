import { Bell, Search, RefreshCw } from 'lucide-react'
import { useState } from 'react'
import { cn } from '@/lib/utils'

interface HeaderProps {
  title: string
  subtitle?: string
}

export function Header({ title, subtitle }: HeaderProps) {
  const [isRefreshing, setIsRefreshing] = useState(false)

  const handleRefresh = () => {
    setIsRefreshing(true)
    setTimeout(() => setIsRefreshing(false), 1000)
  }

  return (
    <header className="sticky top-0 z-40 border-b border-[--color-steel] bg-[--color-void]/80 backdrop-blur-xl">
      <div className="flex h-16 items-center justify-between px-6">
        <div>
          <h1 className="font-display text-xl font-bold text-[--color-light]">{title}</h1>
          {subtitle && <p className="text-sm text-[--color-dim]">{subtitle}</p>}
        </div>

        <div className="flex items-center gap-4">
          {/* Search */}
          <div className="relative">
            <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[--color-dim]" />
            <input
              type="text"
              placeholder="Search blocks, txs, addresses..."
              className="h-10 w-72 rounded-lg border border-[--color-steel] bg-[--color-slate] pl-10 pr-4 text-sm text-[--color-light] placeholder:text-[--color-dim] focus:border-[--color-pulse-dim] focus:outline-none focus:ring-1 focus:ring-[--color-pulse-dim]"
            />
            <kbd className="absolute right-3 top-1/2 -translate-y-1/2 rounded border border-[--color-muted] bg-[--color-steel] px-1.5 py-0.5 text-xs text-[--color-dim]">
              ⌘K
            </kbd>
          </div>

          {/* Refresh */}
          <button
            onClick={handleRefresh}
            className="flex h-10 w-10 items-center justify-center rounded-lg border border-[--color-steel] bg-[--color-slate] text-[--color-ghost] transition-colors hover:border-[--color-pulse-dim] hover:text-[--color-pulse]"
          >
            <RefreshCw className={cn('h-4 w-4', isRefreshing && 'animate-spin')} />
          </button>

          {/* Notifications */}
          <button className="relative flex h-10 w-10 items-center justify-center rounded-lg border border-[--color-steel] bg-[--color-slate] text-[--color-ghost] transition-colors hover:border-[--color-pulse-dim] hover:text-[--color-pulse]">
            <Bell className="h-4 w-4" />
            <span className="absolute -right-1 -top-1 flex h-4 w-4 items-center justify-center rounded-full bg-[--color-danger] text-[10px] font-bold text-white">
              3
            </span>
          </button>
        </div>
      </div>
    </header>
  )
}
