import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, X } from 'lucide-react'
import { useNetwork } from '../contexts/network'
import { useWallet } from '../contexts/wallet'
import {
  advanceDebounce,
  initialDebounceState,
  isActiveNodeOfflineRaw,
  type OfflineDebounceState,
} from '../lib/active-node-offline'

/**
 * Debounced "is the active ingress node offline?" hook (#492).
 *
 * Reuses the EXISTING per-node health polling in NetworkContext (no second poll
 * loop) plus the wallet's `isConnected` to compute a raw verdict, then debounces
 * it so a single transient blip does not flap the banner. The health poll
 * updates `nodeHealth` on its cadence; each such update (and each connection /
 * selection change) is one debounce "tick".
 *
 * Returns the debounced `offline` flag. Switching ingress resets the debounce so
 * the banner does not carry over a stale verdict to a freshly selected node.
 */
export function useActiveNodeOffline(): boolean {
  const { ingressId, nodeHealth } = useNetwork()
  const { isConnected } = useWallet()

  const stateRef = useRef<OfflineDebounceState>(initialDebounceState())
  const [offline, setOffline] = useState(false)

  // Reset the debounce when the selected ingress changes: a new node starts with
  // a clean slate (no inherited "offline" streak from the previous selection).
  useEffect(() => {
    stateRef.current = initialDebounceState()
    setOffline(false)
  }, [ingressId])

  useEffect(() => {
    const rawOffline = isActiveNodeOfflineRaw({ ingressId, nodeHealth, isConnected })
    stateRef.current = advanceDebounce(stateRef.current, rawOffline)
    setOffline(stateRef.current.shown)
  }, [ingressId, nodeHealth, isConnected])

  return offline
}

/**
 * Dismissible banner shown at the top of the wallet view when the user's
 * currently selected ingress node is offline/unreachable (#492).
 *
 * Offers a one-click "Switch node" action that opens the NetworkSelector (the
 * node picker lives in the header). Dismiss hides the banner until the node
 * recovers and goes offline again — recovery clears the dismissal so a later
 * outage re-surfaces it.
 */
export function OfflineBanner() {
  const offline = useActiveNodeOffline()
  const [dismissed, setDismissed] = useState(false)

  // Once the node recovers (banner condition clears), drop the dismissal so a
  // FUTURE outage re-surfaces the banner instead of staying silenced forever.
  useEffect(() => {
    if (!offline) setDismissed(false)
  }, [offline])

  if (!offline || dismissed) return null

  const handleSwitch = () => {
    // The node picker (NetworkSelector) lives in the wallet header. Nudge the
    // user to it; dispatch an event the selector can choose to react to (open),
    // and scroll the header into view as a no-JS-coupling fallback.
    window.dispatchEvent(new CustomEvent('open-network-selector'))
    if (typeof document !== 'undefined') {
      document.querySelector('header')?.scrollIntoView({ behavior: 'smooth', block: 'start' })
    }
  }

  return (
    <div
      role="alert"
      className="flex items-start gap-3 rounded-lg border border-danger/30 bg-danger/10 p-3 sm:p-4"
    >
      <AlertTriangle size={18} className="text-danger mt-0.5 shrink-0" />
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium text-light">Your node is unreachable</p>
        <p className="text-xs text-ghost mt-1">
          The node this wallet is connected to isn&apos;t responding. Balances and history
          may be out of date. Switch to a healthy node to keep using your wallet.
        </p>
      </div>
      <div className="flex items-center gap-2 shrink-0">
        <button
          type="button"
          onClick={handleSwitch}
          className="rounded-md bg-danger/20 px-3 py-1.5 text-xs font-medium text-light hover:bg-danger/30 transition-colors whitespace-nowrap"
        >
          Switch node
        </button>
        <button
          type="button"
          onClick={() => setDismissed(true)}
          aria-label="Dismiss"
          className="text-ghost hover:text-light transition-colors"
        >
          <X size={16} />
        </button>
      </div>
    </div>
  )
}
