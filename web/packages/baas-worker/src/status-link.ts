/**
 * Magic-link status tokens for the Botho-as-a-Service `/status` lookup
 * (P6.3 of #458, §4).
 *
 * Identity for the MVP is the **Stripe customer** — there is no password system
 * (#458 §4). A returning user looks up their rig via a *signed, expiring* token
 * that binds to exactly one Stripe customer id. The `/status` endpoint verifies
 * the token, extracts the customer id, and looks the rig up keyed on that id, so
 * a user can only ever see their own rig (authz — never trust a customer id
 * straight from the client).
 *
 * Token format (URL-safe, single opaque string):
 *
 *     <customerId>.<expUnixSeconds>.<hmacHexOver("<customerId>.<exp>")>
 *
 * The HMAC is SHA-256 keyed on `STATUS_LINK_SECRET` (a Worker secret, never the
 * repo). This is the same crypto primitive the Stripe webhook uses to verify
 * signatures (`hmacSha256Hex` / `timingSafeEqualHex`), reused here so there is a
 * single, audited HMAC implementation.
 *
 * The token is a bearer credential: anyone holding it can read that customer's
 * rig URL/state. That matches the magic-link model in #458 §4 (the link is the
 * credential, mailed to the customer's address). It deliberately grants NO
 * write/provision capability — `/status` is read-only, and provisioning is only
 * ever triggered by a signature-verified Stripe webhook (#458 §5).
 */

import { hmacSha256Hex, timingSafeEqualHex } from './webhook'

/** Default lifetime of a freshly minted status token (seconds): 7 days. */
export const DEFAULT_STATUS_TOKEN_TTL_SECONDS = 7 * 24 * 60 * 60

/** A customer id must look like a Stripe customer (`cus_...`) and be dot-free. */
function isPlausibleCustomerId(id: string): boolean {
  // Stripe customer ids are `cus_` + alphanumerics. Reject anything containing a
  // '.' (our field separator) or whitespace so the token can't be confused.
  return /^cus_[A-Za-z0-9]+$/.test(id)
}

/**
 * Mint a signed status token for a Stripe customer.
 *
 * @param customerId Stripe customer id (`cus_...`).
 * @param secret     `STATUS_LINK_SECRET` Worker secret.
 * @param opts       `ttlSeconds` overrides the default lifetime; `nowSeconds`
 *                   pins the clock for deterministic tests.
 */
export async function mintStatusToken(
  customerId: string,
  secret: string,
  opts: { ttlSeconds?: number; nowSeconds?: number } = {},
): Promise<string> {
  if (!secret) throw new Error('status link secret not configured')
  if (!isPlausibleCustomerId(customerId)) {
    throw new Error(`refusing to mint token for implausible customer id: ${customerId}`)
  }
  const now = opts.nowSeconds ?? Math.floor(Date.now() / 1000)
  const ttl = opts.ttlSeconds ?? DEFAULT_STATUS_TOKEN_TTL_SECONDS
  const exp = now + ttl
  const payload = `${customerId}.${exp}`
  const sig = await hmacSha256Hex(secret, payload)
  return `${payload}.${sig}`
}

/** Outcome of verifying a status token. */
export type StatusTokenResult =
  | { ok: true; customerId: string; exp: number }
  | { ok: false; reason: string }

/**
 * Verify a status token and extract its Stripe customer id.
 *
 * Rejects (without revealing which check failed beyond a generic reason):
 *   - missing / malformed tokens,
 *   - expired tokens,
 *   - tokens whose HMAC does not match (tampered customer id / forged signature),
 *   - implausible customer ids.
 *
 * The HMAC is recomputed over `"<customerId>.<exp>"` and compared in constant
 * time, so a tampered `customerId` (e.g. swapping in another user's id) fails.
 */
export async function verifyStatusToken(
  token: string | null | undefined,
  secret: string,
  opts: { nowSeconds?: number } = {},
): Promise<StatusTokenResult> {
  if (!secret) return { ok: false, reason: 'status link secret not configured' }
  if (!token) return { ok: false, reason: 'missing token' }

  // Split from the RIGHT so a customer id could in theory contain a dot; in
  // practice we also validate the id shape below. Expect exactly 3 fields.
  const parts = token.split('.')
  if (parts.length !== 3) return { ok: false, reason: 'malformed token' }
  const [customerId, expStr, sig] = parts

  if (!isPlausibleCustomerId(customerId)) {
    return { ok: false, reason: 'malformed token' }
  }
  const exp = Number(expStr)
  if (!Number.isInteger(exp) || exp <= 0) {
    return { ok: false, reason: 'malformed token' }
  }

  // Verify the signature BEFORE trusting any field. Constant-time compare.
  const expected = await hmacSha256Hex(secret, `${customerId}.${exp}`)
  if (!timingSafeEqualHex(expected, sig)) {
    return { ok: false, reason: 'signature mismatch' }
  }

  // Only after the signature passes do we honour the (now-trusted) expiry.
  const now = opts.nowSeconds ?? Math.floor(Date.now() / 1000)
  if (now >= exp) {
    return { ok: false, reason: 'token expired' }
  }

  return { ok: true, customerId, exp }
}
