/**
 * Node-backed "fresh node JOINS a running local chain and catches up" test
 * (issue #396), exercised against REAL `botho run` processes peering over real
 * libp2p loopback.
 *
 * This is the hermetic, real-process demonstration of the first half of the
 * "run your own node and participate" capability: a fresh node, starting with
 * an empty ledger, bootstraps to an already-running chain over real libp2p (no
 * live network), CONNECTS, and CATCHES UP from 0 to the network tip N via the
 * real #376 catch-up sync — converging on the SAME blocks (identical hashes) as
 * the running node. It is distinct from:
 *   - `e2e_consensus_integration` (all nodes start at genesis together; no node
 *     ever JOINS mid-chain),
 *   - the in-process `TestNetwork` sim (DashMap message-passing; no real
 *     bootstrap/sync/run-loop), and
 *   - `scripts/join-betanet.sh` (joins the LIVE betanet over the internet; not
 *     hermetic, not a CI gate).
 *
 * Topology
 * --------
 * Two real nodes on 127.0.0.1, each with its own throwaway data dir + config +
 * BIP39 mnemonic and distinct gossip/rpc ports:
 *   - A: a solo minter (explicit 1-of-1 quorum, like the single-node harness),
 *     started first, mining the chain to a height N > ring size (real history +
 *     decoy outputs).
 *   - B: a fresh FOLLOWER (minting disabled), started after A reaches N,
 *     bootstrapping to A. It must connect and catch up 0 -> N over the real
 *     sync, ending on the exact same block hashes as A.
 *
 * Why a follower (and not a third minter that mines an accepted block)
 * --------------------------------------------------------------------
 * The original #396 goal also asked for the joiner to MINT a block the network
 * accepts. While building this test against real processes I found that Botho
 * has **no working multi-node block agreement today** (filed as #397):
 *   1. Real multi-node SCP cannot exchange messages — every received SCP ballot
 *      fails to deserialize ("Bincode does not support
 *      Deserializer::deserialize_identifier"), so a real multi-member quorum
 *      stalls at height 0 and produces no blocks.
 *   2. In "solo mode" (the path that actually mints), the consensus quorum set
 *      is frozen at startup and never updated when peers connect, so two peered
 *      minters each mine a DIVERGENT chain and reject each other's blocks
 *      ("Previous block hash mismatch").
 * Consequently a joined node can sync as a follower (proven here) but cannot
 * currently have a mined block accepted into a shared chain by established
 * peers. The "joiner mines an accepted block + earns coinbase" half of #396 is
 * therefore tracked in #397 (fix multi-node consensus), and this test pins the
 * capability that DOES work end-to-end over real processes: join + catch-up.
 *
 * Bounded by timeouts; tears down all node processes + temp dirs.
 *
 * Gating + how to run: identical to the other node-backed tests — set
 * `BOTHO_E2E_NODE=1` and run via the node-backed runner. Requires
 * `cargo build --release -p botho` (or BOTHO_BIN=/path) and a built wasm
 * artifact (`pnpm --filter @botho/wasm-signer build:wasm`).
 */

import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { type WasmSigner } from '../src/index'
import {
  startMultiNodeNetwork,
  nodeHeight,
  nodePeerCount,
  type MultiNodeNetwork,
} from './multi-node-harness'

