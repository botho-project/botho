/**
 * Botho-as-a-Service control-plane Worker (#458 §1).
 *
 * MVP surface:
 *   - POST /checkout  create a Stripe Checkout Session (subscription, $50/mo)  (P7.1)
 *   - POST /webhook   Stripe signature verify -> provision/deprovision         (P7.2 / #506)
 *   - GET  /status    authenticated user looks up their rig (URL + state +
 *                     live health) via a magic-link token                      (P6.3)
 *   - POST /portal    open a Stripe Customer Portal session (manage/cancel)    (P6.3)
 *   - GET  /healthz   liveness probe
 *
 * The provisioner core (#502) lives in `provisioner.ts` and is exposed as a
 * function — `provisionRig` / `teardownRig` — that the Stripe webhook (#506)
 * calls. There is deliberately NO public `/provision` route: only the
 * signature-verified webhook may trigger a launch (#458 §5).
 *
 * `/status` is read-only and authz-scoped: the customer id always comes from a
 * *verified* magic-link token (`status-link.ts`), and the D1 lookup is keyed on
 * that id, so a user can only ever see their own rig (#458 §4, §5).
 *
 * All secrets (Stripe key, AWS creds, CF DNS token, status-link secret) come
 * from Worker secrets / vars — never the repo. See `wrangler.toml` and
 * `.dev.vars.example` for the binding contract.
 */

import {
  createCheckoutSession,
  missingEnvKeys,
  StripeCheckoutError,
  validateCheckoutRequest,
  type CheckoutEnv,
} from './checkout'
import { depsFromEnv, missingProvisionerEnv, type ProvisionerEnv } from './provisioner-env'
import {
  handleStripeEvent,
  verifyStripeSignature,
  type WebhookEnv,
} from './webhook'
import { verifyStatusToken } from './status-link'
import {
  createPortalSession,
  lookupStatusForCustomer,
  StripePortalError,
} from './status'
import { D1RigStore, type D1Like } from './rig-store'

// Re-export the provisioner surface so the webhook (and any future consumer) can
// import everything from the package entry without reaching into modules.
export {
  provisionRig,
  teardownRig,
  deriveRigId,
  type ProvisionRequest,
  type ProvisionOutcome,
  type ProvisionerDeps,
} from './provisioner'
export { depsFromEnv, missingProvisionerEnv, type ProvisionerEnv } from './provisioner-env'
export {
  handleStripeEvent,
  verifyStripeSignature,
  actionForEventType,
  type WebhookEnv,
} from './webhook'
export { mintStatusToken, verifyStatusToken } from './status-link'
export {
  lookupStatusForCustomer,
  createPortalSession,
  buildWalletDeepLink,
  type StatusResponse,
  type RigHealth,
} from './status'

/** Env keys used only by the `/status` + `/portal` surface (P6.3). */
export interface StatusEnv {
  /**
   * HMAC secret for magic-link status tokens (`status-link.ts`). Worker secret,
   * never the repo. Required for /status and /portal.
   */
  STATUS_LINK_SECRET?: string
  /**
   * Wallet origin used to build the "open in wallet" deep link
   * (e.g. "https://wallet.botho.io"). Worker var.
   */
  WALLET_BASE_URL?: string
  /**
   * Where Stripe returns the user after they close the Customer Portal
   * (e.g. "https://botho.io/rig/status"). Worker var.
   */
  PORTAL_RETURN_URL?: string
}

export interface Env extends CheckoutEnv, ProvisionerEnv, WebhookEnv, StatusEnv {
  /**
   * Comma-separated list of origins allowed to call the browser-facing
   * endpoints (/checkout, /status, /portal). When unset, CORS is not granted
   * (same-origin only).
   */
  ALLOWED_ORIGINS?: string
}

const JSON_HEADERS = { 'Content-Type': 'application/json' }

