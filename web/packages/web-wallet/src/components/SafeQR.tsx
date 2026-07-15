import { QRCodeSVG } from 'qrcode.react'
import { QrCode } from 'lucide-react'

/**
 * Capacity-aware QR wrapper (#965).
 *
 * A single QR symbol tops out around 2953 bytes (byte mode, version 40, EC level
 * L). An address-format-v2 (`botho://2/…`) address is ~4.4 KB because it now
 * carries the ML-KEM-768 (1184 B) + ML-DSA-65 (1952 B) public keys, so it — and
 * any `/pay#…` link that embeds it — cannot fit. Handing such a payload to
 * `qrcode.react` throws *during render*, which would blank the whole modal.
 *
 * Rather than a multi-symbol "structured append" QR (poorly supported by phone
 * scanners) or a hash-pointer QR (needs a resolver service we don't run), we
 * take the least-surprising option: render a real QR only when the payload fits
 * a single symbol, and otherwise show a clear "copy instead" panel. The address
 * text + copy button live alongside this component in every caller, so the
 * receive/share flow still works — you copy the address instead of scanning it.
 *
 * When post-quantum addresses get a short on-chain alias (a future enhancement),
 * that short alias can be fed here and will QR normally.
 */

/**
 * Maximum payload (characters ≈ bytes for our base58/ASCII payloads) that fits a
 * single QR symbol. Uses the EC-level-L byte-mode ceiling; we intentionally pick
 * the largest so short payloads always QR, and only genuinely oversized ones
 * (full v2 addresses / links) fall back.
 */
export const SINGLE_QR_BYTE_CAP = 2953

export function SafeQR({
  value,
  size = 200,
  level = 'M',
  ariaLabel,
  className,
}: {
  value: string
  size?: number
  level?: 'L' | 'M' | 'Q' | 'H'
  ariaLabel?: string
  className?: string
}) {
  const fits = value.length > 0 && value.length <= SINGLE_QR_BYTE_CAP

  if (fits) {
    return (
      <div className={className ?? 'rounded-xl bg-white p-3'}>
        <QRCodeSVG value={value} size={size} level={level} aria-label={ariaLabel} />
      </div>
    )
  }

  // Oversized payload (e.g. a post-quantum v2 address): no single-symbol QR is
  // possible. Show a graceful, same-footprint placeholder that steers the user
  // to copy the address instead of scanning it.
  return (
    <div
      role="note"
      aria-label="Address is too long for a QR code — copy it instead"
      className="flex flex-col items-center justify-center gap-2 rounded-xl border border-steel bg-abyss p-4 text-center text-ghost"
      style={{ width: size, height: size }}
    >
      <QrCode size={28} className="opacity-60" />
      <p className="text-xs leading-snug">
        This address is too long for a QR code. Copy it and share the text instead.
      </p>
    </div>
  )
}
