/**
 * Payment-request links — the *pull* complement to claimable send-links (#460).
 *
 * A claim link PUSHES money: it carries a bearer secret, and whoever holds it
 * can sweep the funds (#460, `@botho/core` `claim-link.ts`). A payment-request
 * link PULLS money instead: it carries the requester's PUBLIC address plus an
 * (optional) amount and memo. The payer opens it, the wallet pre-fills a send,
 * they confirm, and they pay via the normal send path. There is NO secret — the
 * link reveals nothing the requester wouldn't already hand out (their address).
 *
 * Link format (#470):
 *
 *   https://wallet.botho.io/pay#<base64url(JSON)>
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
 */

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
 * base64url-encode a UTF-8 string (no padding).
 *
 * `btoa`/`atob` are globals in every environment this wallet runs in (browsers,
 * jsdom, and Node 16+), so we rely on them directly. `btoa` is latin1-only, so
 * we UTF-8 encode first to preserve unicode in memos.
 */
function base64UrlEncode(input: string): string {
  const bytes = new TextEncoder().encode(input)
  let binary = ''
  for (const b of bytes) binary += String.fromCharCode(b)
  const b64 = btoa(binary)
  return b64.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

/** Decode a base64url string back into its UTF-8 source. Throws if malformed. */
function base64UrlDecode(input: string): string {
  const b64 = input.replace(/-/g, '+').replace(/_/g, '/')
  const binary = atob(b64)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  return new TextDecoder().decode(bytes)
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
 * @param origin e.g. `https://wallet.botho.io` (trailing slash tolerated)
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