function corsHeaders(env: Env, requestOrigin: string | null): Record<string, string> {
  if (!requestOrigin || !env.ALLOWED_ORIGINS) return {}
  const allowed = env.ALLOWED_ORIGINS.split(',').map((o) => o.trim())
  if (!allowed.includes(requestOrigin)) return {}
  return {
    'Access-Control-Allow-Origin': requestOrigin,
    'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
    'Access-Control-Allow-Headers': 'Content-Type',
    Vary: 'Origin',
  }
}

function jsonResponse(
  body: unknown,
  status: number,
  extraHeaders: Record<string, string> = {},
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { ...JSON_HEADERS, ...extraHeaders },
  })
}

/**
 * Handle POST /checkout. Exported for direct unit testing without a full Worker
 * runtime; the default export's fetch() delegates to it.
 */
export async function handleCheckout(
  request: Request,
  env: Env,
  fetchImpl: typeof fetch = fetch,
): Promise<Response> {
  const origin = request.headers.get('Origin')
  const cors = corsHeaders(env, origin)

  if (request.method !== 'POST') {
    return jsonResponse({ error: 'method not allowed' }, 405, cors)
  }

  // Fail closed if the Worker is misconfigured — never call Stripe with an
  // empty key, and don't leak which key is missing to the client.
  const missing = missingEnvKeys(env)
  if (missing.length > 0) {
    console.error('checkout: missing env keys', missing)
    return jsonResponse({ error: 'service not configured' }, 500, cors)
  }

  let parsed: unknown
  try {
    parsed = await request.json()
  } catch {
    return jsonResponse({ error: 'invalid JSON body' }, 400, cors)
  }

  const validation = validateCheckoutRequest(parsed)
  if (!validation.ok) {
    return jsonResponse({ error: validation.error }, 400, cors)
  }

  try {
    const session = await createCheckoutSession(validation.value, env, fetchImpl)
    return jsonResponse({ id: session.id, url: session.url }, 200, cors)
  } catch (err) {
    if (err instanceof StripeCheckoutError) {
      console.error('checkout: stripe error', err.status, err.message)
      // 402-ish upstream failures collapse to 502 — the client just retries.
      return jsonResponse({ error: 'could not create checkout session' }, 502, cors)
    }
    console.error('checkout: unexpected error', err)
    return jsonResponse({ error: 'internal error' }, 500, cors)
  }
}

/**
 * Handle POST /webhook — the signature-verified Stripe → provision/deprovision
 * join (P7.2 / #506, #458 §2/§5). Exported for direct unit testing.
 *
 * Order of operations is security-load-bearing:
 *   1. Method gate (POST only).
 *   2. Read the RAW body (never JSON-parse before verifying — the HMAC is over
 *      the exact bytes Stripe signed).
 *   3. Verify the `Stripe-Signature` HMAC + timestamp window. Reject
 *      unsigned/mismatched/stale with 400 BEFORE any side effect.
 *   4. Only now parse the event JSON and dispatch to the provisioner. Idempotency
 *      against Stripe's retries is the provisioner's job (dedup by
 *      subscription_id). Unknown event types are a 2xx no-op.
 *
 * `depsFor` is injectable so tests supply in-memory fakes; in production it
 * defaults to `depsFromEnv(env)` (real EC2/DNS/D1 clients).
 *
 * No CORS here: Stripe calls server-to-server, not from a browser.
 */
