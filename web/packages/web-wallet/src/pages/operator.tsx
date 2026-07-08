import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Logo } from '@botho/ui'
import {
  captureOperatorToken,
  NetworkDashboard,
  TrustDashboard,
  useFleetHistory,
  useFleetStatus,
  useOperatorQuorumInfo,
  useTrustStatus,
} from '@botho/features'
import { ArrowLeft } from 'lucide-react'
import { FLEET, METRICS_API_BASE } from '../config/fleet'

/**
 * Operator dashboard — public read surface (#706, P4.1 of the #695 proposal).
 *
 * Two tabs over exclusively public RPC data — reads only, no auth, no write
 * affordances:
 * - Fleet: the same `NetworkDashboard` + fleet hooks as `/network` (one
 *   implementation, re-parented — not a forked copy).
 * - Trust: per-node quorum posture from the promotion gate (#651/#509)
 *   merged with the live `network_getPeers` table (#544).
 *
 * Each tab mounts its own polling hook, so only the visible tab polls.
 */
type OperatorTab = 'fleet' | 'trust'

export function OperatorPage() {
  const [tab, setTab] = useState<OperatorTab>('fleet')
  // Lift any magic-link read token out of the URL fragment into sessionStorage
  // on mount (#707), then strip it from the address bar. Null ⇒ public view.
  const [token] = useState<string | null>(() => captureOperatorToken())

  return (
    <div className="min-h-screen">
      <header className="border-b border-steel bg-abyss/50 backdrop-blur-md sticky top-0 z-40">
        <div className="max-w-6xl mx-auto px-4 sm:px-6 py-3 sm:py-4 flex items-center justify-between">
          <Link to="/" className="flex items-center gap-2 sm:gap-3">
            <ArrowLeft size={18} className="text-ghost" />
            <Logo size="sm" showText={false} />
            <span className="font-display text-base sm:text-lg font-semibold hidden sm:inline">
              Operator
            </span>
            <span className="font-display text-base font-semibold sm:hidden">Operator</span>
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
        <div className="max-w-6xl mx-auto px-4 sm:px-6 space-y-4">
          <div role="tablist" aria-label="Operator views" className="flex gap-1">
            <TabButton active={tab === 'fleet'} onClick={() => setTab('fleet')}>
              Fleet
            </TabButton>
            <TabButton active={tab === 'trust'} onClick={() => setTab('trust')}>
              Trust
            </TabButton>
          </div>

          {tab === 'fleet' ? <FleetTab /> : <TrustTab token={token} />}
        </div>
      </main>
    </div>
  )
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={`rounded px-3 py-1.5 text-sm transition-colors ${
        active
          ? 'bg-[--color-slate] font-medium text-[--color-light]'
          : 'text-ghost hover:text-light'
      }`}
    >
      {children}
    </button>
  )
}

/** Identical wiring to `/network` — the shared hooks ARE the page logic. */
function FleetTab() {
  const { statuses, avgBlockSeconds } = useFleetStatus(FLEET)
  const { history, historyState } = useFleetHistory(FLEET, METRICS_API_BASE)
  return (
    <NetworkDashboard
      nodes={FLEET}
      statuses={statuses}
      avgBlockSeconds={avgBlockSeconds}
      history={history}
      historyState={historyState}
    />
  )
}

/**
 * Trust tab (#706), upgraded for #707: when a valid read token is present it
 * additionally polls `operator_getQuorumInfo` and renders per-peer
 * classification badges + the configured-members panel. Without a token it
 * degrades cleanly to the public read-only view.
 */
function TrustTab({ token }: { token: string | null }) {
  const { statuses } = useTrustStatus(FLEET)
  const { info, mode } = useOperatorQuorumInfo(FLEET, token)
  return (
    <TrustDashboard
      nodes={FLEET}
      statuses={statuses}
      operatorInfo={token ? info : undefined}
      operatorMode={mode}
    />
  )
}
