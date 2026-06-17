/**
 * Node-backed "fresh node JOINS a running local chain, PARTICIPATES in
 * consensus, and MINES an accepted block" test (issue #396, flavor A —
 * hermetic local), exercised against REAL `botho run` processes peering over
 * real libp2p loopback.
 *
 * This is the full "run your own node and participate" demonstration, end to
 * end and over real processes (no `TestNetwork` sim, no live betanet):
 *
 *   1. Two real minters (A + B) peer over loopback and form a quorum, mining a
 *      shared chain to a height N > ring size (real history + decoy outputs).
 *      They converge via real SCP (the #406 wire-decode fix lets their SCP
 *      ballots actually deserialize; the #404/#408 dynamic quorum reconfigure
 *      keeps their quorum set tracking live membership).
 *   2. A 3rd FRESH node C (empty ledger, minting enabled) bootstraps to the
 *      running pair, CONNECTS (peerCount >= 1), and CATCHES UP 0 -> N via the
 *      real #376 catch-up sync — converging on the SAME block hashes as A/B.
 *   3. Once synced, C PARTICIPATES: the quorum reconfigures to include C, and C
 *      MINES a block the established nodes ACCEPT — the network tip advances and
 *      ALL THREE nodes converge on the SAME extended tip (identical hashes).
 *   4. C EARNED COINS: its own wallet balance (coinbase paid to C's address)
 *      becomes non-zero and on-chain — proving a freshly-joined node's coinbase
 *      reward is accepted by consensus.
 *
 * Why this is distinct from the other node-backed tests:
 *   - `e2e_consensus_integration` starts all nodes at genesis together; no node
 *     ever JOINS an already-running chain mid-flight.
 *   - `join-consensus-live-node.test.ts` (#398) proves only the JOIN + CATCH-UP
 *     half (one solo minter + one passive follower); the follower never mines.
 *   - the in-process `TestNetwork` sim uses DashMap message-passing — no real
 *     libp2p bootstrap, no #376 sync, no run-loop consensus.
 *
 * Quorum config used (and why)
 * ----------------------------
 * All three nodes use `recommended` mode with `min_peers = 1`. The node's
 * consensus quorum set tracks LIVE membership (`rebuild_scp_quorum_set` in
 * `botho/src/commands/run.rs`), and the BFT threshold is
 * `n - floor((n-1)/3)` over `n = connected_peers + 1`
 * (`QuorumConfig::effective_threshold`):
 *   - while A + B are the only members:  n = 2 -> threshold 2 (2-of-2),
 *   - once C has joined and is counted:  n = 3 -> threshold 3 (3-of-3).
 * `recommended` is used (rather than an `explicit` static peer list) precisely
 * because the joiner's peer id is not known ahead of time; recommended
 * auto-trusts connected peers, and the #404 dynamic reconfigure folds C into
 * the quorum the moment it connects, so C's proposed block is accepted by the
 * whole set. (A 3-node recommended quorum is unanimous — 3-of-3 — which is the
 * documented tradeoff; once C finishes catch-up all three participate every
 * slot, so the chain keeps advancing and C's blocks are accepted.)
 *
 * Distinguishing "mined by the joiner": the coinbase of each block pays the
 * minter's own stealth address, and each node has a DISTINCT BIP39 mnemonic, so
 * C's `wallet_getBalance` is non-zero ONLY if a block whose coinbase pays C was
 * accepted on-chain. We additionally require the network tip to advance beyond
 * N (so new blocks were produced after C joined) with all nodes converged.
 *
 * Bounded by timeouts; tears down all node processes + temp dirs.
 *
 * Gating + how to run: identical to the other node-backed tests — set
 * `BOTHO_E2E_NODE=1` and run via the node-backed runner. Requires
 * `cargo build --release --bin botho` (or BOTHO_BIN=/path) and a built wasm
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
  type RunningNode,
} from './multi-node-harness'

const env =
  (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {}

const here = dirname(fileURLToPath(import.meta.url))
const pkgDir = join(here, '..', 'pkg')
const wasmGlue = join(pkgDir, 'bth_wasm_signer.js')
const wasmBin = join(pkgDir, 'bth_wasm_signer_bg.wasm')
const wasmBuilt = existsSync(wasmGlue) && existsSync(wasmBin)

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

/** Read a node's tip (height + hash) via node_getStatus. */
async function nodeTip(rpc: ReturnType<typeof makeRpc>): Promise<{ height: number; hash: string }> {
  const s = await rpc<{ chainHeight: number; tipHash: string }>('node_getStatus', {})
  return { height: s.chainHeight, hash: s.tipHash }
}

