import { useState } from 'react'
import { Card, ModalOverlay } from '@botho/ui'
import { ShieldAlert, ShieldCheck, X, Link2 } from 'lucide-react'
import { useNetwork } from '../contexts/network'

/**
 * Trust gate for custom-RPC deep links (#587).
 *
 * When the wallet is opened with a `?rpc=<https endpoint>` link, the network
 * context does NOT switch nodes — it surfaces the parsed link as
 * `pendingRpcLink`. This modal is the explicit, unmistakable confirmation the
 * user must clear before that node is ever used:
 *
 *   "This link wants to point your wallet at <host>. You will be trusting it for
 *    balances, confirmations, and transaction relay. Only continue if you trust
 *    whoever sent this link."
 *
 * Decline is the default (keep current node); Accept is the deliberate,
 * secondary action. An UNKNOWN host (not a Botho-operated domain) gets a
 * stronger warning than a `known` host. HTTPS validation already happened in
 * `parseRpcDeepLink`; this gate is the second, necessary half of the defence.
 */
export function CustomRpcTrustGate() {
  const { pendingRpcLink, acceptPendingRpcLink, declinePendingRpcLink } = useNetwork()
  const [busy, setBusy] = useState(false)

  if (!pendingRpcLink) return null

  const { host, trust } = pendingRpcLink
  const known = trust === 'known'

  const handleAccept = async () => {
    setBusy(true)
    try {
      await acceptPendingRpcLink()
    } finally {
      setBusy(false)
    }
  }

  return (
    // Shared dismissal policy (#655): backdrop click / Escape map to DECLINE —
    // the safe default (keep the current node). Accepting a custom node must
    // always be the explicit, deliberate action. Suppressed while connecting.
    <ModalOverlay
      onDismiss={declinePendingRpcLink}
      dismissable={!busy}
      ariaLabelledBy="rpc-trust-title"
      className="bg-void/80 backdrop-blur-sm flex items-end sm:items-center justify-center p-0 sm:p-4"
    >
      <Card className="w-full sm:max-w-md p-5 sm:p-6 rounded-t-2xl sm:rounded-2xl max-h-[92vh] overflow-y-auto">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-2">
            {known ? (
              <ShieldCheck className="text-pulse" size={20} />
            ) : (
              <ShieldAlert className="text-danger" size={20} />
            )}
            <h3 id="rpc-trust-title" className="font-display text-lg font-semibold">
              Trust this node?
            </h3>
          </div>
          <button
            onClick={declinePendingRpcLink}
            className="text-ghost hover:text-light"
            aria-label="Decline"
            disabled={busy}
          >
            <X size={20} />
          </button>
        </div>

        <p className="text-sm text-light">
          This link wants to point your wallet at{' '}
          <span className="font-semibold break-all text-pulse">{host}</span>. You will be trusting
          it for balances, confirmations, and transaction relay. Only continue if you trust whoever
          sent this link.
        </p>

        {known ? (
          <div className="mt-3 flex items-start gap-2 rounded-lg border border-pulse/30 bg-pulse/10 p-3">
            <ShieldCheck size={16} className="text-pulse mt-0.5 shrink-0" />
            <p className="text-xs text-ghost">
              This looks like a Botho-operated host. That is a hint only — a link can still be
              forged, so only continue if you expected it.
            </p>
          </div>
        ) : (
          <div className="mt-3 flex items-start gap-2 rounded-lg border border-danger/30 bg-danger/10 p-3">
            <ShieldAlert size={16} className="text-danger mt-0.5 shrink-0" />
            <p className="text-xs text-ghost">
              <span className="font-semibold text-light">Unknown host.</span> This is not a
              recognised Botho node. A hostile node can lie about your balance and confirmations
              (fake &quot;payment received&quot;), withhold your transactions, and harvest your
              addresses and IP. Decline unless you personally trust this operator.
            </p>
          </div>
        )}

        <div className="mt-5 flex flex-col-reverse sm:flex-row gap-2 sm:justify-end">
          {/* Decline is the default / primary action: keep the current node. */}
          <button
            type="button"
            onClick={declinePendingRpcLink}
            disabled={busy}
            className="rounded-lg bg-steel px-4 py-2.5 text-sm font-medium text-light hover:bg-steel/80 transition-colors disabled:opacity-50"
          >
            Decline (keep current node)
          </button>
          <button
            type="button"
            onClick={handleAccept}
            disabled={busy}
            className={`rounded-lg px-4 py-2.5 text-sm font-medium transition-colors disabled:opacity-50 ${
              known
                ? 'bg-pulse/20 text-pulse hover:bg-pulse/30'
                : 'bg-danger/20 text-danger hover:bg-danger/30'
            }`}
          >
            {busy ? 'Connecting…' : 'Trust & connect'}
          </button>
        </div>
      </Card>
    </ModalOverlay>
  )
}

/**
 * Persistent banner shown while the wallet is connected to a custom node that
 * was accepted from a deep link (#587). Keeps the user aware they are off the
 * default seeds and offers a one-tap revert back to the default ingress.
 */
export function CustomNodeBanner() {
  const { customNodeFromLink, revertCustomNode } = useNetwork()

  if (!customNodeFromLink) return null

  return (
    <div
      role="status"
      className="flex items-start gap-3 rounded-lg border border-pulse/30 bg-pulse/10 p-3 sm:p-4"
    >
      <Link2 size={18} className="text-pulse mt-0.5 shrink-0" />
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium text-light">
          Connected to custom node{' '}
          <span className="break-all text-pulse">{customNodeFromLink}</span>{' '}
          <span className="text-ghost font-normal">(from a link)</span>
        </p>
        <p className="text-xs text-ghost mt-1">
          You are off the default Botho nodes. This node serves all your balances, confirmations,
          and transaction relay. Switch back if you did not mean to connect here.
        </p>
      </div>
      <button
        type="button"
        onClick={revertCustomNode}
        className="rounded-md bg-pulse/20 px-3 py-1.5 text-xs font-medium text-light hover:bg-pulse/30 transition-colors whitespace-nowrap shrink-0"
      >
        Switch back
      </button>
    </div>
  )
}
