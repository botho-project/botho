/**
 * Frontend client for the Botho-as-a-Service `/status` + `/portal` endpoints
 * (P6.3 of #458, §4/§6).
 *
 * A managed-rig owner returns to the status page with a **magic-link token**
 * (the MVP identity model — #458 §4: no password, the signed link is the
 * credential). The page calls `fetchRigStatus(token)` to get the rig's RPC URL,
 * lifecycle state, and live health, and `openManageSubscription(token)` to jump
 * to the Stripe Customer Portal.
 *
 * The browser only ever talks to our control-plane Worker, which holds the
 * Stripe secret + the status-link signing secret. No secrets live here.
 */

import { baasEndpoint } from './rig-checkout'

/** Lifecycle state of a rig (mirrors the Worker's `RigState`). */
export type RigState = 'provisioning' | 'running' | 'suspended' | 'terminated'

/** Health summary surfaced from `node_getStatus` (mirrors the Worker). */
export interface RigHealth {
  status: 'online' | 'offline' | 'unknown'
  chainHeight?: number
  synced?: boolean
  syncProgress?: number
}

/** The `/status` response body. */
export interface RigStatus {
  rigId: string
  rpcUrl: string
  state: RigState
  region: string
  health: RigHealth
  /** Deep link that opens this wallet pointed at the rig's RPC. */
  walletDeepLink: string
}

/** Error from a status/portal call, carrying an HTTP status when available. */
export class RigStatusError extends Error {
  constructor(
    message: string,
    public readonly status?: number,
  ) {
    super(message)
    this.name = 'RigStatusError'
  }
}

/** The magic-link token query-param name on the status page URL. */
export const STATUS_TOKEN_PARAM = 'token'

/**
 * Read the magic-link token from a query string (the status page is opened as
 * `/rig/status?token=…`). Returns null when absent.
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
 * Fetch the authenticated user's rig status from the control-plane Worker.
 * `fetchImpl` is injectable for tests.
 *
 * Throws `RigStatusError` (with the HTTP status) on 4xx/5xx so the page can show
 * a specific message — e.g. 401 → "this link has expired", 404 → "no rig yet".
 */
export async function fetchRigStatus(
  token: string,
  fetchImpl: typeof fetch = fetch,
): Promise<RigStatus> {
  const url = `${baasEndpoint()}/status?${STATUS_TOKEN_PARAM}=${encodeURIComponent(token)}`
  let resp: Response
  try {
    resp = await fetchImpl(url, { method: 'GET' })
  } catch {
    throw new RigStatusError('Could not reach the status service. Try again.')
  }

  if (!resp.ok) {
    if (resp.status === 401) {
      throw new RigStatusError('This link is invalid or has expired.', 401)
    }
    if (resp.status === 404) {
      throw new RigStatusError('No rig found for this account yet.', 404)
    }
    throw new RigStatusError('Could not load your rig status.', resp.status)
  }

  try {
    return (await resp.json()) as RigStatus
  } catch {
    throw new RigStatusError('Unexpected response from the status service.', resp.status)
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
    throw new RigStatusError('Could not reach the billing portal. Try again.')
  }

  let json: { url?: string; error?: string }
  try {
    json = (await resp.json()) as typeof json
  } catch {
    throw new RigStatusError('Unexpected response from the billing portal.', resp.status)
  }
  if (!resp.ok || !json.url) {
    throw new RigStatusError(json.error ?? 'Could not open the billing portal.', resp.status)
  }
  return json.url
}
