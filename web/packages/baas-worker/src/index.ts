/**
 * Botho-as-a-Service control-plane Worker (#458 §1).
 *
 * MVP surface for P7.1 — the billing front door:
 *   - POST /checkout  create a Stripe Checkout Session (subscription, $50/mo)
 *   - GET  /healthz   liveness probe
 *
 * The provisioner core (#502) lives in `provisioner.ts` and is exposed as a
 * function — `provisionRig` / `teardownRig` — that the Stripe webhook (P7.2 /
 * #506) will call from a Queue consumer / Durable Object. There is deliberately
 * NO public `/provision` route: only the (future) signature-verified webhook may
 * trigger a launch (#458 §5).
 *
 * Out of scope here (later phases of #458 §8):
 *   - /webhook  Stripe signature verify -> provision/deprovision  (P7.2 / #506)
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
import type { ProvisionerEnv } from './provisioner-env'

// Re-export the provisioner surface so #506 (webhook) can import everything from
// the package entry without reaching into individual modules.
export {
  provisionRig,
  teardownRig,
  deriveRigId,
  type ProvisionRequest,
  type ProvisionOutcome,
  type ProvisionerDeps,
} from './provisioner'
export { depsFromEnv, missingProvisionerEnv, type ProvisionerEnv } from './provisioner-env'

export interface Env extends CheckoutEnv, ProvisionerEnv {
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

    return jsonResponse({ error: 'not found' }, 404)
  },
} satisfies ExportedHandler<Env>
