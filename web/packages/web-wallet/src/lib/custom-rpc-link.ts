/**
 * Custom-RPC deep link parsing for the Botho Web Wallet (P6.3 of #458, §3 step 5).
 *
 * A managed-node owner is handed a link like:
 *
 *     https://botho.io/wallet?rpc=https%3A%2F%2Fnode-abc.testnet.botho.io%2Frpc
 *
 * Opening it should point the wallet's RPC ingress at their own node. The wallet
 * already supports a "custom RPC" ingress (see `setCustomEndpoint` in the network
 * context + the NetworkSelector picker); this module just extracts and validates
 * the `rpc` query param so the network context can apply it on load.
 *
 * Validation is deliberately strict (the endpoint is used for all read/write RPC
 * and is taken from a URL the user clicked):
 *   - must be a syntactically valid absolute URL,
 *   - must be HTTPS (or http://localhost for local dev),
 *   - we do NOT fetch it here — reachability is checked by the network context's
 *     `setCustomEndpoint` (`node_getStatus`) before it is committed.
 *
 * SECURITY GUARDRAIL (#587): HTTPS validation is necessary but NOT sufficient.
 * A `?rpc=` link is attacker-controllable (Signal/QR/paste all carry it), and an
 * attacker's node is perfectly valid HTTPS — it can lie about balances and
 * confirmations, censor/withhold the user's transactions, and harvest addresses
 * + IP. Therefore a parsed link MUST NEVER silently switch the active node. The
 * network context surfaces a parsed link as a *pending* trust prompt
 * (`pendingRpcLink`) and only applies it after an explicit user accept; a
 * decline leaves the prior node intact. Anyone wiring a new consumer of
 * `parseRpcDeepLink` must route it through that trust gate — do NOT call
 * `setCustomEndpoint` directly from a deep link. See `classifyRpcHost` below and
 * the `CustomRpcTrustGate` component.
 */

/** The query-string key carrying a custom RPC endpoint in a deep link. */
export const RPC_PARAM = 'rpc'

export type ParsedRpcLink =
  | { ok: true; rpcUrl: string }
  | { ok: false; reason: string }
  | { ok: 'absent' }

/**
 * Validate a candidate RPC endpoint URL. Returns the normalized URL on success.
 *
 * Accepts `https://…` anywhere, and `http://localhost`/`http://127.0.0.1` for
 * local development. Rejects non-URLs, non-HTTP(S) schemes, and plain-`http`
 * non-loopback hosts (which would be an insecure ingress).
 */
export function isValidRpcUrl(candidate: string): boolean {
  let url: URL
  try {
    url = new URL(candidate)
  } catch {
    return false
  }
  if (url.protocol === 'https:') return true
  if (url.protocol === 'http:') {
    return url.hostname === 'localhost' || url.hostname === '127.0.0.1'
  }
  return false
}

/**
 * Parse the `rpc` deep-link param out of a query string (or full URL search).
 *
 * @param search The `window.location.search` value (e.g. `"?rpc=https%3A%2F..."`)
 *               or any string `URLSearchParams` accepts.
 * @returns `{ ok: 'absent' }` when there is no `rpc` param, `{ ok: true, rpcUrl }`
 *          for a valid one, or `{ ok: false, reason }` for a present-but-invalid
 *          value.
 */
export function parseRpcDeepLink(search: string): ParsedRpcLink {
  let params: URLSearchParams
  try {
    params = new URLSearchParams(search)
  } catch {
    return { ok: 'absent' }
  }
  const raw = params.get(RPC_PARAM)
  if (raw === null || raw.trim() === '') return { ok: 'absent' }

  const candidate = raw.trim()
  if (!isValidRpcUrl(candidate)) {
    return { ok: false, reason: 'The rpc link is not a valid https:// endpoint.' }
  }
  return { ok: true, rpcUrl: candidate }
}

/**
 * Build a wallet deep link that pre-selects a custom RPC endpoint. Mirrors the
 * Worker's `buildWalletDeepLink` so the two never diverge. Used by the status
 * page's "Open in wallet" button.
 */
export function buildWalletRpcLink(walletPath: string, rpcUrl: string): string {
  const sep = walletPath.includes('?') ? '&' : '?'
  return `${walletPath}${sep}${RPC_PARAM}=${encodeURIComponent(rpcUrl)}`
}

/**
 * Extract the bare host (no port) from a candidate RPC URL, or `null` when the
 * URL cannot be parsed. Used by the trust gate to name the node the link wants
 * to point the wallet at ("This link wants to point your wallet at <host>").
 */
export function rpcLinkHost(candidate: string): string | null {
  try {
    return new URL(candidate).hostname
  } catch {
    return null
  }
}

/**
 * Host suffixes operated by the Botho project. A link whose host falls under one
 * of these is shown as a "known operator" hint (e.g. a BaaS-provisioned node at
 * `node-abc.testnet.botho.io`); anything else is an UNKNOWN host shown with a
 * stronger warning. This is a hint only — it never bypasses the trust gate, it
 * just tunes the wording so a fully-arbitrary `evil.example` cannot masquerade
 * as a first-party node.
 */
export const KNOWN_RPC_HOST_SUFFIXES = ['.botho.io'] as const

/** Hosts that are exactly the operator apex (not just a subdomain). */
const KNOWN_RPC_HOST_EXACT = ['botho.io'] as const

export type RpcHostTrust = 'known' | 'unknown'

/**
 * Classify a candidate RPC URL's host as a `known` Botho-operated host or an
 * `unknown` third-party host. Loopback (localhost / 127.0.0.1) counts as
 * `known` so local-dev links don't trip the stronger-warning path. An
 * unparseable URL is treated as `unknown` (most conservative).
 *
 * IMPORTANT: this is a UI hint, not an authorization decision. Both `known` and
 * `unknown` hosts still require explicit user acceptance via the trust gate
 * (#587) — see the security note at the top of this file.
 */
export function classifyRpcHost(candidate: string): RpcHostTrust {
  const host = rpcLinkHost(candidate)
  if (host === null) return 'unknown'
  const lower = host.toLowerCase()
  if (lower === 'localhost' || lower === '127.0.0.1') return 'known'
  if (KNOWN_RPC_HOST_EXACT.includes(lower as (typeof KNOWN_RPC_HOST_EXACT)[number])) {
    return 'known'
  }
  if (KNOWN_RPC_HOST_SUFFIXES.some((suffix) => lower.endsWith(suffix))) return 'known'
  return 'unknown'
}
