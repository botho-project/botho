/**
 * Minimal JSON-RPC mock for the seed node read RPC used by the explorer/wallet
 * during e2e.
 *
 * The explorer connects to a node via the same-origin `/rpc` proxy, which in
 * production forwards to https://seed.botho.io. Pointing that proxy at the live
 * node makes the explorer specs depend on a shared public node being responsive
 * — which flaked ~1/run, always stuck in the "Connecting to network..." phase
 * (the connect-time `node_getStatus` call timing out). See issue #334.
 *
 * This server returns fixed, deterministic payloads for the JSON-RPC methods the
 * explorer exercises on load and during search:
 *   - node_getStatus   -> connect handshake + chain height
 *   - getChainInfo     -> difficulty (network stats)
 *   - getBlockByHeight -> recent-block list + block-detail + height search
 *
 * Wire it into the Playwright run by pointing the vite-preview `/rpc` proxy at
 * this server (see playwright.config.ts + web-wallet/vite.config.ts, both honor
 * the E2E_RPC_PROXY_TARGET env var).
 *
 * No external dependencies — uses only Node's built-in http module.
 */
import { createServer } from 'node:http'

const PORT = Number(process.env.RPC_MOCK_PORT ?? 4175)

// Deterministic chain shape served by the mock. CHAIN_HEIGHT is the tip; the
// explorer's getRecentBlocks fetches the top `limit` heights ending at the tip,
// and search-by-height fetches an arbitrary in-range height. Any height in
// [0, CHAIN_HEIGHT] resolves to a synthetic-but-valid block so the recent-block
// list, block-detail view, and height search are all deterministic.
const CHAIN_HEIGHT = 1000
const NETWORK = 'botho-testnet'
const VERSION = '0.1.0-e2e-mock'
const DIFFICULTY = 12345

/** Pad a number into a 64-hex-char string so block/prev hashes look realistic. */
function fakeHash(seed) {
  const base = `e2e${seed}`
  let hex = ''
  for (let i = 0; i < base.length; i++) {
    hex += base.charCodeAt(i).toString(16).padStart(2, '0')
  }
  return hex.padEnd(64, '0').slice(0, 64)
}

/** Build a deterministic block payload for a given height. */
function blockAt(height) {
  return {
    height,
    hash: fakeHash(`block-${height}`),
    prevHash: height > 0 ? fakeHash(`block-${height - 1}`) : fakeHash('genesis'),
    // Fixed base timestamp (2024-01-01T00:00:00Z) + 60s per block, in seconds.
    timestamp: 1704067200 + height * 60,
    difficulty: DIFFICULTY,
    txCount: 0,
    mintingReward: 5_000_000,
  }
}

/** Dispatch a single JSON-RPC method to its fixed result. */
function handleMethod(method, params) {
  switch (method) {
    case 'node_getStatus':
      return {
        result: {
          version: VERSION,
          network: NETWORK,
          chainHeight: CHAIN_HEIGHT,
          peerCount: 1,
          mempoolSize: 0,
        },
      }
    case 'getChainInfo':
      return { result: { difficulty: DIFFICULTY } }
    case 'getBlockByHeight': {
      const height = Number(params?.height)
      if (!Number.isFinite(height) || height < 0 || height > CHAIN_HEIGHT) {
        // Out-of-range heights (e.g. the "non-existent block" search) return a
        // JSON-RPC error, which the adapter maps to a null block -> "not found".
        return { error: { code: -32001, message: 'Block not found' } }
      }
      return { result: blockAt(height) }
    }
    default:
      return { error: { code: -32601, message: `Method not found: ${method}` } }
  }
}

const server = createServer((req, res) => {
  // CORS preflight / headers so the mock works whether reached directly or via
  // the same-origin proxy.
  res.setHeader('Access-Control-Allow-Origin', '*')
  res.setHeader('Access-Control-Allow-Methods', 'POST, OPTIONS')
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type')

  if (req.method === 'OPTIONS') {
    res.writeHead(204)
    res.end()
    return
  }

  // Health check: Playwright's webServer readiness probe issues a GET to the
  // server URL and only treats a < 400 status as "ready". Respond 200 to any
  // non-POST request so the server is detected as up.
  if (req.method !== 'POST') {
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify({ status: 'ok', service: 'botho-e2e-rpc-mock' }))
    return
  }

  let body = ''
  req.on('data', (chunk) => {
    body += chunk
  })
  req.on('end', () => {
    let request
    try {
      request = JSON.parse(body)
    } catch {
      res.writeHead(400, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify({ jsonrpc: '2.0', error: { code: -32700, message: 'Parse error' }, id: null }))
      return
    }

    const { method, params, id } = request ?? {}
    const outcome = handleMethod(method, params)
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify({ jsonrpc: '2.0', id: id ?? null, ...outcome }))
  })
})

// The adapter opens a WebSocket to <origin>/rpc/ws for live updates. There is no
// real-time channel in the mock; reject upgrades so the socket closes cleanly.
// The adapter treats a failed/closed WS as "no real-time updates" and continues
// polling, so this does not block connect.
server.on('upgrade', (_req, socket) => {
  socket.destroy()
})

server.listen(PORT, () => {
  // eslint-disable-next-line no-console
  console.log(`RPC mock listening on http://localhost:${PORT} (chainHeight=${CHAIN_HEIGHT})`)
})
