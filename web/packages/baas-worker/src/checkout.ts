/**
 * Stripe Checkout Session creation for Botho-as-a-Service managed nodes.
 *
 * This module is the pure, testable core of the `/checkout` endpoint (P7.1 of
 * the BaaS MVP, parent #458 §2). It takes a validated checkout request plus the
 * Worker's environment (Stripe secret key, the $50/mo price ID, success/cancel
 * URLs) and produces the `application/x-www-form-urlencoded` body for Stripe's
 * Checkout Sessions API — without performing any network I/O here, so it can be
 * unit-tested by asserting on the request it builds.
 *
 * Design constraints (from #458 §2, §5, §7):
 *  - mode = subscription, a single recurring $50/mo Price (id from env, never
 *    hard-coded as a live id in the repo).
 *  - The desired AWS region is captured in `metadata.region` for later
 *    provisioning (#458 §3, §9.4) and constrained to a server-side allowlist
 *    (start: us-west-2) — #458 §5.
 *  - STRIPE_SECRET_KEY and the price id live in Worker secrets / env only.
 *  - TEST mode while on testnet: we never assume a particular key prefix, but we
 *    expose a helper to detect test-mode keys for honest UI copy / guard rails.
 */

/**
 * AWS regions a managed node may be provisioned in. Re-exported from the single
 * source of truth in `node-config.ts` (#458 §5: "Region allowlist — start:
 * us-west-2 only") so checkout and the provisioner can never diverge. The
 * frontend dropdown is constrained to this list, and the Worker re-validates so
 * a crafted request can never request an off-list region.
 */
export {
  REGION_ALLOWLIST,
  REGION_CATALOG,
  isAllowedRegion,
  isCatalogRegion,
  type AllowedRegion,
  type CatalogRegion,
} from './node-config'

import type { AllowedRegion } from './node-config'
import { isAllowedRegion, isCatalogRegion } from './node-config'

/**
 * The subset of Worker env this module needs. Bound from Worker secrets / vars
 * (wrangler `[vars]` for the non-secret URLs, `wrangler secret put` for the
 * Stripe key and price id). Never hard-coded in the repo.
 */
export interface CheckoutEnv {
  /** Stripe secret key (TEST mode while on testnet). Worker secret. */
  STRIPE_SECRET_KEY: string
  /** Recurring $50/mo Price id ("price_..."). Worker secret / var. */
  STRIPE_PRICE_ID: string
  /** Absolute URL Stripe redirects to on success. Worker var. */
  CHECKOUT_SUCCESS_URL: string
  /** Absolute URL Stripe redirects to on cancel. Worker var. */
  CHECKOUT_CANCEL_URL: string
}

/** Parsed + validated input for a checkout request. */
export interface CheckoutRequest {
  /** Desired AWS region for the node; must be in REGION_ALLOWLIST. */
  region: AllowedRegion
  /**
   * Region the customer actually wants once it opens; must be in
   * REGION_CATALOG. Demand data only — never used to provision.
   */
  preferredRegion?: string
  /** Optional pre-filled customer email (lets Stripe skip asking). */
  email?: string
}

export type CheckoutValidation =
  | { ok: true; value: CheckoutRequest }
  | { ok: false; error: string }

/**
 * Validate an untrusted request body for the checkout endpoint.
 *
 * Accepts an already-parsed object (the handler decodes JSON or form data first)
 * and enforces the region allowlist plus a light email sanity check. Returns a
 * discriminated union so the handler can map failures to HTTP 400 without
 * throwing.
 */
export function validateCheckoutRequest(body: unknown): CheckoutValidation {
  if (typeof body !== 'object' || body === null) {
    return { ok: false, error: 'request body must be an object' }
  }
  const record = body as Record<string, unknown>

  const region = record.region
  if (typeof region !== 'string' || region.length === 0) {
    return { ok: false, error: 'region is required' }
  }
  if (!isAllowedRegion(region)) {
    return { ok: false, error: `region "${region}" is not in the allowlist` }
  }

  let preferredRegion: string | undefined
  if (
    record.preferredRegion !== undefined &&
    record.preferredRegion !== null &&
    record.preferredRegion !== ''
  ) {
    if (typeof record.preferredRegion !== 'string') {
      return { ok: false, error: 'preferredRegion must be a string' }
    }
    if (!isCatalogRegion(record.preferredRegion)) {
      return {
        ok: false,
        error: `preferredRegion "${record.preferredRegion}" is not in the region catalog`,
      }
    }
    preferredRegion = record.preferredRegion
  }

  let email: string | undefined
  if (record.email !== undefined && record.email !== null && record.email !== '') {
    if (typeof record.email !== 'string') {
      return { ok: false, error: 'email must be a string' }
    }
    // Intentionally lax: Stripe is the source of truth for email validity. We
    // only reject obviously malformed values to avoid passing junk to the API.
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(record.email)) {
      return { ok: false, error: 'email is not a valid address' }
    }
    email = record.email
  }

  return { ok: true, value: { region, preferredRegion, email } }
}

/**
 * Validate that the Worker env carries the secrets/vars this module needs.
 * Returns a list of missing keys (empty if all present) so the handler can fail
 * closed with a 500 rather than calling Stripe with an empty key.
 */
