/**
 * Multi-node local-network harness for the #396 "fresh node joins a running
 * local chain and catches up" test.
 *
 * Where {@link node-harness.ts} spins up ONE solo-minting node, this harness
 * spawns SEVERAL real `botho run` processes that peer with each other over real
 * libp2p loopback transports. It is used to exercise the REAL bootstrap + #376
 * catch-up paths that the in-process `TestNetwork` sim (DashMap message-passing)
 * and the solo harness do not:
 *
 *   - real libp2p bootstrap (a fresh node dialing an already-running peer),
 *   - the #376 catch-up sync (a fresh follower backfilling 0 -> N and
 *     converging on the running node's exact chain).
 *
 * (Multi-node block *agreement* — two minters reaching one chain — does not
 * work today; see #397. So #396's test uses one solo minter + one follower
 * joiner, which is the capability that works end-to-end over real processes.)
 *
 * Each node:
 *   - has its own throwaway data dir + config (own BIP39 mnemonic),
 *   - binds distinct gossip/rpc ports on 127.0.0.1,
 *   - bootstraps to other nodes via `/ip4/127.0.0.1/tcp/<gossipPort>`
 *     multiaddrs (the transport dials a bare ip4/tcp multiaddr without a peer
 *     id, exactly as `scripts/join-betanet.sh` does).
 *
 * Quorum is selected per node by `minPeers`: `0` -> a solo minter (explicit
 * 1-of-1, like the single-node harness); `>= 1` -> a `recommended` quorum
 * requiring that many connected peers (used by followers).
 *
 * Block timing is fast (`BOTHO_SLOT_DURATION_SECS=1`) and lottery eligibility
 * is tunable (same test-only env overrides as the solo harness), so the test
 * runs within a bounded time.
 *
 * Only used under Node/vitest (shells out to the node binary); imported lazily
 * by the gated test and relies on `node:*` builtins.
 */

import { spawn, type ChildProcess } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync, existsSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

export interface NodeSpec {
  /** Human-readable label, e.g. "A", "B", "joiner". */
  name: string
  /** Valid 24/12-word BIP39 mnemonic. Distinct per node so coinbase is attributable. */
  mnemonic: string
  /** Gossip (libp2p) port. */
  gossipPort: number
  /** JSON-RPC port. */
  rpcPort: number
  /** Gossip ports of peers to bootstrap to (loopback). */
  bootstrapGossipPorts: number[]
  /** Enable minting on this node. */
  mint: boolean
  /**
   * Quorum selector:
   *   - `0`  -> a SOLO minter (explicit 1-of-1 quorum, no peers required).
   *   - `>=1` -> a `recommended` quorum requiring this many connected peers
   *     before the node considers itself in-network (used by followers).
   */
  minPeers: number
}

export interface RunningNode {
  spec: NodeSpec
  /** Base JSON-RPC URL, e.g. http://127.0.0.1:17811/rpc. */
  rpcUrl: string
  /** The node's child process. */
  child: ChildProcess
  /** Path to the node's log file (stdout+stderr). */
  logPath: string
  /** Read the node's accumulated log (best-effort). */
  readLog(): string
}

export interface MultiNodeNetwork {
  nodes: RunningNode[]
  /** Look a running node up by its spec name. */
  get(name: string): RunningNode
  /** Kill all node processes and remove all temp dirs. */
  stop(): Promise<void>
}

export interface MultiNodeOptions {
  /** Path to the botho binary (default: BOTHO_BIN env or repo target/release). */
  binPath?: string
  /** Test-only lottery eligibility overrides forwarded to every node. */
  lottery?: {
    minUtxoAge?: number
    minUtxoValue?: bigint | number
  }
  /** Slot duration in seconds (default 1). */
  slotDurationSecs?: number
}

function env(): Record<string, string | undefined> {
  return (
    (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {}
  )
}

function resolveBinPath(explicit?: string): string {
  const e = env()
  if (explicit) return explicit
  if (e.BOTHO_BIN) return e.BOTHO_BIN
  const cwd = e.PWD ?? process.cwd()
  const candidates = [
    join(cwd, '..', 'target', 'release', 'botho'),
    join(cwd, 'target', 'release', 'botho'),
    join(cwd, '..', '..', 'target', 'release', 'botho'),
  ]
  for (const c of candidates) if (existsSync(c)) return c
  return candidates[0]
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

async function rpcCall<T>(
  url: string,
  method: string,
  params: Record<string, unknown>,
): Promise<T> {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', method, params, id: 1 }),
  })
  const json = (await res.json()) as { result?: T; error?: { message: string } }
  if (json.error) throw new Error(`${method}: ${json.error.message}`)
  return json.result as T
}

/** Read a node's mined chain height (0 if RPC is not up yet). */
export async function nodeHeight(rpcUrl: string): Promise<number> {
  try {
    const s = await rpcCall<{ chainHeight: number }>(rpcUrl, 'node_getStatus', {})
    return s.chainHeight
  } catch {
    return 0
  }
}

/** Read a node's connected peer count (0 if RPC is not up yet). */
export async function nodePeerCount(rpcUrl: string): Promise<number> {
  try {
    const s = await rpcCall<{ peerCount: number }>(rpcUrl, 'node_getStatus', {})
    return s.peerCount
  } catch {
    return 0
  }
}

/**
 * Spawn a multi-node local botho network from the supplied specs. Each node
 * gets its own temp data dir + config; all nodes share the same binary, slot
 * duration and lottery overrides. Returns once every node's RPC is responding
 * (NOT once they have peered/synced — callers poll for those conditions).
 */
