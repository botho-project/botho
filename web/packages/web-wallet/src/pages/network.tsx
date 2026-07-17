import { useMemo } from 'react'
import { Link } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Logo } from '@botho/ui'
import {
  NetworkDashboard,
  createBridgeClient,
  useBridgeStats,
  useFleetHistory,
  useFleetStatus,
  useReserveProof,
} from '@botho/features'
import { ArrowLeft } from 'lucide-react'
import { FLEET, METRICS_API_BASE } from '../config/fleet'
import { BRIDGE_API_BASE } from '../config/bridge'

export function NetworkPage() {
  // Polling/history wiring lives in @botho/features hooks (#706) so the
  // /operator fleet tab shares this exact implementation — no forked copies.
  const { statuses, avgBlockSeconds } = useFleetStatus(FLEET)
  const { history, historyState } = useFleetHistory(FLEET, METRICS_API_BASE)
  const { proof: reserve, state: reserveState } = useReserveProof(METRICS_API_BASE)
  // Bridge activity (#1054): same public order API the /trade page uses.
  // Unconfigured (`VITE_BRIDGE_API_BASE` unset) → null client → the card is
  // hidden; configured-but-unreachable degrades to an "unavailable" state.
  const bridgeClient = useMemo(
    () => (BRIDGE_API_BASE ? createBridgeClient(BRIDGE_API_BASE) : null),
    [],
  )
  const { stats: bridgeStats, state: bridgeStatsState } = useBridgeStats(bridgeClient)
  const { t: bridgeT } = useTranslation('bridge')

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">
              Network
            </span>
            <span className="font-display text-base font-semibold sm:hidden">Network</span>
          </Link>
          <nav className="flex items-center gap-4">
            <Link
              to="/operator"
              className="text-sm text-ghost hover:text-light transition-colors"
            >
              Operator
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
          <NetworkDashboard
            nodes={FLEET}
            statuses={statuses}
            avgBlockSeconds={avgBlockSeconds}
            history={history}
            historyState={historyState}
            reserve={reserve}
            reserveState={reserveState}
            bridgeStats={bridgeStats}
            bridgeStatsState={bridgeStatsState}
            bridgeStatsT={bridgeT}
          />
        </div>
      </main>
    </div>
  )
}
