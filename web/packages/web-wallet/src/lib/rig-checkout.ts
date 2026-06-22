/**
 * Frontend client for the Botho-as-a-Service billing front door (#458 §2).
 *
 * The "Get a rig" surface (`RigPage`) calls `startRigCheckout()`, which POSTs to
 * the control-plane Worker's `/checkout` endpoint (`@botho/baas-worker`) and
 * receives a hosted Stripe Checkout URL to redirect the browser to.
 *
 * Keys/secrets never live here — the browser only ever talks to our Worker, which
 * holds the Stripe secret. This module just shapes the request and surfaces
 * errors for the UI.
 */

/**
 * AWS regions a managed rig can be provisioned in (#458 §5). Mirrors the
 * server-side allowlist in `@botho/baas-worker`; the Worker re-validates, so this
 * is purely to constrain the dropdown. Start with us-west-2 only.
 */
export const RIG_REGIONS: ReadonlyArray<{ id: string; label: string }> = [
  { id: 'us-west-2', label: 'US West (Oregon) — us-west-2' },
]

export const DEFAULT_RIG_REGION = RIG_REGIONS[0].id

/**
 * Base URL of the BaaS control-plane Worker. Configured at build time via
 * `VITE_BAAS_ENDPOINT`; falls back to the production control-plane host.
 */
export function baasEndpoint(): string {
  const fromEnv = import.meta.env.VITE_BAAS_ENDPOINT as string | undefined
  return (fromEnv && fromEnv.length > 0 ? fromEnv : 'https://baas.botho.io').replace(/\/+$/, '')
}

export interface StartRigCheckoutInput {
  /** Desired AWS region (must be one of RIG_REGIONS). */
  region: string
  /** Optional email to pre-fill Stripe checkout. */
  email?: string
}

export interface RigCheckoutSession {
  /** Stripe Checkout Session id. */
  id: string
  /** Hosted Stripe Checkout URL to redirect to. */
  url: string
}

/** Error from the checkout flow, carrying an HTTP status when available. */
export class RigCheckoutError extends Error {
  constructor(
    message: string,
    public readonly status?: number,
  ) {
    super(message)
    this.name = 'RigCheckoutError'
  }
}

/**
 * Create a Stripe Checkout Session via the control-plane Worker and return its
 * id + hosted URL. `fetchImpl` is injectable for tests.
 */
export async function startRigCheckout(
  input: StartRigCheckoutInput,
  fetchImpl: typeof fetch = fetch,
): Promise<RigCheckoutSession> {
  const body: Record<string, unknown> = { region: input.region }
  if (input.email) body.email = input.email

  let resp: Response
  try {
    resp = await fetchImpl(`${baasEndpoint()}/checkout`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
  } catch {
    throw new RigCheckoutError('Could not reach the checkout service. Try again.')
  }

  let json: { id?: string; url?: string; error?: string }
  try {
    json = (await resp.json()) as typeof json
  } catch {
    throw new RigCheckoutError('Unexpected response from the checkout service.', resp.status)
  }

  if (!resp.ok || !json.url || !json.id) {
    throw new RigCheckoutError(json.error ?? 'Could not start checkout.', resp.status)
  }

  return { id: json.id, url: json.url }
}
