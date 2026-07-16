import { Card, CardContent } from '@botho/ui'
import { ArrowUpRight, Boxes, Clock, Waves, Zap } from 'lucide-react'
import type { Translate, Venue, VenueChain } from '../types'

export interface VenueCardProps {
  venue: Venue
  /** `bridge`-namespace translator supplied by the page. */
  t: Translate
  className?: string
}

const CHAIN_ICON: Record<VenueChain, typeof Boxes> = {
  ethereum: Boxes,
  solana: Zap,
  hyperliquid: Waves,
}

/** `0x1234…abcd` — short, monospace-friendly address for display only. */
function shortAddress(addr: string): string {
  if (addr.length <= 12) return addr
  return `${addr.slice(0, 6)}…${addr.slice(-4)}`
}

/**
 * A single wBTH venue (#1030): chain + pair, the deployed token/pool
 * addresses, and an external "Trade on X ↗" deep-link. Presentation only — no
 * chain client, no wallet-connect. `coming-soon` venues render a badge and no
 * trade link (e.g. Hyperliquid HIP-1 spot, pending #877).
 */
export function VenueCard({ venue, t, className }: VenueCardProps) {
  const Icon = CHAIN_ICON[venue.chain]
  const isLive = venue.status === 'live' && !!venue.tradeUrl

  return (
    <Card className={className}>
      <CardContent className="flex h-full flex-col p-5">
        <div className="flex items-start justify-between gap-3">
          <div className="flex items-center gap-2.5">
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-[--color-steel]/50">
              <Icon className="h-5 w-5 text-[--color-pulse]" />
            </div>
            <div>
              <div className="font-display text-base font-semibold text-[--color-light]">
                {venue.venueName}
              </div>
              <div className="text-xs text-[--color-dim]">{venue.chainLabel}</div>
            </div>
          </div>
          {isLive ? (
            <span className="rounded-full bg-emerald-400/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-emerald-400">
              {t('venues.testnet')}
            </span>
          ) : (
            <span className="flex items-center gap-1 rounded-full bg-[--color-warning]/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-[--color-warning]">
              <Clock className="h-3 w-3" />
              {t('venues.comingSoon')}
            </span>
          )}
        </div>

        <div className="mt-4 text-sm font-medium text-[--color-soft]">{venue.pairLabel}</div>

        <dl className="mt-3 space-y-1.5 text-xs">
          <div className="flex items-center justify-between gap-2">
            <dt className="text-[--color-dim]">{t('venues.tokenAddress')}</dt>
            <dd className="font-mono text-[--color-soft]" title={venue.tokenAddress}>
              {shortAddress(venue.tokenAddress)}
            </dd>
          </div>
          {venue.poolAddress && (
            <div className="flex items-center justify-between gap-2">
              <dt className="text-[--color-dim]">{t('venues.poolAddress')}</dt>
              <dd className="font-mono text-[--color-soft]" title={venue.poolAddress}>
                {shortAddress(venue.poolAddress)}
              </dd>
            </div>
          )}
        </dl>

        <div className="mt-auto flex flex-wrap items-center gap-3 pt-4">
          {isLive ? (
            <a
              href={venue.tradeUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1.5 rounded-xl bg-[--color-pulse] px-4 py-2 text-sm font-semibold text-[--color-void] transition-all hover:bg-[--color-pulse-dim]"
            >
              {t('venues.tradeOn', { venue: venue.venueName })}
              <ArrowUpRight className="h-4 w-4" />
            </a>
          ) : (
            <span className="text-xs text-[--color-dim]">{t('venues.comingSoonNote')}</span>
          )}
          {venue.explorerUrl && (
            <a
              href={venue.explorerUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 text-sm text-[--color-ghost] transition-colors hover:text-[--color-light]"
            >
              {t('venues.viewPool')}
              <ArrowUpRight className="h-3.5 w-3.5" />
            </a>
          )}
        </div>
      </CardContent>
    </Card>
  )
}