export async function handleWebhook(
  request: Request,
  env: Env,
  depsFor: (env: Env) => ReturnType<typeof depsFromEnv> = (e) => depsFromEnv(e),
): Promise<Response> {
  if (request.method !== 'POST') {
    return jsonResponse({ error: 'method not allowed' }, 405)
  }

  // Fail closed if the signing secret is unset — never accept an unverifiable
  // webhook. Don't leak which key is missing.
  if (!env.STRIPE_WEBHOOK_SECRET) {
    console.error('webhook: STRIPE_WEBHOOK_SECRET not configured')
    return jsonResponse({ error: 'service not configured' }, 500)
  }

  // The provisioner must be configured too, or a verified event has nowhere to
  // go. Surface a 500 (not 400) so Stripe retries once we're configured rather
  // than treating it as a permanent client error.
  const missingProv = missingProvisionerEnv(env)
  if (missingProv.length > 0) {
    console.error('webhook: provisioner not configured', missingProv)
    return jsonResponse({ error: 'service not configured' }, 500)
  }

  // (2) Raw body — exact bytes, no parsing yet.
  const rawBody = await request.text()
  const signature = request.headers.get('Stripe-Signature')

  // (3) Verify BEFORE any side effect.
  const verified = await verifyStripeSignature(rawBody, signature, env.STRIPE_WEBHOOK_SECRET)
  if (!verified.ok) {
    console.warn('webhook: signature rejected:', verified.reason)
    return jsonResponse({ error: 'invalid signature' }, 400)
  }

  // (4) Now it is safe to parse the verified payload.
  let event: unknown
  try {
    event = JSON.parse(rawBody)
  } catch {
    return jsonResponse({ error: 'invalid JSON body' }, 400)
  }

  let deps: ReturnType<typeof depsFromEnv>
  try {
    deps = depsFor(env)
  } catch (err) {
    console.error('webhook: failed to build provisioner deps', err)
    return jsonResponse({ error: 'service not configured' }, 500)
  }

  try {
    const handled = await handleStripeEvent(event as never, deps)
    // ACK Stripe with 2xx. A failed provision/teardown is logged but still
    // acked so Stripe doesn't hammer us — the provisioner is idempotent and the
    // reconciliation cron (SEC #508) is the safety net for stuck work.
    if (handled.action === 'provision' && !handled.outcome.ok) {
      console.error('webhook: provision failed', handled.subscriptionId, handled.outcome.code)
    } else if (handled.action === 'teardown' && !handled.result.ok) {
      console.error('webhook: teardown failed', handled.subscriptionId, handled.result.error)
    }
    return jsonResponse({ received: true, action: handled.action }, 200)
  } catch (err) {
    // Unexpected error -> 500 so Stripe retries (idempotency makes this safe).
    console.error('webhook: handler error', err)
    return jsonResponse({ error: 'internal error' }, 500)
  }
}

/**
 * Build a `RigStore` from the D1 binding for the read-only `/status` + `/portal`
 * surface. Kept separate from the provisioner's `depsFromEnv` because status
 * needs only the store (no EC2/DNS/AWS creds), so a misconfigured provisioner
 * never blocks a user from reading their own rig.
 */
function storeFromEnv(env: Env): D1RigStore {
  if (env.DB == null) {
    throw new Error('status: DB binding not configured')
  }
  return new D1RigStore(env.DB as D1Like)
}

/**
 * Handle GET /status — the authenticated rig lookup (P6.3, #458 §4/§6).
 *
 * Authz model: the customer id is taken ONLY from the verified magic-link token
 * (`?token=`), never from the request. The D1 lookup is keyed on that id, so a
 * user can only ever see their own rig. Exported for direct unit testing.
 *
 *   200 -> { rigId, rpcUrl, state, region, health, walletDeepLink }
 *   400 -> missing token
 *   401 -> invalid / expired / forged token (no data leak)
 *   404 -> token valid but this customer has no rig
 *   500 -> service not configured
 */
export async function handleStatus(
  request: Request,
  env: Env,
  fetchImpl: typeof fetch = fetch,
): Promise<Response> {
  const origin = request.headers.get('Origin')
  const cors = corsHeaders(env, origin)

  if (request.method !== 'GET') {
    return jsonResponse({ error: 'method not allowed' }, 405, cors)
  }

  // Fail closed if the signing secret / wallet base url are unset.
  if (!env.STATUS_LINK_SECRET || !env.WALLET_BASE_URL) {
    console.error('status: not configured (STATUS_LINK_SECRET / WALLET_BASE_URL)')
    return jsonResponse({ error: 'service not configured' }, 500, cors)
  }

  const url = new URL(request.url)
  const token = url.searchParams.get('token')
  if (!token) {
    return jsonResponse({ error: 'token is required' }, 400, cors)
  }

  const verified = await verifyStatusToken(token, env.STATUS_LINK_SECRET)
  if (!verified.ok) {
    // Generic 401 — never reveal which check failed or whether a rig exists.
    console.warn('status: token rejected:', verified.reason)
    return jsonResponse({ error: 'unauthorized' }, 401, cors)
  }

  let store: D1RigStore
  try {
    store = storeFromEnv(env)
  } catch (err) {
    console.error('status: store unavailable', err)
    return jsonResponse({ error: 'service not configured' }, 500, cors)
  }

  try {
    const result = await lookupStatusForCustomer(
      verified.customerId,
      store,
      env.WALLET_BASE_URL,
      fetchImpl,
    )
    if (!result.ok) {
      return jsonResponse({ error: 'no rig found' }, 404, cors)
    }
    return jsonResponse(result.status, 200, cors)
  } catch (err) {
    console.error('status: lookup error', err)
    return jsonResponse({ error: 'internal error' }, 500, cors)
  }
}

