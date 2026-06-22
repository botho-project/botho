/**
 * Botho-as-a-Service control-plane Worker (#458 §1).
 *
 * MVP surface:
 *   - POST /checkout  create a Stripe Checkout Session (subscription, $50/mo)  (P7.1)
 *   - POST /webhook   Stripe signature verify -> provision/deprovision         (P7.2 / #506)
 *   - GET  /healthz   liveness probe
 *
 * The provisioner core (#502) lives in `provisioner.ts` and is exposed as a
 * function — `provisionRig` / `teardownRig` — that the Stripe webhook (#506)
 * calls. There is deliberately NO public `/provision` route: only the
 * signature-verified webhook may trigger a launch (#458 §5).
 *
 * Out of scope here (later phases of #458 §8):
 *   - /status   user looks up their rig                            (P6.3)
 *
 * All secrets (Stripe key, AWS creds, CF DNS token) come from Worker secrets /
 * vars — never the repo. See `wrangler.toml` and `.dev.vars.example` for the
 * binding contract.
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

export interface Env extends CheckoutEnv, ProvisionerEnv, WebhookEnv {
  /**
   * Comma-separated list of origins allowed to call /checkout from the browser
   * (e.g. "https://botho.io,https://wallet.botho.io"). When unset, CORS is not
   * granted (same-origin only).
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
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
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

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url)
    const origin = request.headers.get('Origin')

    // CORS preflight for the browser "Get a rig" surface.
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

    return jsonResponse({ error: 'not found' }, 404)
  },
} satisfies ExportedHandler<Env>
