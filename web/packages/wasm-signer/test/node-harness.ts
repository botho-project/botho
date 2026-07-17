/**
 * Throwaway solo-minting botho node harness for the node-backed #372 test.
 *
 * Spins up `botho run --mint` against a hand-written config in a temp data dir
 * configured for SOLO consensus (explicit quorum, threshold 1, no peers) and
 * FAST block timing (`BOTHO_SLOT_DURATION_SECS=1`, a test-only override added in
 * `botho/src/commands/run.rs`). Polls the node's JSON-RPC until it has mined the
 * requested number of blocks (so a CLSAG decoy ring is available), then hands
 * back the RPC URL + the funded wallet mnemonic. `stop()` kills the node and
 * removes the temp dir.
 *
 * Only used under Node/vitest (the harness shells out to the node binary), so it
 * is imported lazily by the gated test and relies on `node:*` builtins.
 */

import { spawn, type ChildProcess } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

export interface NodeHarness {
  /** Base JSON-RPC URL, e.g. http://127.0.0.1:17599/rpc. */
  rpcUrl: string
  /** The funded (minting) wallet's BIP39 mnemonic. */
  mnemonic: string
  /** Current mined chain height (at the moment startup returned). */
  height: number
  /** Kill the node and clean up the temp data dir. */
  stop(): Promise<void>
}

export interface HarnessOptions {
  /** Mine at least this many blocks before returning (decoy-ring maturity). */
  minBlocks: number
  /** Path to the botho binary (default: BOTHO_BIN env or repo target/release). */
  binPath?: string
  /** RPC port to bind (default 17599). */
  rpcPort?: number
  /** Gossip port to bind (default 17598). */
  gossipPort?: number
  /**
   * Test-only lottery eligibility overrides, forwarded to the node as
   * `BOTHO_LOTTERY_MIN_UTXO_AGE` / `BOTHO_LOTTERY_MIN_UTXO_VALUE`. Lowering
   * these lets freshly created UTXOs enter the per-block lottery draw without
   * pre-mining ~720 blocks, so a payout to a test wallet lands within a bounded
   * number of rounds (issue #394). Both the block proposer and the validator in
   * the solo node read the same env, so the draw stays consensus-deterministic.
   * Omit for production-default eligibility (age 720, value 1 microBTH).
   */
  lottery?: {
    /** Minimum UTXO age (blocks) to be lottery-eligible. */
    minUtxoAge?: number
    /** Minimum UTXO value (picocredits) to be lottery-eligible. */
    minUtxoValue?: bigint | number
  }
}

// A fixed valid 24-word BIP39 phrase for the minting (sender) wallet. This is
// the canonical all-zero-entropy BIP39 test vector (checksum-valid); throwaway
// testnet keys with no value. The node mints coinbase rewards to this wallet.
const SENDER_MNEMONIC =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon ' +
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon ' +
  'abandon abandon abandon abandon abandon art'

function env(): Record<string, string | undefined> {
  return (
    (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {}
  )
}

function resolveBinPath(explicit?: string): string {
  const e = env()
  if (explicit) return explicit
  if (e.BOTHO_BIN) return e.BOTHO_BIN
  // packages/wasm-signer/test -> repo root is five levels up
  // .../web/packages/wasm-signer/test -> .../ (repo root)
  const cwd = e.PWD ?? process.cwd()
  // Try the conventional release path relative to the repo root. The test is
  // typically run from the `web` dir; the repo root is its parent.
  const candidates = [
    join(cwd, '..', 'target', 'release', 'botho'),
    join(cwd, 'target', 'release', 'botho'),
    join(cwd, '..', '..', 'target', 'release', 'botho'),
  ]
  for (const c of candidates) if (existsSync(c)) return c
  // Fall back to the first candidate; spawn will surface a clear error.
  return candidates[0]
}

async function rpcCall<T>(url: string, method: string, params: Record<string, unknown>): Promise<T> {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', method, params, id: 1 }),
  })
  const json = (await res.json()) as { result?: T; error?: { message: string } }
  if (json.error) throw new Error(`${method}: ${json.error.message}`)
  return json.result as T
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