/** Read a node's own wallet confirmed balance (coinbase paid to its address). */
async function nodeBalance(rpc: ReturnType<typeof makeRpc>): Promise<bigint> {
  const b = await rpc<{ confirmed: number | string }>('wallet_getBalance', {})
  return BigInt(b.confirmed)
}

function tail(node: RunningNode, n = 30): string {
  return node.readLog().split('\n').slice(-n).join('\n')
}

// Distinct valid BIP39 test vectors. Throwaway testnet keys, no value. Each
// node's coinbase pays its own address, so a distinct mnemonic per node makes
// "who mined this block" attributable via that node's wallet balance.
const MNEMONIC_A =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon ' +
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon ' +
  'abandon abandon abandon abandon abandon art'
const MNEMONIC_B = 'legal winner thank year wave sausage worth useful legal winner thank yellow'
const MNEMONIC_C =
  'letter advice cage absurd amount doctor acoustic avoid letter advice cage above'

maybe(
  'node-backed: a fresh node joins a running local chain, participates in consensus, and mines an accepted block (#396)',
  () => {
    let network: MultiNodeNetwork | null = null
    let ringSize: number

    // Ports: keep clear of the other node-backed tests (17599/17798/17820...).
    const PORTS = {
      A: { gossip: 17840, rpc: 17841 },
      B: { gossip: 17842, rpc: 17843 },
      C: { gossip: 17844, rpc: 17845 },
    }

    beforeAll(async () => {
      const signer = await loadWasmNode()
      ringSize = signer.ringSize()
    }, 60_000)

    afterAll(async () => {
      if (network) await network.stop()
    })

    it('A+B mine to N; fresh C joins, catches up 0 -> N, then mines a block the whole network accepts + earns coinbase', async () => {
      // --------------------------------------------------------------------
      // Step 1: start A + B — two real minters in a `recommended` quorum
      // (min_peers = 1). They peer over loopback, form a 2-of-2 quorum, and
      // mine a SHARED chain to N > ringSize (real history + a decoy ring).
      // --------------------------------------------------------------------
      network = await startMultiNodeNetwork(
        [
          {
            name: 'A',
            mnemonic: MNEMONIC_A,
            gossipPort: PORTS.A.gossip,
            rpcPort: PORTS.A.rpc,
            bootstrapGossipPorts: [PORTS.B.gossip],
            mint: true,
            minPeers: 1,
          },
          {
            name: 'B',
            mnemonic: MNEMONIC_B,
            gossipPort: PORTS.B.gossip,
            rpcPort: PORTS.B.rpc,
            bootstrapGossipPorts: [PORTS.A.gossip],
            mint: true,
            minPeers: 1,
          },
        ],
        // Fast lottery eligibility keeps the run bounded (same test-only env
        // overrides as the other node-backed tests).
        { lottery: { minUtxoAge: 1, minUtxoValue: 1 } },
      )

      const nodeA = network.get('A')
      const nodeB = network.get('B')
      const rpcA = makeRpc(nodeA.rpcUrl)
      const rpcB = makeRpc(nodeB.rpcUrl)

      // A + B must peer before they can reach their 2-of-2 quorum and mine.
      {
        const deadline = Date.now() + 60_000
        let peered = false
        while (Date.now() < deadline) {
          if ((await nodePeerCount(nodeA.rpcUrl)) >= 1 && (await nodePeerCount(nodeB.rpcUrl)) >= 1) {
            peered = true
            break
          }
          await sleep(1000)
        }
        expect(
          peered,
          `A and B never peered.\nA log:\n${tail(nodeA)}\n\nB log:\n${tail(nodeB)}`,
        ).toBe(true)
      }

      const N = ringSize + 4
      {
        const deadline = Date.now() + 240_000
        let h = 0
        while (Date.now() < deadline) {
          h = Math.min(await nodeHeight(nodeA.rpcUrl), await nodeHeight(nodeB.rpcUrl))
          if (h >= N) break
          await sleep(1000)
        }
        expect(
          h,
          `A+B only mined a shared height of ${h}/${N}.\n` +
            `A log:\n${tail(nodeA, 25)}\n\nB log:\n${tail(nodeB, 25)}`,
        ).toBeGreaterThanOrEqual(N)
      }
      const heightN = Math.min(await nodeHeight(nodeA.rpcUrl), await nodeHeight(nodeB.rpcUrl))
      expect(heightN).toBeGreaterThan(ringSize)

      // A + B agree on the chain at N (real SCP agreement, not divergent forks).
      {
        const ha = await blockHashAt(rpcA, heightN)
        const hb = await blockHashAt(rpcB, heightN)
        expect(ha, 'A should have block N').not.toBeNull()
        expect(hb, "B's block N must match A's (A+B agree via SCP)").toBe(ha)
      }
      // eslint-disable-next-line no-console
      console.log(`[#396] A+B (2-of-2) reached a shared height N=${heightN} (ringSize=${ringSize})`)

      // --------------------------------------------------------------------
      // Step 2: start C — a FRESH minter (empty ledger) — bootstrapping to A
      // and B. Assert it connects (peerCount >= 1).
      // --------------------------------------------------------------------
      const cNetwork = await startMultiNodeNetwork(
        [
          {
            name: 'C',
            mnemonic: MNEMONIC_C,
            gossipPort: PORTS.C.gossip,
            rpcPort: PORTS.C.rpc,
            bootstrapGossipPorts: [PORTS.A.gossip, PORTS.B.gossip],
            mint: true,
            minPeers: 1,
          },
        ],
        { lottery: { minUtxoAge: 1, minUtxoValue: 1 } },
      )
      // Fold C into the same network object so teardown kills all three.
      const nodeC = cNetwork.get('C')
      network.nodes.push(nodeC)
      const origStop = network.stop
      const cStop = cNetwork.stop
      network.stop = async () => {
        await cStop()
        await origStop()
      }
      const rpcC = makeRpc(nodeC.rpcUrl)

      // C starts near genesis (its own empty ledger).
      const cStart = await nodeHeight(nodeC.rpcUrl)
      expect(cStart, 'C should start near genesis').toBeLessThan(heightN)

      {
        const deadline = Date.now() + 60_000
        let connected = false
        while (Date.now() < deadline) {
          if ((await nodePeerCount(nodeC.rpcUrl)) >= 1) {
            connected = true
            break
          }
          await sleep(1000)
        }
        expect(connected, `C did not connect to the running pair.\nC log:\n${tail(nodeC)}`).toBe(
          true,
        )
      }
      // eslint-disable-next-line no-console
      console.log(`[#396] C connected (peerCount=${await nodePeerCount(nodeC.rpcUrl)})`)

      // --------------------------------------------------------------------
      // Step 3: C catches up from 0 to at least N via the real #376 sync.
      // --------------------------------------------------------------------
      {
        const deadline = Date.now() + 240_000
        let hc = 0
        while (Date.now() < deadline) {
          hc = await nodeHeight(nodeC.rpcUrl)
          if (hc >= heightN) break
          await sleep(1000)
        }
        expect(
          hc,
          `C only caught up to ${hc}/${heightN} (stuck — the pre-#376 failure).\n` +
            `C log:\n${tail(nodeC, 40)}`,
        ).toBeGreaterThanOrEqual(heightN)
      }
      // C converged on the SAME chain as A at several heights (not a private fork).
      {
        const checkHeights = Array.from(
          new Set([1, Math.floor(heightN / 2), heightN]),
        ).filter((h) => h >= 1 && h <= heightN)
        for (const h of checkHeights) {
          const hashA = await blockHashAt(rpcA, h)
          let hashC: string | null = null
          const deadline = Date.now() + 30_000
          while (Date.now() < deadline) {
            hashC = await blockHashAt(rpcC, h)
            if (hashC) break
            await sleep(1000)
          }
          expect(hashC, `C's block ${h} hash must match A's (same chain, real sync)`).toBe(hashA)
        }
      }
      // eslint-disable-next-line no-console
      console.log(`[#396] C caught up 0 -> >=${heightN} via the real sync, on A's exact chain`)

      // --------------------------------------------------------------------
      // Step 4: C PARTICIPATES + MINES an accepted block. Now that C is part
      // of the live quorum, require:
      //   (a) the network tip advances beyond N (new blocks produced after C
      //       joined), and ALL THREE nodes converge on the SAME extended tip
      //       (identical hash) — i.e. C's blocks are accepted by A + B, not a
      //       private fork; and
      //   (b) C's own wallet balance becomes non-zero — its coinbase reward is
      //       on-chain and owned by it (distinct mnemonic per node, so balance
      //       is attributable to C having minted an accepted block).
      // --------------------------------------------------------------------
      const targetTip = heightN + 3
      {
        const deadline = Date.now() + 300_000
        let converged = false
        while (Date.now() < deadline) {
          const [ta, tb, tc] = await Promise.all([
            nodeTip(rpcA),
            nodeTip(rpcB),
            nodeTip(rpcC),
          ])
          if (
            ta.height >= targetTip &&
            ta.hash === tb.hash &&
            tb.hash === tc.hash &&
            ta.height === tb.height &&
            tb.height === tc.height
          ) {
            converged = true
            break
          }
          await sleep(1000)
        }
        const [ta, tb, tc] = await Promise.all([nodeTip(rpcA), nodeTip(rpcB), nodeTip(rpcC)])
        expect(
          converged,
          `Network did not advance past N=${heightN} with all three converged on one tip.\n` +
            `A=${ta.height}/${ta.hash.slice(0, 12)} ` +
            `B=${tb.height}/${tb.hash.slice(0, 12)} ` +
            `C=${tc.height}/${tc.hash.slice(0, 12)}\n` +
            `C log:\n${tail(nodeC, 40)}`,
        ).toBe(true)
      }
      const finalTip = await nodeTip(rpcA)
      // eslint-disable-next-line no-console
      console.log(
        `[#396] network advanced to a single converged tip at height ${finalTip.height} ` +
          `(all of A/B/C agree on ${finalTip.hash.slice(0, 16)}...)`,
      )

      // (b) C earned coins: its coinbase reward is on-chain and owned by it.
      {
        const deadline = Date.now() + 240_000
        let bal = 0n
        while (Date.now() < deadline) {
          bal = await nodeBalance(rpcC)
          if (bal > 0n) break
          await sleep(2000)
        }
        expect(
          bal,
          `C never received a coinbase reward — it did not mine an accepted block.\n` +
            `C log:\n${tail(nodeC, 40)}`,
        ).toBeGreaterThan(0n)
        // eslint-disable-next-line no-console
        console.log(`[#396] C earned an on-chain coinbase reward: confirmed balance = ${bal}`)
      }

      // Sanity: A and B are still at the converged tip (the chain is live and shared).
      const tA = await nodeTip(rpcA)
      const tB = await nodeTip(rpcB)
      expect(tA.hash).toBe(tB.hash)
      expect(tA.height).toBeGreaterThanOrEqual(targetTip)
    }, 900_000)
  },
)
