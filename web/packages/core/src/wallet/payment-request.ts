/**
 * Payment-request links — the *pull* complement to claimable send-links (#460).
 *
 * A claim link PUSHES money: it carries a bearer secret, and whoever holds it
 * can sweep the funds (#460, `claim-link.ts`). A payment-request link PULLS
 * money instead: it carries the requester's PUBLIC address plus an (optional)
 * amount and memo. The payer opens it, the wallet pre-fills a send, they
 * confirm, and they pay via the normal send path. There is NO secret — the link
 * reveals nothing the requester wouldn't already hand out (their address).
 *
 * Link format (#470):
 *
 *   https://botho.io/pay#<base64url(JSON)>
 *
 * where the JSON is `{ to, amount?, memo? }`:
 *   - `to`     the requester's public address (tbotho://… / botho://…).
 *   - `amount` OPTIONAL requested amount in PICOCREDITS, serialized as a decimal
 *              string (bigint-safe). Omitted/blank means "payer chooses".
 *   - `memo`   OPTIONAL human label / note for the request.
 *
 * Why the FRAGMENT (after `#`) and not the query string: browsers never transmit
 * the fragment to a server, so the requester's address stays out of
 * Cloudflare/CDN/server access logs — exactly as claim links carry their payload.
 * The pay page strips the fragment (`history.replaceState`) after reading it.
 *
 * Encoding is plain base64url'd JSON (not the claim link's `v1.<base58>` form):
 * a payment request has no fixed-width secret, just a small structured record,
 * so base64'd JSON keeps it readable, versionable, and trivially round-trippable.
 *
 * SES SAFETY (#1108): this module lives in `@botho/core` so the Botho MetaMask
 * Snap can import it. The Snap bundle runs inside MetaMask's SES (hardened)
 * executor, where browser globals like `btoa` / `atob` / `TextEncoder` /
 * `TextDecoder` are NOT reliably endowed (the same casualty `snap/src/format.ts`
 * documents for `Intl`). So the base64url step uses `@scure/base`'s
 * `base64urlnopad` (already a `@botho/core` dependency — see `claim-link.ts`'s
 * `base58`) and the string↔bytes step uses a tiny pure-JS UTF-8 codec instead of
 * `TextEncoder` / `TextDecoder`. The `base64urlnopad` alphabet output is
 * byte-for-byte identical to the previous `btoa`-based encoder, so `/pay#…` links
 * minted before this change keep round-tripping (asserted in the test).
 */

import { base64urlnopad } from '@scure/base'

/** A payment request: who to pay, optionally how much, optionally why. */
export interface PaymentRequest {
  /** The requester's PUBLIC address (tbotho://… / botho://…). */
  to: string
  /**
   * OPTIONAL requested amount in picocredits. When omitted the payer enters the
   * amount themselves on the pay page.
   */
  amount?: bigint
  /** OPTIONAL human-readable label / note for the request. */
  memo?: string
}

/** The shape we actually serialize to JSON (bigint -> decimal string). */
interface PaymentRequestWire {
  to: string
  amount?: string
  memo?: string
}

/**
 * Encode a JS string as UTF-8 bytes WITHOUT `TextEncoder` (not reliably endowed
 * under the Snaps SES executor — see the module header). Handles the full BMP
 * plus astral code points via surrogate-pair recombination, so unicode memos
 * (emoji, CJK) survive the round-trip.
 */
function utf8Encode(input: string): Uint8Array {
  const out: number[] = []
  for (let i = 0; i < input.length; i++) {
    let code = input.charCodeAt(i)
    // Recombine a high/low surrogate pair into a single astral code point.
    if (code >= 0xd800 && code <= 0xdbff && i + 1 < input.length) {
      const next = input.charCodeAt(i + 1)
      if (next >= 0xdc00 && next <= 0xdfff) {
        code = 0x10000 + ((code - 0xd800) << 10) + (next - 0xdc00)
        i++
      }
    }
    if (code < 0x80) {
      out.push(code)
    } else if (code < 0x800) {
      out.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f))
    } else if (code < 0x10000) {
      out.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f))
    } else {
      out.push(
        0xf0 | (code >> 18),
        0x80 | ((code >> 12) & 0x3f),
        0x80 | ((code >> 6) & 0x3f),
        0x80 | (code & 0x3f),
      )
    }
  }
  return Uint8Array.from(out)
}

/**
 * Decode UTF-8 bytes back to a JS string WITHOUT `TextDecoder` (see the module
 * header). Inverse of {@link utf8Encode}. Throws on a truncated multi-byte
 * sequence so malformed payloads fail loudly (the caller maps that to a
 * "not a valid link" error).
 */
