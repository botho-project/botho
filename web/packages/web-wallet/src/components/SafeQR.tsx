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
 * Version-40 byte-mode capacity of a single QR symbol, per error-correction
 * level. The fit decision MUST use the ceiling for the *same* EC level the
 * symbol is rendered at — a higher-EC level packs fewer data bytes, so using
 * the level-L ceiling (2953) while rendering at the default level M (2331)
 * would judge a ~2331..2953-byte payload as "fits" and then hand it to
 * `qrcode.react`, which throws during render (#979). Keying the cap off the
 * rendered `level` makes the two impossible to drift apart.
 *
 * Sources: ISO/IEC 18004 version-40 byte-mode data capacities.
 */
export const QR_V40_BYTE_CAP: Record<'L' | 'M' | 'Q' | 'H', number> = {
  L: 2953,
  M: 2331,
  Q: 1663,
  H: 1273,
}

/**
 * Maximum payload (characters ≈ bytes for our base58/ASCII payloads) that fits a
 * single QR symbol at the given EC `level`. This is the single source of truth
 * for the fit decision, so the cap can never diverge from the level the symbol
 * is actually rendered at.
 */
export function singleQrByteCap(level: 'L' | 'M' | 'Q' | 'H'): number {
  return QR_V40_BYTE_CAP[level]
}

/**
 * Level-L ceiling, retained for reference/back-compat. Prefer
 * `singleQrByteCap(level)` — this constant is only correct when rendering at
 * EC level L, and using it directly is exactly the drift that #979 fixed.
 *
 * @deprecated Use {@link singleQrByteCap} keyed on the rendered `level`.
 */
export const SINGLE_QR_BYTE_CAP = QR_V40_BYTE_CAP.L

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
  // Cap is derived from the SAME level the symbol renders at, so a payload that
  // passes this guard is always encodable at that level (#979).
  const cap = singleQrByteCap(level)
  const fits = value.length > 0 && value.length <= cap

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
