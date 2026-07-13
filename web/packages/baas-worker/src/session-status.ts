/**
 * `session_id` → status-token exchange for the post-checkout success page
 * (P6.3 of #458, §4; issue #805).
 *
 * After a Stripe Checkout completes, the browser lands on
 * `/node/success?session_id=cs_...`. That `session_id` is the only credential the
 * payer's browser holds, so we exchange it — server-side — for the same signed,
 * expiring magic-link token the `/status` page already uses:
 *
 *     session_id
 *       → retrieve the Checkout Session from Stripe (STRIPE_SECRET_KEY)
 *       → confirm payment_status === 'paid'
 *       → read the `customer` id off the session
 *       → mintStatusToken(customerId)         (status-link.ts, keyed on cus_...)
 *       → build the `/node/status?token=…` URL the page links to
 *
 * Provisioning is asynchronous: the D1 row is written by the signature-verified
 * webhook, which can lag the success redirect by seconds. So the exchange has a
 * distinct "pending" outcome (session is paid, but the node row is not in D1 yet)
 * that the frontend polls on, versus a terminal rejection for an unknown / unpaid
 * / malformed session.
 *
 * Security: the token is minted from the *verified* Stripe customer id (never a
 * client-supplied one), and this module never reveals which check failed — an
 * unknown, unpaid, or malformed session and an unretrievable one all collapse to
 * the same generic rejection, mirroring the `/status` 401 no-leak precedent.
 *
 * Pure / injectable: `fetchImpl` defaults to `boundFetch` (workerd requires the
 * bound global — a bare `fetch` throws `Illegal invocation` in production, see
 * `bound-fetch.ts`) and the store is injected, so NO real Stripe / D1 call
 * happens in a test path.
 */

import { boundFetch } from './bound-fetch'
import type { NodeStore } from './node-store'
import { mintStatusToken } from './status-link'

/** A Stripe Checkout Session id looks like `cs_...` (test: `cs_test_...`). */
function isPlausibleSessionId(id: string): boolean {
  // Reject anything not shaped like a Stripe checkout session id so we never
  // interpolate junk into the Stripe request URL.
  return /^cs_[A-Za-z0-9_]+$/.test(id)
}

/** Error thrown when Stripe rejects (or errors on) the session retrieve. */
export class StripeSessionError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message)
    this.name = 'StripeSessionError'
  }
}

/** The fields we read off a retrieved Checkout Session. */
export interface RetrievedSession {
  /** `paid` once the payment has settled; `unpaid` / `no_payment_required` otherwise. */
  paymentStatus: string
  /** Stripe customer id (`cus_...`) created for the subscription, if present. */
  customerId?: string
}

/**
 * Retrieve a Stripe Checkout Session by id.
 *
 * Follows the established Stripe call shape (`stripe-subscriptions.ts` /
 * `checkout.ts`): `Authorization: Bearer`, pinned `Stripe-Version: 2024-06-20`,
 * and a `fetchImpl` that defaults to `boundFetch`.
 *
 *   - 200 → `{ paymentStatus, customerId }`
 *   - 404 → THROW `StripeSessionError(…, 404)` (no such session — treated as a
 *           terminal generic rejection by the caller)
 *   - any other non-2xx → THROW `StripeSessionError` (transient / auth)
 */
export async function retrieveCheckoutSession(
  sessionId: string,
  stripeSecretKey: string,
  fetchImpl: typeof fetch = boundFetch,
): Promise<RetrievedSession> {
  const resp = await fetchImpl(
    `https://api.stripe.com/v1/checkout/sessions/${encodeURIComponent(sessionId)}`,
    {
      method: 'GET',
      headers: {
        Authorization: `Bearer ${stripeSecretKey}`,
        'Stripe-Version': '2024-06-20',
      },
    },
  )

  const json = (await resp.json().catch(() => ({}))) as {
    payment_status?: string
    customer?: string | { id?: string }
    error?: { message?: string }
  }

  if (!resp.ok) {
    throw new StripeSessionError(
      json.error?.message ?? `Stripe returned HTTP ${resp.status}`,
      resp.status,
    )
  }

  const customer = json.customer
  const customerId =
    typeof customer === 'string'
      ? customer || undefined
      : customer && typeof customer === 'object' && typeof customer.id === 'string'
        ? customer.id || undefined
        : undefined

  return { paymentStatus: json.payment_status ?? 'unpaid', customerId }
}

/** Outcome of exchanging a `session_id` for a status token / URL. */
export type SessionExchange =
  /** Paid session + a provisioned node row → the status link is ready. */
  | { kind: 'ready'; statusUrl: string; token: string }
  /**
   * Paid session, but the node row is not in D1 yet (provisioning lag). The
   * frontend should keep polling — this is NOT an error.
   */
  | { kind: 'pending' }
  /**
   * Unknown / unpaid / malformed session, or the customer has no node and never
   * will from this session. Terminal — the frontend must stop polling. The
   * generic `reason` is for server logs only; it is never surfaced to the client.
   */
  | { kind: 'rejected'; reason: string }

/**
 * Build the `/node/status?token=…` URL a returning user opens. `walletBaseUrl`
 * is the site origin (e.g. `https://botho.io`); the token rides as the `token`
 * query param the status page already reads.
 */
export function buildStatusUrl(walletBaseUrl: string, token: string): string {
  const base = walletBaseUrl.replace(/\/+$/, '')
  return `${base}/node/status?token=${encodeURIComponent(token)}`
}

/**
 * Exchange a `session_id` for a status URL.
 *
 * Steps (all failures collapse to a generic `rejected`, never leaking which):
 *   1. Shape-check the session id (`cs_…`).
 *   2. Retrieve the session from Stripe; a 404 / any error → rejected.
 *   3. Require `payment_status === 'paid'` and a `customer` id → else rejected.
 *   4. Look up the node row by that customer id. Absent → `pending` (the webhook
 *      hasn't landed yet); present → mint a token and return the `ready` URL.
 *
 * `nowSeconds` is threaded into `mintStatusToken` for deterministic tests.
 */
export async function exchangeSessionForStatus(
  sessionId: string,
  opts: {
    stripeSecretKey: string
    statusLinkSecret: string
    walletBaseUrl: string
    store: NodeStore
    fetchImpl?: typeof fetch
    nowSeconds?: number
  },
): Promise<SessionExchange> {
  if (!isPlausibleSessionId(sessionId)) {
    return { kind: 'rejected', reason: 'malformed session id' }
  }

  let session: RetrievedSession
  try {
    session = await retrieveCheckoutSession(
      sessionId,
      opts.stripeSecretKey,
      opts.fetchImpl ?? boundFetch,
    )
  } catch (err) {
    const reason =
      err instanceof StripeSessionError
        ? `stripe session retrieve failed (${err.status})`
        : 'stripe session retrieve error'
    return { kind: 'rejected', reason }
  }

  if (session.paymentStatus !== 'paid') {
    return { kind: 'rejected', reason: `session not paid (${session.paymentStatus})` }
  }
  if (!session.customerId) {
    return { kind: 'rejected', reason: 'session has no customer' }
  }

  const node = await opts.store.getByCustomer(session.customerId)
  if (!node) {
    // Paid, but the provisioning webhook hasn't written the row yet.
    return { kind: 'pending' }
  }

  const token = await mintStatusToken(session.customerId, opts.statusLinkSecret, {
    nowSeconds: opts.nowSeconds,
  })
  return { kind: 'ready', statusUrl: buildStatusUrl(opts.walletBaseUrl, token), token }
}