/**
 * Handle POST /portal — open a Stripe Customer Portal session so the user can
 * manage/cancel their subscription (P6.3, #458 §4). The customer id is taken
 * from the verified status token in the JSON body (`{ token }`), never from the
 * client directly. Exported for direct unit testing.
 *
 *   200 -> { url }     hosted Stripe portal URL to redirect to
 *   400 -> missing token
 *   401 -> invalid/expired token
 *   500 -> service not configured
 *   502 -> Stripe rejected the request
 */
export async function handlePortal(
  request: Request,
  env: Env,
  fetchImpl: typeof fetch = fetch,
): Promise<Response> {
  const origin = request.headers.get('Origin')
  const cors = corsHeaders(env, origin)

  if (request.method !== 'POST') {
    return jsonResponse({ error: 'method not allowed' }, 405, cors)
  }

  if (!env.STATUS_LINK_SECRET || !env.STRIPE_SECRET_KEY || !env.PORTAL_RETURN_URL) {
    console.error('portal: not configured')
    return jsonResponse({ error: 'service not configured' }, 500, cors)
  }

  let parsed: unknown
  try {
    parsed = await request.json()
  } catch {
    return jsonResponse({ error: 'invalid JSON body' }, 400, cors)
  }
  const token =
    typeof parsed === 'object' && parsed !== null
      ? (parsed as Record<string, unknown>).token
      : undefined
  if (typeof token !== 'string' || token.length === 0) {
    return jsonResponse({ error: 'token is required' }, 400, cors)
  }

  const verified = await verifyStatusToken(token, env.STATUS_LINK_SECRET)
  if (!verified.ok) {
    console.warn('portal: token rejected:', verified.reason)
    return jsonResponse({ error: 'unauthorized' }, 401, cors)
  }

  try {
    const session = await createPortalSession(
      verified.customerId,
      env.PORTAL_RETURN_URL,
      env.STRIPE_SECRET_KEY,
      fetchImpl,
    )
    return jsonResponse({ url: session.url }, 200, cors)
  } catch (err) {
    if (err instanceof StripePortalError) {
      console.error('portal: stripe error', err.status, err.message)
      return jsonResponse({ error: 'could not open portal' }, 502, cors)
    }
    console.error('portal: unexpected error', err)
    return jsonResponse({ error: 'internal error' }, 500, cors)
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url)
    const origin = request.headers.get('Origin')

    // CORS preflight for the browser "Get a rig" / status surfaces.
    if (request.method === 'OPTIONS') {
      return new Response(null, { status: 204, headers: corsHeaders(env, origin) })
    }

    if (url.pathname === '/healthz') {
      return jsonResponse({ ok: true }, 200)
    }

    if (url.pathname === '/checkout') {
      return handleCheckout(request, env)
    }

    if (url.pathname === '/webhook') {
      return handleWebhook(request, env)
    }

    if (url.pathname === '/status') {
      return handleStatus(request, env)
    }

    if (url.pathname === '/portal') {
      return handlePortal(request, env)
    }

    return jsonResponse({ error: 'not found' }, 404)
  },
} satisfies ExportedHandler<Env>
