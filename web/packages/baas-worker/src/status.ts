/**
 * `/status` lookup for the Botho-as-a-Service control plane (P6.3 of #458, §3
 * step 5, §4, §6).
 *
 * Given an authenticated identity (a magic-link status token that binds to one
 * Stripe customer — see `status-link.ts`), return that user's node: its RPC URL,
 * lifecycle state, and a health summary. The summary is derived live from the
 * node's `node_getStatus` RPC (the same method the wallet's node picker and the
 * seed scripts use, #458 §6).
 *
 * AUTHZ (#458 §4, §5): the customer id always comes from the *verified* token,
 * never from the request. The D1 lookup is keyed on that customer id, so a user
 * can only ever see their own node — there is no code path that returns another
 * customer's node. A valid token for customer A can never surface customer B's
 * data.
 *
 * This module is pure/injectable (a `NodeStore` + a `fetch` impl) so tests use an
 * in-memory store and a mocked node fetch — NO real D1 / node call in a test
 * path. The Stripe customer-portal session creator (`createPortalSession`) is
 * likewise injectable.
 */

import type { NodeRecord, NodeState, NodeStore } from './node-store'

/** Health summary for a node, surfaced from `node_getStatus` (#458 §6). */
export interface NodeHealth {
  /** 'online' if node_getStatus succeeded, 'offline' on any error/timeout. */
  status: 'online' | 'offline' | 'unknown'
  /** Current chain height (when online). */
  chainHeight?: number
  /** Whether the node reports itself synced. */
  synced?: boolean
  /** Sync progress percentage 0-100 (when reported). */
  syncProgress?: number
}

/** The body returned by GET /status for an authenticated user. */
export interface StatusResponse {
  /** Short opaque node id (`node-<id>`). */
  nodeId: string
  /** HTTPS `/rpc` URL the user points the PWA at. */
  rpcUrl: string
  /** Lifecycle state of the node. */
  state: NodeState
  /** AWS region the node runs in. */
  region: string
  /** Live health summary (or `unknown` while still provisioning). */
  health: NodeHealth
  /**
   * A deep link that opens the wallet pointed at this node's RPC (#458 §3 step 5).
   * Built from `WALLET_BASE_URL` so the Worker doesn't hard-code the host.
   */
  walletDeepLink: string
}

/** Max time (ms) to wait on a node's node_getStatus before calling it offline. */
const HEALTH_TIMEOUT_MS = 5000

/**
 * Query a node's health via `node_getStatus`. Never throws — returns an
 * 'offline' snapshot on any network/timeout/RPC error so a down node never fails
 * the whole `/status` response. Mirrors the wallet's `fetchNodeHealth`.
 */
export async function fetchNodeHealth(
  rpcUrl: string,
  fetchImpl: typeof fetch = fetch,
): Promise<NodeHealth> {
  try {
    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), HEALTH_TIMEOUT_MS)
    let resp: Response
    try {
      resp = await fetchImpl(rpcUrl, {
        method: 'POST',
        signal: controller.signal,
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          jsonrpc: '2.0',
          method: 'node_getStatus',
          params: {},
          id: 1,
        }),
      })
    } finally {
      clearTimeout(timeoutId)
    }
    if (!resp.ok) return { status: 'offline' }
    const json = (await resp.json()) as {
      result?: { chainHeight?: number; synced?: boolean; syncProgress?: number }
      error?: unknown
    }
    if (json.error || !json.result) return { status: 'offline' }
    return {
      status: 'online',
      chainHeight: json.result.chainHeight,
      synced: json.result.synced,
      syncProgress: json.result.syncProgress,
    }
  } catch {
    return { status: 'offline' }
  }
}

/**
 * Build the wallet deep link that pre-selects a node's RPC endpoint (#458 §3
 * step 5). The wallet reads the `rpc` query param and offers it as the "custom
 * RPC" ingress. `walletBaseUrl` should be the wallet origin (e.g.
 * `https://wallet.botho.io`); the link targets the `/wallet` route.
 */
export function buildWalletDeepLink(walletBaseUrl: string, rpcUrl: string): string {
  const base = walletBaseUrl.replace(/\/+$/, '')
  return `${base}/wallet?rpc=${encodeURIComponent(rpcUrl)}`
}

/**
 * Build a `StatusResponse` for a node record, querying live health unless the node
 * is in a pre-launch / terminal state where a health probe is meaningless.
 *
 * Health is only probed for `running` nodes — a `provisioning` node has no DNS/TLS
 * yet, and `suspended`/`terminated` nodes are intentionally not serving — so we
 * report `unknown` rather than emit a guaranteed-failing probe.
 */
export async function buildStatusResponse(
  node: NodeRecord,
  walletBaseUrl: string,
  fetchImpl: typeof fetch = fetch,
): Promise<StatusResponse> {
  const health: NodeHealth =
    node.state === 'running'
      ? await fetchNodeHealth(node.rpcUrl, fetchImpl)
      : { status: 'unknown' }

  return {
    nodeId: node.nodeId,
    rpcUrl: node.rpcUrl,
    state: node.state,
    region: node.region,
    health,
    walletDeepLink: buildWalletDeepLink(walletBaseUrl, node.rpcUrl),
  }
}

/** Outcome of a status lookup against the store + node. */
export type StatusLookup =
  | { ok: true; status: StatusResponse }
  | { ok: false; code: 'not_found' }

/**
 * Look up the node owned by a *verified* Stripe customer and assemble its status.
 *
 * The caller MUST have already verified the magic-link token and pass the
 * customer id it yielded — this function trusts `customerId` as authenticated.
 * Returns `not_found` when the customer has no node (so the handler answers 404,
 * never leaking whether some *other* customer has one).
 */
export async function lookupStatusForCustomer(
  customerId: string,
  store: NodeStore,
  walletBaseUrl: string,
  fetchImpl: typeof fetch = fetch,
): Promise<StatusLookup> {
  const node = await store.getByCustomer(customerId)
  if (!node) return { ok: false, code: 'not_found' }
  const status = await buildStatusResponse(node, walletBaseUrl, fetchImpl)
  return { ok: true, status }
}

// --- Stripe Customer Portal -------------------------------------------------

/** Error thrown when Stripe rejects the portal-session creation. */
export class StripePortalError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message)
    this.name = 'StripePortalError'
  }
}

/**
 * Create a Stripe Billing (Customer) Portal session for a verified customer so
 * the user can manage/cancel their subscription (#458 §4 "Manage Subscription").
 *
 * `fetchImpl` is injectable so tests assert on the exact request without network
 * I/O. The customer id is the *verified* one from the status token — never from
 * the client — so a user can only open their own portal.
 */
export async function createPortalSession(
  customerId: string,
  returnUrl: string,
  stripeSecretKey: string,
  fetchImpl: typeof fetch = fetch,
): Promise<{ url: string }> {
  const body = new URLSearchParams()
  body.set('customer', customerId)
  body.set('return_url', returnUrl)

  const resp = await fetchImpl('https://api.stripe.com/v1/billing_portal/sessions', {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${stripeSecretKey}`,
      'Content-Type': 'application/x-www-form-urlencoded',
      'Stripe-Version': '2024-06-20',
    },
    body: body.toString(),
  })

  const json = (await resp.json()) as { url?: string; error?: { message?: string } }
  if (!resp.ok || !json.url) {
    const message = json.error?.message ?? `Stripe returned HTTP ${resp.status}`
    throw new StripePortalError(message, resp.status)
  }
  return { url: json.url }
}
