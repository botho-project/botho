/**
 * Real local botho node for the full-stack e2e wallet flow (#372 layer b).
 *
 * The default e2e suite points the vite-preview `/rpc` proxy at the static RPC
 * mock (`serve-rpc-mock.mjs`), which deliberately does NOT implement `tx_submit`
 * — fine for the explorer/connect specs, useless for a real send. The
 * full-stack send spec instead needs a REAL node so the browser wallet can
 * build+sign+submit a transaction the node actually accepts and mines.
 *
 * This script launches `botho run --mint` as a solo-minting node (explicit
 * quorum, threshold 1, no peers) with fast block timing
 * (`BOTHO_SLOT_DURATION_SECS`, a test/dev override in `commands/run.rs`) in a
 * throwaway data dir, pre-mines enough blocks for a CLSAG decoy ring, and keeps
 * running until killed. Point the proxy at it with:
 *
 *   E2E_RPC_PROXY_TARGET=http://127.0.0.1:17599
 *
 * The node mints to the canonical 24-word BIP39 test vector
 * (`abandon ... art` == fixtures' TEST_MNEMONIC_24), so importing that mnemonic
 * into the browser wallet yields a funded wallet that can send.
 *
 * Env:
 *   BOTHO_BIN            path to the botho binary (default ../target/release/botho)
 *   E2E_NODE_RPC_PORT    RPC port (default 17599)
 *   E2E_NODE_GOSSIP_PORT gossip port (default 17598)
 *   E2E_NODE_MIN_BLOCKS  blocks to pre-mine before reporting ready (default 23)
 *
 * No external dependencies — uses only Node builtins.
 */
import { spawn } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer } from 'node:http'

const here = dirname(fileURLToPath(import.meta.url))
const webDir = resolve(here, '..')
const repoRoot = resolve(webDir, '..')

const RPC_PORT = Number(process.env.E2E_NODE_RPC_PORT ?? 17599)
const GOSSIP_PORT = Number(process.env.E2E_NODE_GOSSIP_PORT ?? 17598)
const MIN_BLOCKS = Number(process.env.E2E_NODE_MIN_BLOCKS ?? 23)
// Health/readiness port Playwright's `webServer.url` probes with a GET. The node
// RPC only answers POST /rpc, so a GET-based probe against it never reports
// ready; this tiny sidecar returns 200 ONLY once the node has mined MIN_BLOCKS.
const HEALTH_PORT = Number(process.env.E2E_NODE_HEALTH_PORT ?? 17600)
const BIN = process.env.BOTHO_BIN || join(repoRoot, 'target', 'release', 'botho')
const RPC_URL = `http://127.0.0.1:${RPC_PORT}/rpc`

let nodeReady = false
const healthServer = createServer((req, res) => {
  res.setHeader('Access-Control-Allow-Origin', '*')
  if (nodeReady) {
    res.writeHead(200, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify({ status: 'ready', rpc: RPC_URL }))
  } else {
    res.writeHead(503, { 'Content-Type': 'application/json' })
    res.end(JSON.stringify({ status: 'starting' }))
  }
})
healthServer.listen(HEALTH_PORT)

const SENDER_MNEMONIC =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon ' +
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon ' +
  'abandon abandon abandon abandon abandon art'

if (!existsSync(BIN)) {
  console.error(
    `[serve-node] botho binary not found at ${BIN}.\n` +
      '  Build it with `cargo build --release --bin botho` or set BOTHO_BIN.',
  )
  process.exit(1)
}

const dataDir = mkdtempSync(join(tmpdir(), 'botho-e2e-node-'))
const configPath = join(dataDir, 'config.toml')
writeFileSync(
  configPath,
  [
    'network_type = "testnet"',
    '',
    '[wallet]',
    `mnemonic = "${SENDER_MNEMONIC}"`,
    '',
    '[network]',
    `gossip_port = ${GOSSIP_PORT}`,
    `rpc_port = ${RPC_PORT}`,
    'metrics_port = 0',
    'cors_origins = ["*"]',
    'bootstrap_peers = []',
    '',
    '[network.dns_seeds]',
    'enabled = false',
    '',
    '[network.quorum]',
    'mode = "explicit"',
    'threshold = 1',
    'members = []',
    'min_peers = 0',
    '',
    '[minting]',
    'enabled = true',
    'threads = 1',
    '',
    '[faucet]',
    '',
  ].join('\n'),
)

const child = spawn(
  BIN,
  ['--testnet', '--config', configPath, 'run', '--mint', '--mint-threads', '1'],
  {
    env: {
      ...process.env,
      BOTHO_HOME: dataDir,
      BOTHO_SLOT_DURATION_SECS: process.env.BOTHO_SLOT_DURATION_SECS ?? '1',
      RUST_LOG: process.env.RUST_LOG ?? 'warn',
    },
    stdio: 'inherit',
  },
)

let shuttingDown = false
function shutdown() {
  if (shuttingDown) return
  shuttingDown = true
  try {
    if (!child.killed) child.kill('SIGTERM')
  } catch {
    /* ignore */
  }
  try {
    healthServer.close()
  } catch {
    /* ignore */
  }
  try {
    rmSync(dataDir, { recursive: true, force: true })
  } catch {
    /* ignore */
  }
}
process.on('SIGINT', () => {
  shutdown()
  process.exit(0)
})
process.on('SIGTERM', () => {
  shutdown()
  process.exit(0)
})
process.on('exit', shutdown)
child.on('exit', (code) => {
  if (!shuttingDown) {
    console.error(`[serve-node] node exited unexpectedly (code ${code})`)
    process.exit(code ?? 1)
  }
})

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
async function rpc(method, params = {}) {
  const res = await fetch(RPC_URL, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', method, params, id: 1 }),
  })
  const json = await res.json()
  if (json.error) throw new Error(json.error.message)
  return json.result
}

;(async () => {
  // Wait for RPC.
  const upDeadline = Date.now() + 60_000
  let up = false
  while (Date.now() < upDeadline) {
    try {
      await rpc('node_getStatus')
      up = true
      break
    } catch {
      await sleep(1000)
    }
  }
  if (!up) {
    console.error('[serve-node] RPC did not come up within 60s')
    shutdown()
    process.exit(1)
  }

  // Pre-mine enough blocks for a decoy ring + funds.
  const mineDeadline = Date.now() + 240_000
  let height = 0
  while (Date.now() < mineDeadline) {
    height = (await rpc('node_getStatus')).chainHeight
    if (height >= MIN_BLOCKS) break
    await sleep(1000)
  }
  nodeReady = true
  console.log(
    `[serve-node] ready: RPC ${RPC_URL}, health http://127.0.0.1:${HEALTH_PORT}, ` +
      `height ${height}, funded mnemonic = TEST_MNEMONIC_24`,
  )
})().catch((err) => {
  console.error('[serve-node] startup failed:', err)
  shutdown()
  process.exit(1)
})