const env =
  (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {}

const here = dirname(fileURLToPath(import.meta.url))
const pkgDir = join(here, '..', 'pkg')
const wasmGlue = join(pkgDir, 'bth_wasm_signer.js')
const wasmBin = join(pkgDir, 'bth_wasm_signer_bg.wasm')
const wasmBuilt = existsSync(wasmGlue) && existsSync(wasmBin)

// This test ALWAYS needs to spawn its own local network, so it is gated solely
// on BOTHO_E2E_NODE. (The wasm artifact is used only to read the ring size that
// sizes N, keeping it consistent with the other node-backed tests.)
const RUN_LOCAL_NODE = env.BOTHO_E2E_NODE === '1'
const enabled = wasmBuilt && RUN_LOCAL_NODE
const maybe = enabled ? describe : describe.skip

interface WasmMod extends WasmSigner {
  default: (init: { module_or_path: BufferSource }) => Promise<unknown>
}

async function loadWasmNode(): Promise<WasmSigner> {
  const mod = (await import(/* @vite-ignore */ wasmGlue)) as unknown as WasmMod
  await mod.default({ module_or_path: readFileSync(wasmBin) })
  return {
    buildAndSign: (request) => mod.buildAndSign(request),
    scanOwnedOutputs: (request) => mod.scanOwnedOutputs(request),
    computeOwnedOutputKeyImages: (request) => mod.computeOwnedOutputKeyImages(request),
    ringSize: () => mod.ringSize(),
    minFee: () => mod.minFee(),
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

function makeRpc(url: string) {
  let id = 1
  return async function rpc<T>(method: string, params: Record<string, unknown>): Promise<T> {
    for (let attempt = 0; ; attempt++) {
      const res = await fetch(url, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', method, params, id: id++ }),
      })
      const json = (await res.json()) as { result?: T; error?: { message: string } }
      if (json.error) {
        if (json.error.message.includes('Rate limit') && attempt < 40) {
          await sleep(2000)
          continue
        }
        throw new Error(`${method}: ${json.error.message}`)
      }
      return json.result as T
    }
  }
}

/** Fetch a block's hash at `height` from a node, or null if it lacks it yet. */
async function blockHashAt(
  rpc: ReturnType<typeof makeRpc>,
  height: number,
): Promise<string | null> {
  try {
    const b = await rpc<{ hash: string; height: number }>('getBlockByHeight', { height })
    return b?.hash ?? null
  } catch {
    return null
  }
}

// Distinct valid BIP39 test vectors. Throwaway testnet keys, no value.
const MNEMONIC_A =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon ' +
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon ' +
  'abandon abandon abandon abandon abandon art'
const MNEMONIC_B = 'legal winner thank year wave sausage worth useful legal winner thank yellow'

maybe('node-backed: a fresh node joins a running local chain and catches up (#396)', () => {
  let network: MultiNodeNetwork | null = null
  let ringSize: number

  // Ports: keep clear of the other node-backed tests (17599/17798/...).
  const PORTS = {
    A: { gossip: 17820, rpc: 17821 },
    B: { gossip: 17822, rpc: 17823 },
  }

  beforeAll(async () => {
    const signer = await loadWasmNode()
    ringSize = signer.ringSize()
  }, 60_000)

  afterAll(async () => {
    if (network) await network.stop()
  })

  it('A mines to N > ringSize; a fresh follower B joins, connects, and syncs 0 -> N onto the same chain', async () => {
    // ----------------------------------------------------------------------
    // Step 1: start A — a solo minter (explicit 1-of-1 quorum, exactly like the
    // single-node harness) — and mine to N > ringSize so there is real history
    // and a decoy ring. B is NOT started yet.
    // ----------------------------------------------------------------------
    network = await startMultiNodeNetwork(
      [
        {
          name: 'A',
          mnemonic: MNEMONIC_A,
          gossipPort: PORTS.A.gossip,
          rpcPort: PORTS.A.rpc,
          bootstrapGossipPorts: [],
          mint: true,
          // Solo minter: explicit 1-of-1 quorum (the harness emits this when
          // minPeers === 0 — see multi-node-harness.ts), so A mines on its own
          // and B can later follow it.
          minPeers: 0,
        },
      ],
      {
        // Match the other node-backed tests' fast lottery eligibility so the
        // run stays bounded; A is a solo proposer+validator, so the draw is
        // consensus-deterministic.
        lottery: { minUtxoAge: 1, minUtxoValue: 1 },
      },
    )

    const nodeA = network.get('A')
    const rpcA = makeRpc(nodeA.rpcUrl)

    const N = ringSize + 4
    {
      const deadline = Date.now() + 180_000
      let h = 0
      while (Date.now() < deadline) {
        h = await nodeHeight(nodeA.rpcUrl)
        if (h >= N) break
        await sleep(1000)
      }
      expect(
        h,
        `A only mined ${h}/${N} blocks.\nA log:\n${nodeA
          .readLog()
          .split('\n')
          .slice(-25)
          .join('\n')}`,
      ).toBeGreaterThanOrEqual(N)
    }
    const heightN = await nodeHeight(nodeA.rpcUrl)
    expect(heightN).toBeGreaterThan(ringSize)
    // eslint-disable-next-line no-console
    console.log(`[#396] A (solo minter) reached height N=${heightN} (ringSize=${ringSize})`)

    // ----------------------------------------------------------------------
    // Step 2: start B — a FRESH FOLLOWER (empty ledger, minting disabled) —
    // bootstrapping to A. Assert it connects (peerCount >= 1).
    // ----------------------------------------------------------------------
    const bNetwork = await startMultiNodeNetwork(
      [
        {
          name: 'B',
          mnemonic: MNEMONIC_B,
          gossipPort: PORTS.B.gossip,
          rpcPort: PORTS.B.rpc,
          bootstrapGossipPorts: [PORTS.A.gossip],
          mint: false,
          minPeers: 1,
        },
      ],
      { lottery: { minUtxoAge: 1, minUtxoValue: 1 } },
    )
    // Fold B into the same network object so teardown kills both.
    const nodeB = bNetwork.get('B')
    network.nodes.push(nodeB)
    const origStop = network.stop
    const bStop = bNetwork.stop
    network.stop = async () => {
      await bStop()
      await origStop()
    }
    const rpcB = makeRpc(nodeB.rpcUrl)

    // B starts near genesis (it has its own empty ledger).
    const bStart = await nodeHeight(nodeB.rpcUrl)
    expect(bStart, 'B should start near genesis').toBeLessThan(heightN)

    {
      const deadline = Date.now() + 60_000
      let connected = false
      while (Date.now() < deadline) {
        if ((await nodePeerCount(nodeB.rpcUrl)) >= 1) {
          connected = true
          break
        }
        await sleep(1000)
      }
      expect(
        connected,
        `B did not connect to the running node A.\nB log:\n${nodeB
          .readLog()
          .split('\n')
          .slice(-30)
          .join('\n')}`,
      ).toBe(true)
    }
    // eslint-disable-next-line no-console
    console.log(`[#396] B connected to A (peerCount=${await nodePeerCount(nodeB.rpcUrl)})`)

    // ----------------------------------------------------------------------
    // Step 3: B must catch up from 0 toward N via the real #376 catch-up sync.
    // A keeps mining (its tip moves), so require B to reach at least the height
    // A had when B joined — proving B backfilled history rather than getting
    // stuck at genesis (the pre-#376 failure mode).
    // ----------------------------------------------------------------------
    {
      const deadline = Date.now() + 180_000
      let hb = 0
      while (Date.now() < deadline) {
        hb = await nodeHeight(nodeB.rpcUrl)
        if (hb >= heightN) break
        await sleep(1000)
      }
      expect(
        hb,
        `B only caught up to ${hb}/${heightN} (stuck — the pre-#376 failure).\n` +
          `B log:\n${nodeB.readLog().split('\n').slice(-40).join('\n')}`,
      ).toBeGreaterThanOrEqual(heightN)
    }
    const bHeight = await nodeHeight(nodeB.rpcUrl)
    // eslint-disable-next-line no-console
    console.log(`[#396] B caught up 0 -> ${bHeight} (joined at N=${heightN}) via the real sync`)

    // ----------------------------------------------------------------------
    // Step 4: B converged on the SAME chain as A — not a private fork. Compare
    // block hashes at several heights spanning the caught-up range; every one
    // must match A's. This proves B applied A's actual blocks (real sync), not
    // that it merely reached a similar height.
    // ----------------------------------------------------------------------
    const checkHeights = Array.from(
      new Set([1, Math.floor(heightN / 2), heightN - 1, heightN]),
    ).filter((h) => h >= 1 && h <= heightN)

    for (const h of checkHeights) {
      const hashA = await blockHashAt(rpcA, h)
      // B may still be a block or two behind A's *current* tip, but it must
      // have the block at height <= heightN; poll briefly for it.
      let hashB: string | null = null
      const deadline = Date.now() + 30_000
      while (Date.now() < deadline) {
        hashB = await blockHashAt(rpcB, h)
        if (hashB) break
        await sleep(1000)
      }
      expect(hashA, `A should have block ${h}`).not.toBeNull()
      expect(hashB, `B should have synced block ${h}`).not.toBeNull()
      expect(hashB, `B's block ${h} hash must match A's (same chain, real sync)`).toBe(hashA)
    }
    // eslint-disable-next-line no-console
    console.log(
      `[#396] B converged on A's chain: identical block hashes at heights [${checkHeights.join(
        ', ',
      )}]`,
    )

    // Sanity: the running node never paused on its own (the chain is live).
    expect(await nodeHeight(nodeA.rpcUrl)).toBeGreaterThanOrEqual(heightN)
  }, 600_000)
})