export async function startNodeBackedHarness(opts: HarnessOptions): Promise<NodeHarness> {
  const binPath = resolveBinPath(opts.binPath)
  if (!existsSync(binPath)) {
    throw new Error(
      `botho binary not found at ${binPath}. Build it with ` +
        '`cargo build --release --bin botho` or set BOTHO_BIN.',
    )
  }

  const rpcPort = opts.rpcPort ?? 17599
  const gossipPort = opts.gossipPort ?? 17598
  const rpcUrl = `http://127.0.0.1:${rpcPort}/rpc`

  const dataDir = mkdtempSync(join(tmpdir(), 'botho-e2e-'))
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
      `gossip_port = ${gossipPort}`,
      `rpc_port = ${rpcPort}`,
      'metrics_port = 0',
      'cors_origins = ["*"]',
      'bootstrap_peers = []',
      '',
      '[network.dns_seeds]',
      'enabled = false',
      '',
      // Quorum: RECOMMENDED mode with min_peers = 0, i.e. a genuine solo node.
      // This must NOT be "explicit": since the #770 startup sync gate, an
      // explicit-mode node is seeded `initial_sync_complete = false` and, with
      // zero connected peers, the sync manager stays in Discovery forever — so
      // minting never arms and the harness times out at 0 blocks. A
      // recommended-mode node with `min_peers = 0` takes the solo carve-out
      // (seeded synced) and mints immediately.
      '[network.quorum]',
      'mode = "recommended"',
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

  const lotteryEnv: Record<string, string> = {}
  if (opts.lottery?.minUtxoAge !== undefined) {
    lotteryEnv.BOTHO_LOTTERY_MIN_UTXO_AGE = String(opts.lottery.minUtxoAge)
  }
  if (opts.lottery?.minUtxoValue !== undefined) {
    lotteryEnv.BOTHO_LOTTERY_MIN_UTXO_VALUE = String(opts.lottery.minUtxoValue)
  }

  const child: ChildProcess = spawn(
    binPath,
    ['--testnet', '--config', configPath, 'run', '--mint', '--mint-threads', '1'],
    {
      env: {
        ...env(),
        BOTHO_HOME: dataDir,
        BOTHO_SLOT_DURATION_SECS: '1',
        RUST_LOG: env().RUST_LOG ?? 'warn',
        ...lotteryEnv,
      } as NodeJS.ProcessEnv,
      stdio: 'ignore',
    },
  )

  let stopped = false
  const stop = async () => {
    if (stopped) return
    stopped = true
    if (!child.killed) child.kill('SIGTERM')
    // Give it a moment, then force kill.
    await sleep(500)
    if (!child.killed) child.kill('SIGKILL')
    try {
      rmSync(dataDir, { recursive: true, force: true })
    } catch {
      /* best effort */
    }
  }
  child.on('exit', () => {
    stopped = true
  })

  try {
    // Wait for the RPC server to come up.
    const deadline = Date.now() + 60_000
    let up = false
    while (Date.now() < deadline) {
      try {
        await rpcCall<{ chainHeight: number }>(rpcUrl, 'node_getStatus', {})
        up = true
        break
      } catch {
        await sleep(1000)
      }
    }
    if (!up) throw new Error('node RPC did not come up within 60s')

    // Wait until enough blocks are mined for a decoy ring + funds.
    let height = 0
    const mineDeadline = Date.now() + 240_000
    while (Date.now() < mineDeadline) {
      const status = await rpcCall<{ chainHeight: number }>(rpcUrl, 'node_getStatus', {})
      height = status.chainHeight
      if (height >= opts.minBlocks) break
      await sleep(1000)
    }
    if (height < opts.minBlocks) {
      throw new Error(`node only mined ${height}/${opts.minBlocks} blocks before timeout`)
    }

    return { rpcUrl, mnemonic: SENDER_MNEMONIC, height, stop }
  } catch (err) {
    await stop()
    throw err
  }
}
