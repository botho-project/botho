import { useState } from 'react'
import { Button, Card, Input } from '@botho/ui'
import { formatBTH, parseBTH, CLAIM_LINK_MAX_AMOUNT_PICOCREDITS } from '@botho/core'
import { Link2, Copy, Check, AlertCircle, Loader2, X, ShieldAlert } from 'lucide-react'
import { useWallet, type CreatedClaimLink } from '../contexts/wallet'

/**
 * P1 — "Send via link" (claimable payment link, #460).
 *
 * Sender enters an amount; we fund a fresh ephemeral wallet (the link's bearer
 * secret) and produce a shareable URL. The bearer warning is shown explicitly:
 * anyone with the link can claim, like cash. The secret lives only in the URL
 * fragment — never sent to a server.
 */
export function SendLinkModal({ isOpen, onClose }: { isOpen: boolean; onClose: () => void }) {
  const { sendViaLink, balance } = useWallet()
  const [amountStr, setAmountStr] = useState('')
  const [isCreating, setIsCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [created, setCreated] = useState<CreatedClaimLink | null>(null)
  const [copied, setCopied] = useState(false)

  if (!isOpen) return null

  const reset = () => {
    setAmountStr('')
    setError(null)
    setCreated(null)
    setCopied(false)
    setIsCreating(false)
  }

  const handleClose = () => {
    reset()
    onClose()
  }

  let amount = 0n
  try {
    amount = amountStr ? parseBTH(amountStr) : 0n
  } catch {
    amount = 0n
  }
  const available = balance?.available ?? 0n
  // Per-link amount cap (#589): bound how much can sit claimable in chat
  // history. Over the cap, block creation and nudge toward a request link.
  const overCap = amount > CLAIM_LINK_MAX_AMOUNT_PICOCREDITS
  const canCreate = amount > 0n && !overCap && !isCreating

  const handleCreate = async () => {
    setError(null)
    setIsCreating(true)
    try {
      const result = await sendViaLink(amount)
      setCreated(result)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create link')
    } finally {
      setIsCreating(false)
    }
  }

  const handleCopy = async () => {
    if (!created) return
    try {
      await navigator.clipboard.writeText(created.url)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard may be unavailable; the URL is still selectable in the field.
    }
  }

  return (
    <div className="fixed inset-0 bg-void/80 backdrop-blur-sm flex items-end sm:items-center justify-center p-0 sm:p-4 z-50">
      <Card className="w-full sm:max-w-md p-5 sm:p-6 rounded-t-2xl sm:rounded-2xl">
        <div className="flex items-center justify-between mb-5">
          <div className="flex items-center gap-2">
            <Link2 className="text-pulse" size={20} />
            <h3 className="font-display text-lg font-semibold">Send via Link</h3>
          </div>
          <button onClick={handleClose} className="text-ghost hover:text-light" aria-label="Close">
            <X size={20} />
          </button>
        </div>

        {!created ? (
          <div className="space-y-4">
            <p className="text-sm text-ghost">
              Create a shareable link the recipient can claim to any address — even
              if they don&apos;t have a wallet yet.
            </p>

            <div>
              <label className="block text-sm text-ghost mb-1.5">Amount (BTH)</label>
              <Input
                type="text"
                inputMode="decimal"
                placeholder="0.00"
                value={amountStr}
                onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
                  setAmountStr(e.target.value)
                  setError(null)
                }}
                autoFocus
              />
              <p className="text-xs text-ghost mt-1">
                Available: {formatBTH(available)} BTH. A small network fee is added to
                cover the recipient&apos;s claim.
              </p>
              <p className="text-xs text-ghost mt-1">
                Max per link: {formatBTH(CLAIM_LINK_MAX_AMOUNT_PICOCREDITS)} BTH.
              </p>
              {overCap && (
                <div className="mt-2 flex items-start gap-2 p-2.5 rounded-lg bg-danger/10 border border-danger/20 text-danger text-xs">
                  <AlertCircle size={14} className="shrink-0 mt-0.5" />
                  <span>
                    Claim links are capped at {formatBTH(CLAIM_LINK_MAX_AMOUNT_PICOCREDITS)} BTH —
                    treat them like cash. For a larger transfer, use a <strong>request link</strong>{' '}
                    instead so the funds stay in your custody until the recipient pulls them.
                  </span>
                </div>
              )}
            </div>

            <div className="flex items-start gap-2 p-3 rounded-lg bg-amber-500/10 border border-amber-500/20">
              <ShieldAlert size={16} className="text-amber-400 mt-0.5 shrink-0" />
              <p className="text-xs text-amber-200/90">
                <strong>Anyone with this link can claim these funds — share it like
                cash.</strong> The link reveals its amount to whoever holds it. Send small
                amounts and reclaim unclaimed links from &quot;Outstanding links.&quot;
              </p>
            </div>

            {error && (
              <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
                <AlertCircle size={16} className="shrink-0" />
                <span>{error}</span>
              </div>
            )}

            <Button onClick={handleCreate} disabled={!canCreate} className="w-full justify-center">
              {isCreating ? (
                <><Loader2 size={16} className="mr-2 animate-spin" />Creating link…</>
              ) : (
                <><Link2 size={16} className="mr-2" />Create Claim Link</>
              )}
            </Button>
          </div>
        ) : (
          <div className="space-y-4">
            <div className="flex items-start gap-2 p-3 rounded-lg bg-success/10 border border-success/20">
              <Check size={16} className="text-success mt-0.5 shrink-0" />
              <p className="text-sm text-success">
                Link created and funded with {formatBTH(created.amount)} BTH. Share it with
                the recipient.
              </p>
            </div>

            <div>
              <label className="block text-sm text-ghost mb-1.5">Claim link</label>
              <div className="flex gap-2">
                <input
                  readOnly
                  value={created.url}
                  onFocus={(e) => e.currentTarget.select()}
                  className="flex-1 min-w-0 px-3 py-2 rounded-lg bg-abyss border border-steel font-mono text-xs text-light"
                />
                <Button onClick={handleCopy} size="sm" variant="secondary">
                  {copied ? <Check size={16} /> : <Copy size={16} />}
                </Button>
              </div>
            </div>

            <div className="flex items-start gap-2 p-3 rounded-lg bg-amber-500/10 border border-amber-500/20">
              <ShieldAlert size={16} className="text-amber-400 mt-0.5 shrink-0" />
              <p className="text-xs text-amber-200/90">
                Treat this link like cash. Anyone who opens it can claim the funds. If it
                goes unclaimed you can reclaim it from &quot;Outstanding links.&quot;
              </p>
            </div>

            <div className="flex gap-2">
              <Button onClick={reset} variant="secondary" className="flex-1 justify-center">
                Create another
              </Button>
              <Button onClick={handleClose} className="flex-1 justify-center">
                Done
              </Button>
            </div>
          </div>
        )}
      </Card>
    </div>
  )
}
