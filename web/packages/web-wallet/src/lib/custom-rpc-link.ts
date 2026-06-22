/**
 * Custom-RPC deep link parsing for the Botho Web Wallet (P6.3 of #458, §3 step 5).
 *
 * A managed-rig owner is handed a link like:
 *
 *     https://wallet.botho.io/wallet?rpc=https%3A%2F%2Frig-abc.testnet.botho.io%2Frpc
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