function utf8Decode(bytes: Uint8Array): string {
  let out = ''
  let i = 0
  while (i < bytes.length) {
    const b0 = bytes[i++]
    let code: number
    if (b0 < 0x80) {
      code = b0
    } else if ((b0 & 0xe0) === 0xc0) {
      if (i >= bytes.length) throw new Error('truncated UTF-8 sequence')
      code = ((b0 & 0x1f) << 6) | (bytes[i++] & 0x3f)
    } else if ((b0 & 0xf0) === 0xe0) {
      if (i + 1 >= bytes.length) throw new Error('truncated UTF-8 sequence')
      code = ((b0 & 0x0f) << 12) | ((bytes[i++] & 0x3f) << 6) | (bytes[i++] & 0x3f)
    } else {
      if (i + 2 >= bytes.length) throw new Error('truncated UTF-8 sequence')
      code =
        ((b0 & 0x07) << 18) |
        ((bytes[i++] & 0x3f) << 12) |
        ((bytes[i++] & 0x3f) << 6) |
        (bytes[i++] & 0x3f)
    }
    if (code > 0xffff) {
      code -= 0x10000
      out += String.fromCharCode(0xd800 + (code >> 10), 0xdc00 + (code & 0x3ff))
    } else {
      out += String.fromCharCode(code)
    }
  }
  return out
}

/**
 * base64url-encode a UTF-8 string (no padding), SES-safely.
 *
 * Uses a pure-JS UTF-8 codec + `@scure/base` `base64urlnopad` so it never
 * touches `btoa` / `TextEncoder` (not reliably endowed under the Snaps SES
 * executor). The output is byte-for-byte identical to the previous `btoa`-based
 * encoder (verified in the test), so existing `/pay#…` links keep round-tripping.
 */
function base64UrlEncode(input: string): string {
  return base64urlnopad.encode(utf8Encode(input))
}

/** Decode a base64url string back into its UTF-8 source. Throws if malformed. */
function base64UrlDecode(input: string): string {
  return utf8Decode(base64urlnopad.decode(input))
}

/**
 * Encode a payment request into a URL fragment payload.
 *
 * Returns the fragment WITHOUT the leading `#`, i.e. the base64url'd JSON.
 *
 * @throws if `to` is missing/blank, or `amount` is negative.
 */
export function buildPaymentRequestFragment(req: PaymentRequest): string {
  const to = (req.to ?? '').trim()
  if (!to) throw new Error('A payment request needs a recipient address')

  const wire: PaymentRequestWire = { to }
  if (req.amount !== undefined) {
    if (req.amount < 0n) throw new Error('amount must be non-negative')
    // A zero amount is treated the same as "no amount" (payer chooses).
    if (req.amount > 0n) wire.amount = req.amount.toString()
  }
  const memo = req.memo?.trim()
  if (memo) wire.memo = memo

  return base64UrlEncode(JSON.stringify(wire))
}

/**
 * Build the full shareable payment-request URL.
 *
 * @param origin e.g. `https://botho.io` (trailing slash tolerated)
 * @param req    the payment request
 */
export function buildPaymentRequestLink(origin: string, req: PaymentRequest): string {
  const base = origin.replace(/\/$/, '')
  return `${base}/pay#${buildPaymentRequestFragment(req)}`
}

/**
 * Parse a payment-request fragment back into a {@link PaymentRequest}.
 *
 * Accepts the fragment with or without a leading `#`, and accepts a full URL
 * (only the part after `#` is used). Throws on a malformed/unsupported payload.
 *
 * NOTE: this validates only that `to` is a non-empty string — it does NOT
 * validate the address FORMAT. Callers that act on the address (e.g. the Snap's
 * prefill-send path) must additionally validate it via `parseAddress` /
 * `isValidAddress` before use.
 */
export function parsePaymentRequestFragment(fragment: string): PaymentRequest {
  let raw = (fragment ?? '').trim()
  // Allow passing a whole URL — take only the fragment portion.
  const hashIdx = raw.indexOf('#')
  if (hashIdx >= 0) raw = raw.slice(hashIdx + 1)
  if (raw.startsWith('#')) raw = raw.slice(1)
  if (!raw) throw new Error('Empty payment-request link')

  let json: string
  try {
    json = base64UrlDecode(raw)
  } catch {
    throw new Error('This payment-request link is not valid.')
  }

  let parsed: unknown
  try {
    parsed = JSON.parse(json)
  } catch {
    throw new Error('This payment-request link is not valid.')
  }

  if (typeof parsed !== 'object' || parsed === null) {
    throw new Error('This payment-request link is not valid.')
  }
  const wire = parsed as Record<string, unknown>

  const to = typeof wire.to === 'string' ? wire.to.trim() : ''
  if (!to) throw new Error('This payment-request link is missing a recipient address.')

  const req: PaymentRequest = { to }

  if (wire.amount !== undefined && wire.amount !== null && wire.amount !== '') {
    if (typeof wire.amount !== 'string' && typeof wire.amount !== 'number') {
      throw new Error('This payment-request link has an invalid amount.')
    }
    let amount: bigint
    try {
      amount = BigInt(wire.amount)
    } catch {
      throw new Error('This payment-request link has an invalid amount.')
    }
    if (amount < 0n) throw new Error('This payment-request link has an invalid amount.')
    if (amount > 0n) req.amount = amount
  }

  if (typeof wire.memo === 'string') {
    const memo = wire.memo.trim()
    if (memo) req.memo = memo
  }

  return req
}
