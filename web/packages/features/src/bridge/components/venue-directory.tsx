import { VenueCard } from './venue-card'
import type { Translate, Venue, VenueChain } from '../types'

export interface VenueDirectoryProps {
  venues: Venue[]
  /** `bridge`-namespace translator supplied by the page. */
  t: Translate
  /**
   * Chain to visually emphasize (#1031): after a completed export the panel's
   * "Trade wBTH now" CTA highlights the matching-chain venues here. `null`
   * highlights nothing.
   */
  highlightChain?: VenueChain | null
  className?: string
}

/**
 * The wBTH venue directory (#1030): a responsive grid of {@link VenueCard}s.
 * Renders an empty-state note when a network has no configured venues (e.g. the
 * still-empty `mainnet` set in `venues.ts`).
 */
export function VenueDirectory({ venues, t, highlightChain, className }: VenueDirectoryProps) {
  return (
    <section className={className}>
      <h2 className="font-display text-xl font-semibold text-[--color-light]">
        {t('venues.heading')}
      </h2>
      <p className="mt-1 text-sm text-[--color-dim]">{t('venues.subheading')}</p>

      {venues.length === 0 ? (
        <p className="mt-4 text-sm text-[--color-dim]">{t('venues.empty')}</p>
      ) : (
        <div className="mt-4 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {venues.map((venue) => (
            <VenueCard
              key={venue.id}
              venue={venue}
              t={t}
              highlight={highlightChain != null && venue.chain === highlightChain}
            />
          ))}
        </div>
      )}
    </section>
  )
}
