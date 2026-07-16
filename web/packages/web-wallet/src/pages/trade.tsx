import { useMemo } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { Logo } from '@botho/ui'
import {
  ACTIVE_BRIDGE_NETWORK,
  BridgeView,
  createBridgeClient,
  useBridgeVenues,
  useReserveProof,
  type ExportController,
} from '@botho/features'
import { ArrowLeft } from 'lucide-react'
import { METRICS_API_BASE } from '../config/fleet'
import { BRIDGE_API_BASE } from '../config/bridge'
import { useWallet } from '../contexts/wallet'

/**
 * `/trade` — wBTH discovery (Tier 0, #1030) + integrated BTH→wBTH export
 * (Tier 1, #1031).
 *
 * Mirrors `NetworkPage`: the page owns the data + wallet wiring and hands it to
 * the pure `BridgeView`. For the integrated export it builds an
 * `ExportController` from the wallet context — crucially, the BTH deposit is
 * built/signed/submitted by the SAME `send()` path the wallet uses for every
 * transfer (`@botho/wasm-signer`), so no new signing code is introduced and the
 * wallet never touches the counterparty chain. wBTH is minted by the bridge to
 * the user's own EVM/SVM wallet.
 *
 * i18n lives in the `bridge` namespace; `BridgeView` is i18n-runtime-agnostic
 * so `@botho/features` keeps no react-i18next dependency — we pass `t` in.
 */
export function TradePage() {
  const { t } = useTranslation('bridge')
  const navigate = useNavigate()
  const { venues } = useBridgeVenues()
  const { proof: reserve, state: reserveState } = useReserveProof(METRICS_API_BASE)

  const { hasWallet, isLocked, balance, send } = useWallet()

  // The bridge order client — `null` until a CORS-enabled public order endpoint
  // is configured (see config/bridge.ts). A null client makes the export panel
  // render an explicit "not wired yet" state.
  const client = useMemo(
    () => (BRIDGE_API_BASE ? createBridgeClient(BRIDGE_API_BASE) : null),
    [],
  )

  const exportController = useMemo<ExportController>(
    () => ({
      client,
      network: ACTIVE_BRIDGE_NETWORK,
      wallet: {
        hasWallet,
        isLocked,
        // `balance.available` is the wallet's spent-filtered spendable total.
        // The factor-1 eligibility of individual coins is verified by the bridge
        // at deposit (ADR 0003) — surfaced as a requirement in the panel.
        spendableBalance: balance ? balance.available : null,
      },
      // Reuse the wallet's real wasm-signer send path for the BTH deposit: send
      // to the reserve deposit address carrying the order memo. No new signing.
      submitDeposit: ({ depositAddress, amount, memo }) =>
        send(depositAddress, amount, memo),
      requestWallet: () => navigate('/wallet'),
    }),
    [client, hasWallet, isLocked, balance, send, navigate],
  )

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
          <BridgeView
            venues={venues}
            reserve={reserve}
            reserveState={reserveState}
            t={t}
            exportController={exportController}
          />
        </div>
      </main>
    </div>
  )
}
