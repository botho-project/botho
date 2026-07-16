import { useRef, useState } from 'react'
import { TriangleAlert } from 'lucide-react'
import { ReserveProofCard } from '../../network/components/reserve-proof-card'
import type { ReserveProof, ReserveProofState } from '../../network/types'
import { VenueDirectory } from './venue-directory'
import { ExportExplainer } from './export-panel'
import { UnwrapExplainer } from './unwrap-panel'
import type {
  DestinationChain,
  ExportController,
  Translate,
  UnwrapController,
  Venue,
  VenueChain,
} from '../types'

export interface BridgeViewProps {
  /** Venues to list (from `useBridgeVenues`). */
  venues: Venue[]
  /** Latest reserve snapshot (from the network module's `useReserveProof`). */
  reserve: ReserveProof | null
  /** Reserve fetch outcome; `absent` hides the peg card. */
  reserveState: ReserveProofState
  /** `bridge`-namespace translator supplied by the page. */
  t: Translate
  /**
   * Tier 1 integrated-export wiring (#1031): the wallet + bridge-client the
   * `ExportPanel` needs, injected by the page. Optional so the discovery page
   * still renders without it (the panel shows a "not configured" state).
   */
  exportController?: ExportController
  /**
   * Unwrap wiring (#1032): the wallet + release-order client the `UnwrapPanel`
   * needs, injected by the page. Optional so the discovery page still renders
   * without it (the panel shows the destination + guidance, tracking degrades).
   */
  unwrapController?: UnwrapController
  className?: string
}

/**
 * The `/trade` discovery + integrated-export experience.
 *
 * Tier 0 (#1030): a hero + testnet notice, the wBTH venue directory, live peg
 * health (reusing the network module's Proof-of-Reserves card — NOT a fork),
 * and the guided BTH→wBTH export explainer.
 *
 * Tier 1 (#1031): the explainer's panel is now the real integrated export flow,
 * wired via `exportController`. On a completed export the panel's "Trade wBTH
 * now" CTA scrolls to the venue directory with the destination chain
 * highlighted (handled here so the venue coupling stays local to this view).
 *
 * Data plumbing (venue config, reserve polling, wallet/client) is injected by
 * the page so this stays presentation, mirroring `NetworkDashboard`.
 */
export function BridgeView({
  venues,
  reserve,
  reserveState,
  t,
  exportController,
  unwrapController,
  className,
}: BridgeViewProps) {
  const venuesRef = useRef<HTMLDivElement>(null)
  const [highlightChain, setHighlightChain] = useState<VenueChain | null>(null)
  // Export/Unwrap direction toggle (#1032): the two legs of the round trip.
  const [tab, setTab] = useState<'export' | 'unwrap'>('export')

  const onTradeNow = (chain: DestinationChain) => {
    setHighlightChain(chain)
    venuesRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  return (
    <div className={className}>
      {/* Hero + explicit testnet notice. */}
      <section>
        <h1 className="font-display text-3xl font-bold text-[--color-light] sm:text-4xl">
          {t('hero.title')}
        </h1>
        <p className="mt-2 max-w-2xl text-base text-[--color-ghost]">{t('hero.subtitle')}</p>
        <div className="mt-4 flex items-start gap-2 rounded-xl border border-[--color-warning]/30 bg-[--color-warning]/5 px-4 py-3">
          <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0 text-[--color-warning]" />
          <p className="text-sm text-[--color-soft]">{t('hero.testnetNotice')}</p>
        </div>
      </section>

      {/* Peg health — reused Proof-of-Reserves card. */}
      <section className="mt-10">
        <h2 className="font-display text-xl font-semibold text-[--color-light]">
          {t('peg.heading')}
        </h2>
        <p className="mt-1 text-sm text-[--color-dim]">{t('peg.subheading')}</p>
        <ReserveProofCard proof={reserve} state={reserveState} className="mt-4" />
      </section>

      {/* Venue directory (scroll target for the "Trade wBTH now" hand-off). */}
      <div ref={venuesRef}>
        <VenueDirectory
          venues={venues}
          t={t}
          highlightChain={highlightChain}
          className="mt-10"
        />
      </div>

      {/* Export / Unwrap direction toggle — the two legs of the round trip. */}
      <section className="mt-10">
        <div
          role="tablist"
          aria-label={t('tabs.ariaLabel')}
          className="inline-flex rounded-xl border border-[--color-steel] bg-[--color-abyss]/40 p-1"
        >
          <button
            role="tab"
            type="button"
            aria-selected={tab === 'export'}
            onClick={() => setTab('export')}
            className={`rounded-lg px-4 py-1.5 text-sm font-medium transition-colors ${
              tab === 'export'
                ? 'bg-[--color-pulse]/15 text-[--color-light]'
                : 'text-[--color-dim] hover:text-[--color-soft]'
            }`}
          >
            {t('tabs.export')}
          </button>
          <button
            role="tab"
            type="button"
            aria-selected={tab === 'unwrap'}
            onClick={() => setTab('unwrap')}
            className={`rounded-lg px-4 py-1.5 text-sm font-medium transition-colors ${
              tab === 'unwrap'
                ? 'bg-[--color-pulse]/15 text-[--color-light]'
                : 'text-[--color-dim] hover:text-[--color-soft]'
            }`}
          >
            {t('tabs.unwrap')}
          </button>
        </div>

        {tab === 'export' ? (
          <ExportExplainer
            t={t}
            controller={exportController}
            onTradeNow={onTradeNow}
            className="mt-6"
          />
        ) : (
          <UnwrapExplainer t={t} controller={unwrapController} className="mt-6" />
        )}
      </section>
    </div>
  )
}