export function missingEnvKeys(env: Partial<CheckoutEnv>): string[] {
  const required: (keyof CheckoutEnv)[] = [
    'STRIPE_SECRET_KEY',
    'STRIPE_PRICE_ID',
    'CHECKOUT_SUCCESS_URL',
    'CHECKOUT_CANCEL_URL',
  ]
  return required.filter((k) => {
    const v = env[k]
    return typeof v !== 'string' || v.length === 0
  })
}

/**
 * True for a Stripe TEST-mode secret key (`sk_test_...` / `rk_test_...`).
 *
 * Used only for honest UI copy / log breadcrumbs (#458 §7) — never to gate
 * behaviour, since Stripe itself is the source of truth for the mode.
 */
export function isTestModeKey(secretKey: string): boolean {
  return /^(sk|rk)_test_/.test(secretKey)
}

/**
 * Build the form-encoded body for Stripe's
 * `POST /v1/checkout/sessions` call.
 *
 * Stripe's API consumes `application/x-www-form-urlencoded` with bracketed keys
 * for nested objects/arrays (e.g. `line_items[0][price]`). We build that body
 * here so it can be asserted in tests without hitting the network.
 *
 * The success URL carries Stripe's `{CHECKOUT_SESSION_ID}` template so the
 * placeholder success page (#458 §4) can later look the session up.
 */
export function buildCheckoutSessionParams(
  req: CheckoutRequest,
  env: CheckoutEnv,
): URLSearchParams {
  const params = new URLSearchParams()
  params.set('mode', 'subscription')

  // Single $50/mo line item; quantity 1 (one node per subscription — #458 §5).
  params.set('line_items[0][price]', env.STRIPE_PRICE_ID)
  params.set('line_items[0][quantity]', '1')

  // Append the Stripe session-id template to the success URL so the success
  // page can resolve which checkout completed (P6.3 wires the status lookup).
  params.set('success_url', appendSessionIdTemplate(env.CHECKOUT_SUCCESS_URL))
  params.set('cancel_url', env.CHECKOUT_CANCEL_URL)

  // Capture the desired region for the provisioner (#458 §3, §9.4). Stored on
  // both the session and the resulting subscription so the webhook (P7.2) can
  // read it from `subscription.metadata` regardless of which event fires.
  params.set('metadata[region]', req.region)
  params.set('subscription_data[metadata][region]', req.region)

  // Demand data (#458 §5 expansion planning): where the customer actually
  // wants to host once that datacenter opens. Read from the Stripe dashboard;
  // deliberately NOT read by the provisioner.
  if (req.preferredRegion && req.preferredRegion !== req.region) {
    params.set('metadata[preferred_region]', req.preferredRegion)
    params.set('subscription_data[metadata][preferred_region]', req.preferredRegion)
  }

  if (req.email) {
    params.set('customer_email', req.email)
  }

  // Subscription mode always creates a Customer (so renewals + the customer
  // portal work) — `customer_creation` is a payment-mode-only param and Stripe
  // rejects it here, so we don't set it.
  params.set('billing_address_collection', 'auto')

  return params
}

/**
 * Append Stripe's `session_id={CHECKOUT_SESSION_ID}` template parameter to a
 * success URL, preserving any existing query string.
 */
export function appendSessionIdTemplate(url: string): string {
  const sep = url.includes('?') ? '&' : '?'
  // The braces are a Stripe-side template, not a value we encode.
  return `${url}${sep}session_id={CHECKOUT_SESSION_ID}`
}

/** Shape returned to the frontend on success. */
export interface CheckoutSessionResult {
  /** Stripe Checkout Session id. */
  id: string
  /** Hosted Stripe Checkout URL to redirect the browser to. */
  url: string
}

/**
 * Create a Stripe Checkout Session by calling the Stripe REST API.
 *
 * `fetchImpl` is injectable so tests can supply a mock and assert on the exact
 * request (URL, auth header, body) without any network access. In the Worker it
 * defaults to the global `fetch`.
 */
export async function createCheckoutSession(
  req: CheckoutRequest,
  env: CheckoutEnv,
  fetchImpl: typeof fetch = fetch,
): Promise<CheckoutSessionResult> {
  const body = buildCheckoutSessionParams(req, env)

  const resp = await fetchImpl('https://api.stripe.com/v1/checkout/sessions', {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${env.STRIPE_SECRET_KEY}`,
      'Content-Type': 'application/x-www-form-urlencoded',
      // Pin the API version so behaviour is reproducible across Stripe rollouts.
      'Stripe-Version': '2024-06-20',
    },
    body: body.toString(),
  })

  const json = (await resp.json()) as {
    id?: string
    url?: string
    error?: { message?: string }
  }

  if (!resp.ok || !json.id || !json.url) {
    const message = json.error?.message ?? `Stripe returned HTTP ${resp.status}`
    throw new StripeCheckoutError(message, resp.status)
  }

  return { id: json.id, url: json.url }
}

/** Error thrown when Stripe rejects the checkout-session creation. */
export class StripeCheckoutError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message)
    this.name = 'StripeCheckoutError'
  }
}
