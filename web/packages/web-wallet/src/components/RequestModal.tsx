import { useState } from 'react'
import { QRCodeSVG } from 'qrcode.react'
import { Button, Card, Input } from '@botho/ui'
import { formatBTH, parseBTH } from '@botho/core'
import { Download, Copy, Check, AlertCircle, X, QrCode } from 'lucide-react'
import { useWallet } from '../contexts/wallet'
import { buildPaymentRequestLink } from '../lib/payment-request'

/**
 * "Request" — generate a payment-request link (#470), the *pull* complement to
 * the *push* "Send via Link" (#460, `SendLinkModal`).
 *
 * The requester picks an (optional) amount and memo; we build a `/pay#…` URL
 * carrying their PUBLIC address + the request in the URL FRAGMENT (never the
 * query string, so it stays out of CDN/server logs). The payer opens it, the
 * wallet pre-fills a send, and they pay via the normal send path. No secret is
 * involved — unlike a claim link, this link cannot move anyone's money.
 */
export function RequestModal({ isOpen, onClose }: { isOpen: boolean; onClose: () => void }) {
  const { address } = useWallet()
  const [amountStr, setAmountStr] = useState('')
  const [memo, setMemo] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  if (!isOpen) return null

  const reset = () => {
    setAmountStr('')
    setMemo('')
    setError(null)
    setCopied(false)
  }

  const handleClose = () => {
    reset()
    onClose()
  }

  // Build the link live as the requester types. A blank/zero amount means
  // "payer chooses"; an unparseable amount surfaces a friendly error and
  // suppresses the link rather than producing a broken one.
  let amount: bigint | undefined
  let amountError: string | null = null
  if (amountStr.trim()) {
    try {
      const parsed = parseBTH(amountStr)
      if (parsed < 0n) {
        amountError = 'Amount must be positive.'
      } else if (parsed > 0n) {
        amount = parsed
      }
    } catch {
      amountError = 'Enter a valid amount.'
    }
  }

  let url: string | null = null
  if (address && !amountError) {
    const origin =
      typeof window !== 'undefined' && window.location?.origin
        ? window.location.origin
        : 'https://wallet.botho.io'
    try {
      url = buildPaymentRequestLink(origin, {
        to: address,
        amount,
        memo: memo.trim() || undefined,
      })
    } catch (err) {
      url = null
      // Should not happen (address is present), but stay friendly.
      if (!error) setError(err instanceof Error ? err.message : 'Failed to build link')
    }
  }

  const handleCopy = async () => {
    if (!url) return
    try {
      await navigator.clipboard.writeText(url)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard may be unavailable; the URL is still selectable in the field.
    }
  }

  return (
    <div className="fixed inset-0 bg-void/80 backdrop-blur-sm flex items-end sm:items-center justify-center p-0 sm:p-4 z-50">
      <Card className="w-full sm:max-w-md p-5 sm:p-6 rounded-t-2xl sm:rounded-2xl max-h-[92vh] overflow-y-auto">
        <div className="flex items-center justify-between mb-5">
          <div className="flex items-center gap-2">
            <Download className="text-pulse" size={20} />
            <h3 className="font-display text-lg font-semibold">Request Payment</h3>
          </div>
          <button onClick={handleClose} className="text-ghost hover:text-light" aria-label="Close">
            <X size={20} />
          </button>
        </div>

        {!address ? (
          <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
            <AlertCircle size={16} className="shrink-0" />
            <span>Unlock or create a wallet to request a payment.</span>
          </div>
        ) : (
          <div className="space-y-4">
            <p className="text-sm text-ghost">
              Create a link (or QR code) someone can open to pay you. Leave the amount
              blank to let the payer choose. The link carries only your public address —
              no secret, so it can never move your funds.
            </p>

            <div>
              <label className="block text-sm text-ghost mb-1.5">Amount (BTH)</label>
              <Input
                type="text"
                inputMode="decimal"
                placeholder="Any amount"
                value={amountStr}
                onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
                  setAmountStr(e.target.value)
                  setError(null)
                }}
                autoFocus
              />
              <p className="text-xs text-ghost mt-1">
                Leave blank to let the payer enter the amount.
              </p>
            </div>

            <div>
              <label className="block text-sm text-ghost mb-1.5">
                Memo / label <span className="text-ghost/70">(optional)</span>
              </label>
              <Input
                type="text"
                placeholder="What's this for?"
                value={memo}
                onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
                  setMemo(e.target.value)
                  setError(null)
                }}
              />
            </div>

            {amountError && (
              <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
                <AlertCircle size={16} className="shrink-0" />
                <span>{amountError}</span>
              </div>
            )}

            {error && (
              <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
                <AlertCircle size={16} className="shrink-0" />
                <span>{error}</span>
              </div>
            )}

            {url && (
              <>
                <div className="flex flex-col items-center gap-3">
                  <div className="rounded-xl bg-white p-3">
                    <QRCodeSVG value={url} size={176} level="M" />
                  </div>
                  <p className="text-xs text-ghost flex items-center gap-1.5">
                    <QrCode size={13} />
                    {amount ? (
                      <>Scan to pay {formatBTH(amount)} BTH</>
                    ) : (
                      <>Scan to pay</>
                    )}
                  </p>
                </div>

                <div>
                  <label className="block text-sm text-ghost mb-1.5">Payment link</label>
                  <div className="flex gap-2">
                    <input
                      readOnly
                      value={url}
                      onFocus={(e) => e.currentTarget.select()}
                      className="flex-1 min-w-0 px-3 py-2 rounded-lg bg-abyss border border-steel font-mono text-xs text-light"
                    />
                    <Button onClick={handleCopy} size="sm" variant="secondary">
                      {copied ? <Check size={16} /> : <Copy size={16} />}
                    </Button>
                  </div>
                </div>

                <Button onClick={handleClose} className="w-full justify-center">
                  Done
                </Button>
              </>
            )}
          </div>
        )}
      </Card>
    </div>
  )
}
