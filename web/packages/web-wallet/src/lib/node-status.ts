/**
 * Frontend client for the Botho-as-a-Service `/status` + `/portal` endpoints
 * (P6.3 of #458, §4/§6).
 *
 * A managed-node owner returns to the status page with a **magic-link token**
 * (the MVP identity model — #458 §4: no password, the signed link is the
 * credential). The page calls `fetchNodeStatus(token)` to get the node's RPC URL,
 * lifecycle state, and live health, and `openManageSubscription(token)` to jump
 * to the Stripe Customer Portal.
 *
 * The browser only ever talks to our control-plane Worker, which holds the
 * Stripe secret + the status-link signing secret. No secrets live here.
 */

import { baasEndpoint } from './node-checkout'

/** Lifecycle state of a node (mirrors the Worker's `NodeState`). */
export type NodeState = 'provisioning' | 'running' | 'suspended' | 'terminated'

/** Health summary surfaced from `node_getStatus` (mirrors the Worker). */
export interface NodeHealth {
  status: 'online' | 'offline' | 'unknown'
  chainHeight?: number
  synced?: boolean
  syncProgress?: number
}

/** The `/status` response body. */
export interface NodeStatus {
  nodeId: string
  rpcUrl: string
  state: NodeState
  region: string
  health: NodeHealth
  /** Deep link that opens this wallet pointed at the node's RPC. */
  walletDeepLink: string
}

/** Error from a status/portal call, carrying an HTTP status when available. */
export class NodeStatusError extends Error {
  constructor(
    message: string,
    public readonly status?: number,
  ) {
    super(message)
    this.name = 'NodeStatusError'
  }
}

/** The magic-link token query-param name on the status page URL. */
export const STATUS_TOKEN_PARAM = 'token'

/**
 * Read the magic-link token from a query string (the status page is opened as
 * `/node/status?token=…`). Returns null when absent.
 */
export function tokenFromSearch(search: string): string | null {
  try {
    const v = new URLSearchParams(search).get(STATUS_TOKEN_PARAM)
    return v && v.trim() !== '' ? v.trim() : null
  } catch {
    return null
  }
}

/**
 * Fetch the authenticated user's node status from the control-plane Worker.
 * `fetchImpl` is injectable for tests.
 *
 * Throws `NodeStatusError` (with the HTTP status) on 4xx/5xx so the page can show
 * a specific message — e.g. 401 → "this link has expired", 404 → "no node yet".
 */
export async function fetchNodeStatus(
  token: string,
  fetchImpl: typeof fetch = fetch,
): Promise<NodeStatus> {
  const url = `${baasEndpoint()}/status?${STATUS_TOKEN_PARAM}=${encodeURIComponent(token)}`
  let resp: Response
  try {
    resp = await fetchImpl(url, { method: 'GET' })
  } catch {
    throw new NodeStatusError('Could not reach the status service. Try again.')
  }

  if (!resp.ok) {
    if (resp.status === 401) {
      throw new NodeStatusError('This link is invalid or has expired.', 401)
    }
    if (resp.status === 404) {
      throw new NodeStatusError('No node found for this account yet.', 404)
    }
    throw new NodeStatusError('Could not load your node status.', resp.status)
  }

  try {
    return (await resp.json()) as NodeStatus
  } catch {
    throw new NodeStatusError('Unexpected response from the status service.', resp.status)
  }
}

/**
 * Open a Stripe Customer Portal session for the authenticated user and return
 * the hosted URL to redirect the browser to. The caller redirects.
 */
export async function createPortalUrl(
  token: string,
  fetchImpl: typeof fetch = fetch,
): Promise<string> {
  let resp: Response
  try {
    resp = await fetchImpl(`${baasEndpoint()}/portal`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ token }),
    })
  } catch {
    throw new NodeStatusError('Could not reach the billing portal. Try again.')
  }

  let json: { url?: string; error?: string }
  try {
    json = (await resp.json()) as typeof json
  } catch {
    throw new NodeStatusError('Unexpected response from the billing portal.', resp.status)
  }
  if (!resp.ok || !json.url) {
    throw new NodeStatusError(json.error ?? 'Could not open the billing portal.', resp.status)
  }
  return json.url
}
