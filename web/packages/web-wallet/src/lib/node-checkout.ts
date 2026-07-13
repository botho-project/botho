/**
 * Frontend client for the Botho-as-a-Service billing front door (#458 §2).
 *
 * The "Get a node" surface (`NodePage`) calls `startNodeCheckout()`, which POSTs to
 * the control-plane Worker's `/checkout` endpoint (`@botho/baas-worker`) and
 * receives a hosted Stripe Checkout URL to redirect the browser to.
 *
 * Keys/secrets never live here — the browser only ever talks to our Worker, which
 * holds the Stripe secret. This module just shapes the request and surfaces
 * errors for the UI.
 */

/**
 * The region catalog shown in the dropdown. `available: true` entries mirror
 * the server-side provisioning allowlist in `@botho/baas-worker` (the Worker
 * re-validates, so this is purely to constrain the UI). The rest are
 * "coming soon": selectable so we can record demand (the choice is sent as
 * `preferredRegion` and lands in Stripe metadata), but the node itself
 * launches in the default region until that datacenter opens.
 *
 * `labelKey` names an i18n key under the `node` namespace (`region.*`) resolved
 * at render time with `t()` — this lib module cannot call `useTranslation()` at
 * module scope, so it carries the key and the page component translates it (the
 * same indirection `AUTO_LOCK_OPTIONS` uses in `wallet.tsx`).
 */
export const NODE_REGIONS: ReadonlyArray<{ id: string; labelKey: string; available: boolean }> = [
  { id: 'us-west-2', labelKey: 'region.usWest2', available: true },
  { id: 'us-east-1', labelKey: 'region.usEast1', available: false },
  { id: 'ca-central-1', labelKey: 'region.caCentral1', available: false },
  { id: 'sa-east-1', labelKey: 'region.saEast1', available: false },
  { id: 'eu-central-1', labelKey: 'region.euCentral1', available: false },
  { id: 'eu-west-2', labelKey: 'region.euWest2', available: false },
  { id: 'af-south-1', labelKey: 'region.afSouth1', available: false },
  { id: 'me-south-1', labelKey: 'region.meSouth1', available: false },
  { id: 'ap-south-1', labelKey: 'region.apSouth1', available: false },
  { id: 'ap-southeast-1', labelKey: 'region.apSoutheast1', available: false },
  { id: 'ap-northeast-1', labelKey: 'region.apNortheast1', available: false },
  { id: 'ap-southeast-2', labelKey: 'region.apSoutheast2', available: false },
]

export const DEFAULT_NODE_REGION = NODE_REGIONS.find((r) => r.available)!.id

/** True if `id` names a catalog region that can be provisioned today. */
export function isRegionAvailable(id: string): boolean {
  return NODE_REGIONS.some((r) => r.id === id && r.available)
}

/**
 * Base URL of the BaaS control-plane Worker. Configured at build time via
 * `VITE_BAAS_ENDPOINT`; falls back to the production control-plane host.
 */
export function baasEndpoint(): string {
  const fromEnv = import.meta.env.VITE_BAAS_ENDPOINT as string | undefined
  return (fromEnv && fromEnv.length > 0 ? fromEnv : 'https://baas.botho.io').replace(/\/+$/, '')
}

export interface StartNodeCheckoutInput {
  /** Region the node will actually be provisioned in (must be available). */
  region: string
  /**
   * The region the user actually wants, when it isn't provisionable yet.
   * Recorded in Stripe metadata as demand data for opening new datacenters.
   */
  preferredRegion?: string
  /** Optional email to pre-fill Stripe checkout. */
  email?: string
}

export interface NodeCheckoutSession {
  /** Stripe Checkout Session id. */
  id: string
  /** Hosted Stripe Checkout URL to redirect to. */
  url: string
}

/** Error from the checkout flow, carrying an HTTP status when available. */
export class NodeCheckoutError extends Error {
  constructor(
    message: string,
    public readonly status?: number,
  ) {
    super(message)
    this.name = 'NodeCheckoutError'
  }
}

/**
 * Create a Stripe Checkout Session via the control-plane Worker and return its
 * id + hosted URL. `fetchImpl` is injectable for tests.
 */
export async function startNodeCheckout(
  input: StartNodeCheckoutInput,
  fetchImpl: typeof fetch = fetch,
): Promise<NodeCheckoutSession> {
  const body: Record<string, unknown> = { region: input.region }
  if (input.preferredRegion && input.preferredRegion !== input.region) {
    body.preferredRegion = input.preferredRegion
  }
  if (input.email) body.email = input.email

  let resp: Response
  try {
    resp = await fetchImpl(`${baasEndpoint()}/checkout`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
  } catch {
    throw new NodeCheckoutError('Could not reach the checkout service. Try again.')
  }

  let json: { id?: string; url?: string; error?: string }
  try {
    json = (await resp.json()) as typeof json
  } catch {
    throw new NodeCheckoutError('Unexpected response from the checkout service.', resp.status)
  }

  if (!resp.ok || !json.url || !json.id) {
    throw new NodeCheckoutError(json.error ?? 'Could not start checkout.', resp.status)
  }

  return { id: json.id, url: json.url }
}
