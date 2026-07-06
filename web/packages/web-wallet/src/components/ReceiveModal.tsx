import { useState } from 'react'
import { QRCodeSVG } from 'qrcode.react'
import { Button, Card, ModalOverlay } from '@botho/ui'
import { QrCode, Copy, Check, AlertCircle, X, Link2 } from 'lucide-react'
import { useWallet } from '../contexts/wallet'

/**
 * "Receive" — show the wallet's OWN public address as a scannable QR so someone
 * can pay you in person (#477). The QR encodes the RAW address only, so any
 * wallet that can scan an address will work — no Botho-specific link parsing
 * required.
 *
 * This is intentionally distinct from "Request Payment" (#470, `RequestModal`),
 * which builds a `/pay#…` *link* carrying an optional amount/memo and only opens
 * in a Botho wallet. Here there is no amount and no link — just "this is my
 * address, scan it to pay me". A button is offered to jump to the richer
 * request-a-link flow when an amount/memo is wanted.
 */
export function ReceiveModal({
  isOpen,
  onClose,
  onRequestLink,
}: {
  isOpen: boolean
  onClose: () => void
  /** Optional: switch over to the richer "Request a link" flow (#470). */
  onRequestLink?: () => void
}) {
  const { address } = useWallet()
  const [copied, setCopied] = useState(false)

  if (!isOpen) return null

  const handleClose = () => {
    setCopied(false)
    onClose()
  }

  const handleCopy = async () => {
    if (!address) return
    try {
      await navigator.clipboard.writeText(address)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      // Clipboard may be unavailable; the address is still selectable in the field.
    }
  }

  return (
    // Shared dismissal policy (#655): backdrop click / Escape dismiss through
    // handleClose so the copied state resets, same as the X / Done buttons.
    <ModalOverlay
      onDismiss={handleClose}
      className="bg-void/80 backdrop-blur-sm flex items-end sm:items-center justify-center p-0 sm:p-4"
    >
      <Card className="w-full sm:max-w-md p-5 sm:p-6 rounded-t-2xl sm:rounded-2xl max-h-[92vh] overflow-y-auto">
        <div className="flex items-center justify-between mb-5">
          <div className="flex items-center gap-2">
            <QrCode className="text-pulse" size={20} />
            <h3 className="font-display text-lg font-semibold">Receive</h3>
          </div>
          <button onClick={handleClose} className="text-ghost hover:text-light" aria-label="Close">
            <X size={20} />
          </button>
        </div>

        {!address ? (
          <div className="flex items-center gap-2 p-3 rounded-lg bg-danger/10 border border-danger/20 text-danger text-sm">
            <AlertCircle size={16} className="shrink-0" />
            <span>Unlock or create a wallet to receive a payment.</span>
          </div>
        ) : (
          <div className="space-y-4">
            <p className="text-sm text-ghost">
              Let someone scan this to pay you in person. The QR is your public address — no amount,
              no link, no secret. Anyone can send you any amount.
            </p>

            <div className="flex flex-col items-center gap-3">
              <div className="rounded-xl bg-white p-3">
                <QRCodeSVG value={address} size={200} level="M" aria-label="Receiving address QR code" />
              </div>
              <p className="text-xs text-ghost flex items-center gap-1.5">
                <QrCode size={13} />
                Scan to pay me
              </p>
            </div>

            <div>
              <label className="block text-sm text-ghost mb-1.5">Your address</label>
              <div className="flex gap-2">
                <input
                  readOnly
                  value={address}
                  onFocus={(e) => e.currentTarget.select()}
                  className="flex-1 min-w-0 px-3 py-2 rounded-lg bg-abyss border border-steel font-mono text-xs text-light"
                />
                <Button onClick={handleCopy} size="sm" variant="secondary" aria-label="Copy address">
                  {copied ? <Check size={16} /> : <Copy size={16} />}
                </Button>
              </div>
            </div>

            {onRequestLink && (
              <button
                onClick={() => {
                  handleClose()
                  onRequestLink()
                }}
                className="flex w-full items-center justify-center gap-1.5 text-sm text-ghost hover:text-light transition-colors"
              >
                <Link2 size={14} />
                Request a specific amount instead
              </button>
            )}

            <Button onClick={handleClose} className="w-full justify-center">
              Done
            </Button>
          </div>
        )}
      </Card>
    </ModalOverlay>
  )
}
