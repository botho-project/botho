import { Card, CardContent, Button } from '@botho/ui'
import { Search, Loader2 } from 'lucide-react'
import { useExplorer } from '../context'

export interface SearchBarProps {
  /** Placeholder text */
  placeholder?: string
  /** Custom class name */
  className?: string
}

export function SearchBar({
  placeholder = 'Search by block height or hash...',
  className,
}: SearchBarProps) {
  const { searchQuery, setSearchQuery, search, loading } = useExplorer()

  return (
    <Card className={className}>
      <CardContent className="py-4">
        <div className="flex gap-3">
          <div className="relative flex-1">
            <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[--color-dim]" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && search()}
              placeholder={placeholder}
              className="w-full rounded-lg border border-[--color-slate]/50 bg-[--color-void]/50 py-2.5 pl-10 pr-4 font-mono text-sm text-[--color-light] placeholder:text-[--color-dim] focus:border-[--color-pulse] focus:outline-none focus:ring-1 focus:ring-[--color-pulse]"
            />
          </div>
          <Button onClick={search} disabled={loading}>
            {loading ? <Loader2 className="h-4 w-4 animate-spin" /> : 'Search'}
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
