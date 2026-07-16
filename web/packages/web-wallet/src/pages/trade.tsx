import { Link } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Logo } from '@botho/ui'
import { BridgeView, useBridgeVenues, useReserveProof } from '@botho/features'
import { ArrowLeft } from 'lucide-react'
import { METRICS_API_BASE } from '../config/fleet'

/**
 * `/trade` — Tier 0 wBTH discovery page (#1030, epic #1029).
 *
 * Mirrors `NetworkPage`: the page owns the data wiring (venue directory + the
 * reserve-proof poll reused from the network module) and hands it to the pure
 * `BridgeView`. No chain client, no wallet-connect, no embedded swap — venues
 * are external deep-links.
 *
 * i18n lives in the `bridge` namespace; `BridgeView` is i18n-runtime-agnostic
 * so `@botho/features` keeps no react-i18next dependency — we pass `t` in.
 */
export function TradePage() {
  const { t } = useTranslation('bridge')
  const { venues } = useBridgeVenues()
  const { proof: reserve, state: reserveState } = useReserveProof(METRICS_API_BASE)

  // On the wallet subdomain the landing lives at `/home`; keep `/` elsewhere so
  // existing nav/e2e behavior is unchanged (mirrors wallet.tsx / #459).
  const homeHref =
    typeof window !== 'undefined' && window.location.hostname.startsWith('wallet.')
      ? '/home'
      : '/'

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to={homeHref} className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">
              {t('meta.titleLong')}
            </span>
            <span className="font-display text-base font-semibold sm:hidden">
              {t('meta.titleShort')}
            </span>
            <span className="rounded-full bg-warning/10 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-warning">
              {t('meta.testnetBadge')}
            </span>
          </Link>
          <nav className="flex items-center gap-4">
            <Link
              to="/network"
              className="text-sm text-ghost hover:text-light transition-colors"
            >
              Network
            </Link>
            <Link
              to="/explorer"
              className="text-sm text-ghost hover:text-light transition-colors"
            >
              Block Explorer
            </Link>
          </nav>
        </div>
      </header>

      <main className="py-6 sm:py-8">
        <div className="max-w-6xl mx-auto px-4 sm:px-6">
          <BridgeView venues={venues} reserve={reserve} reserveState={reserveState} t={t} />
        </div>
      </main>
    </div>
  )
}