export async function startMultiNodeNetwork(
  specs: NodeSpec[],
  opts: MultiNodeOptions = {},
): Promise<MultiNodeNetwork> {
  const binPath = resolveBinPath(opts.binPath)
  if (!existsSync(binPath)) {
    throw new Error(
      `botho binary not found at ${binPath}. Build it with ` +
        '`cargo build --release -p botho` or set BOTHO_BIN.',
    )
  }

  const slot = opts.slotDurationSecs ?? 1
  const lotteryEnv: Record<string, string> = {}
  if (opts.lottery?.minUtxoAge !== undefined) {
    lotteryEnv.BOTHO_LOTTERY_MIN_UTXO_AGE = String(opts.lottery.minUtxoAge)
  }
  if (opts.lottery?.minUtxoValue !== undefined) {
    lotteryEnv.BOTHO_LOTTERY_MIN_UTXO_VALUE = String(opts.lottery.minUtxoValue)
  }

  const running: RunningNode[] = []
  const dataDirs: string[] = []

  const stop = async () => {
    for (const n of running) {
      if (!n.child.killed) n.child.kill('SIGTERM')
    }
    await sleep(500)
    for (const n of running) {
      if (!n.child.killed) n.child.kill('SIGKILL')
    }
    for (const d of dataDirs) {
      try {
        rmSync(d, { recursive: true, force: true })
      } catch {
        /* best effort */
      }
    }
  }

  try {
    for (const spec of specs) {
      const dataDir = mkdtempSync(join(tmpdir(), `botho-e2e-${spec.name}-`))
      dataDirs.push(dataDir)
      const configPath = join(dataDir, 'config.toml')
      const logPath = join(dataDir, 'node.log')

      const bootstrap = spec.bootstrapGossipPorts
        .map((p) => `"/ip4/127.0.0.1/tcp/${p}"`)
        .join(', ')

      // Quorum config:
      //   minPeers === 0 -> a SOLO minter: explicit 1-of-1 quorum (no peers
      //     required), identical to the single-node harness, so the node mines
      //     on its own from genesis.
      //   minPeers >= 1  -> recommended quorum: auto-trust connected peers and
      //     require at least `minPeers` connected before minting. A follower
      //     uses this to require a peer before it considers itself in-network.
      const quorumLines =
        spec.minPeers === 0
          ? ['[network.quorum]', 'mode = "explicit"', 'threshold = 1', 'members = []', 'min_peers = 0']
          : ['[network.quorum]', 'mode = "recommended"', `min_peers = ${spec.minPeers}`]

      writeFileSync(
        configPath,
        [
          'network_type = "testnet"',
          '',
          '[wallet]',
          `mnemonic = "${spec.mnemonic}"`,
          '',
          '[network]',
          `gossip_port = ${spec.gossipPort}`,
          `rpc_port = ${spec.rpcPort}`,
          'metrics_port = 0',
          'cors_origins = ["*"]',
          `bootstrap_peers = [${bootstrap}]`,
          '',
          '[network.dns_seeds]',
          'enabled = false',
          '',
          ...quorumLines,
          '',
          '[minting]',
          `enabled = ${spec.mint}`,
          'threads = 1',
          '',
          '[faucet]',
          '',
        ].join('\n'),
      )

      const args = ['--testnet', '--config', configPath, 'run']
      if (spec.mint) args.push('--mint', '--mint-threads', '1')

      // Append each node's output to its own log file via a shell so we can
      // surface diagnostics on failure (mirrors join-betanet.sh's NODE_LOG).
      const child = spawn(binPath, args, {
        env: {
          ...env(),
          BOTHO_HOME: dataDir,
          BOTHO_SLOT_DURATION_SECS: String(slot),
          RUST_LOG: env().RUST_LOG ?? 'info',
          ...lotteryEnv,
        } as NodeJS.ProcessEnv,
        stdio: ['ignore', 'pipe', 'pipe'],
      })
      const fsChunks: Buffer[] = []
      child.stdout?.on('data', (d: Buffer) => fsChunks.push(d))
      child.stderr?.on('data', (d: Buffer) => fsChunks.push(d))
      child.on('exit', () => {
        try {
          writeFileSync(logPath, Buffer.concat(fsChunks))
        } catch {
          /* best effort */
        }
      })

      running.push({
        spec,
        rpcUrl: `http://127.0.0.1:${spec.rpcPort}/rpc`,
        child,
        logPath,
        readLog: () => {
          try {
            // Prefer in-memory buffer; fall back to the flushed file.
            if (fsChunks.length) return Buffer.concat(fsChunks).toString('utf8')
            if (existsSync(logPath)) return readFileSync(logPath, 'utf8')
          } catch {
            /* best effort */
          }
          return ''
        },
      })
    }

    // Wait for every node's RPC to come up.
    const deadline = Date.now() + 90_000
    for (const n of running) {
      let up = false
      while (Date.now() < deadline) {
        try {
          await rpcCall(n.rpcUrl, 'node_getStatus', {})
          up = true
          break
        } catch {
          if (n.child.exitCode !== null) {
            throw new Error(
              `node ${n.spec.name} exited (code ${n.child.exitCode}) before RPC came up:\n` +
                n.readLog().split('\n').slice(-30).join('\n'),
            )
          }
          await sleep(1000)
        }
      }
      if (!up) {
        throw new Error(`node ${n.spec.name} RPC did not come up within timeout`)
      }
    }

    const network: MultiNodeNetwork = {
      nodes: running,
      get(name: string) {
        const n = running.find((r) => r.spec.name === name)
        if (!n) throw new Error(`no node named ${name}`)
        return n
      },
      stop,
    }
    return network
  } catch (err) {
    await stop()
    throw err
  }
}
