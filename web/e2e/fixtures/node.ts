/**
 * Test helpers for the Botho-as-a-Service "Host a node" e2e flow (#458 §2/§4).
 *
 * The web-wallet build used by the e2e suite leaves `VITE_BAAS_ENDPOINT` unset,
 * so `baasEndpoint()` falls back to the production control-plane host
 * `https://baas.botho.io`. These tests never hit that host: every node spec
 * installs `page.route(...)` handlers (below) that intercept the `/checkout`,
 * `/status`, and `/portal` calls and reply with deterministic fixtures. This
 * keeps the node e2e coverage hermetic — no Stripe, no Worker, no AWS.
 *
 * The frontend redirects the browser (`window.location.assign`) on both a
 * successful checkout and "Manage subscription". To keep those redirects inside
 * the app (so navigation resolves and we can assert on it) the fixtures point
 * the returned URLs at in-app routes rather than real Stripe URLs.
 */
import type { Page, Route } from '@playwright/test'

const WEB_BASE = process.env.E2E_WEB_BASE_URL ?? 'http://localhost:4173'

/**
 * URL matchers for the three control-plane endpoints the frontend calls. Anchored
 * to the `baas.botho.io` host so they never accidentally intercept the wallet's
 * own same-origin `/rpc` traffic.
 */
export const BAAS_CHECKOUT_RE = /\/\/baas\.botho\.io\/checkout$/
export const BAAS_STATUS_RE = /\/\/baas\.botho\.io\/status(\?|$)/
export const BAAS_PORTAL_RE = /\/\/baas\.botho\.io\/portal$/

/**
 * Where the mocked checkout / portal responses redirect to. Real Stripe URLs
 * would navigate the browser off-app (and fail to load in CI); pointing at
 * in-app routes lets the redirect complete so specs can assert on the result.
 */
export const MOCK_STRIPE_CHECKOUT_URL = `${WEB_BASE}/node/success?session_id=cs_test_e2e`
export const MOCK_STRIPE_PORTAL_URL = `${WEB_BASE}/?portal=return`

/** A healthy, running node — the happy-path `/status` payload. */
export const RUNNING_NODE = {
  nodeId: 'node-e2e0001',
  rpcUrl: 'https://node-e2e0001.testnet.botho.io/rpc',
  state: 'running' as const,
  region: 'us-west-2',
  health: { status: 'online' as const, chainHeight: 4321, synced: true },
  walletDeepLink: `${WEB_BASE}/wallet?rpc=${encodeURIComponent(
    'https://node-e2e0001.testnet.botho.io/rpc',
  )}`,
}

/** A freshly-created node that is still booting (health not yet reporting). */
export const PROVISIONING_NODE = {
  nodeId: 'node-e2e0002',
  rpcUrl: 'https://node-e2e0002.testnet.botho.io/rpc',
  state: 'provisioning' as const,
  region: 'us-west-2',
  health: { status: 'unknown' as const },
  walletDeepLink: `${WEB_BASE}/wallet?rpc=${encodeURIComponent(
    'https://node-e2e0002.testnet.botho.io/rpc',
  )}`,
}

/** Reply to a route with a JSON body + status (mirrors the Worker's responses). */
export async function fulfillJson(route: Route, status: number, body: unknown): Promise<void> {
  await route.fulfill({
    status,
    contentType: 'application/json',
    body: JSON.stringify(body),
  })
}

/**
 * Intercept `POST /checkout` with a successful Stripe session, capturing the
 * request body the frontend sent. Returns a getter for that body so specs can
 * assert the exact `{ region, email? }` shape the Worker would receive.
 */
export function captureCheckout(
  page: Page,
  opts: { delayMs?: number } = {},
): () => Record<string, unknown> | undefined {
  let body: Record<string, unknown> | undefined
  void page.route(BAAS_CHECKOUT_RE, async (route) => {
    body = route.request().postDataJSON() as Record<string, unknown>
    if (opts.delayMs) await new Promise((r) => setTimeout(r, opts.delayMs))
    await fulfillJson(route, 200, { id: 'cs_test_e2e', url: MOCK_STRIPE_CHECKOUT_URL })
  })
  return () => body
}
