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
 * All three nodes use `recommended` mode with `min_peers = 1` and the DEFAULT
 * `fault_model = crash` (#432). The node's consensus quorum set tracks LIVE
 * membership (`rebuild_scp_quorum_set` in `botho/src/commands/run.rs`), and the
 * crash-fault threshold is the 2f+1 simple majority `floor(n/2) + 1` over
 * `n = connected_peers + 1` (`QuorumConfig::effective_threshold`):
 *   - while A + B are the only members:  n = 2 -> threshold 2 (2-of-2),
 *   - once C has joined and is counted:  n = 3 -> threshold 2 (2-of-3).
 * `recommended` is used (rather than an `explicit` static peer list) precisely
 * because the joiner's peer id is not known ahead of time; recommended
 * auto-trusts connected peers, and the #404 dynamic reconfigure folds C into
 * the quorum the moment it connects, so C's proposed block is accepted by the
 * whole set. The crash fault model (#432) is the key fix that unblocks this
 * soak: at n=3 the quorum is 2-of-3 (NOT the old 3-of-3 unanimous set), so A+B
 * can keep externalizing while C is still catching up — a behind/lagging node
 * no longer acts like a fault that stalls the whole network — and once C is
 * synced its freshly-mined blocks are accepted by the 2-of-3 majority.
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

// STATUS after #419: the #417 SAFETY FORK (Finding 1) is FIXED and the JOIN +
// CATCH-UP + SCP-slot fast-forward (Finding 3) work; the full
// "joiner mines and all three converge several blocks past N" END STATE is
// still blocked by a SEPARATE, pre-existing liveness defect (SCP-slot /
// block-height DRIFT) that is out of scope for #419.
//
// What #419 fixed and PROVED (real 2-node `botho run` over loopback):
//   - Finding 1 (the fork): the SCP `validity_fn` is now TIP-AGNOSTIC, so two
//     minters' competing-but-valid coinbases are no longer dropped, the quorum
//     never partitions, and federated voting + the deterministic combiner
//     converge on ONE value. Verified: a 2-of-2 net mined to height 35 with
//     IDENTICAL block hashes at EVERY height including the old fork zone
//     (22/23/24) — no divergence — via real SCP (`Slot externalized!` in
//     non-solo mode). The fork that previously appeared by ~height 22 is gone.
//   - Finding 3: a joiner that block-syncs to height H fast-forwards its SCP
//     slot (verified in the 3-node run: "Fast-forwarding SCP slot ... Finding 3"
//     advanced C from slot 1 to height+1), so it stops discarding live messages
//     as "future slots". After C joined, the 3-of-3 net DID advance and all
//     three converged on identical tips.
//
// The 3rd-node CATCH-UP SYNC blocker (#423) is FIXED: a fresh joiner at height
// 0 against a small tip now triggers the historical 0 -> N download instead of
// jumping straight to Synced. This is proven end-to-end over real loopback
// processes by `join-consensus-live-node.test.ts` (B catches up 0 -> N onto A's
// exact chain, including small N), which is enabled and green.
//
// Why the full end-state is still gated after reverting #422:
//   #422 ("keep SCP slot aligned with block height": a proposal-side height
//   filter + a gated backward slot-realign backstop) was REVERTED because it
//   regressed two-minter liveness — it deterministically WEDGED the A+B pair
//   (the height filter dropped both minters' competing-but-valid coinbases in a
//   race, and the realign-on-reject backstop thrashed the slot), so the shared
//   chain stalled (#424). A bisect confirmed: clean at #420, stalls at #422.
//   Reverting #422 restores #420's clean two-minter liveness (verified: a 2-of-2
//   net mines past the old stall with identical tips), which is what unblocks
//   the A+B half of this soak.
//
//   The trade-off: reverting #422 RE-OPENS the SCP-slot/height DRIFT it was
//   solving (#421) — the `current_slot_index` can drift ahead of the ledger
//   height on the established minters when a stale/duplicate-height value is
//   externalized then rejected at apply, so a freshly-synced joiner and the
//   drifted established nodes discard each other's messages. That drift only
//   bites the 3-of-3 joiner step here, and must be re-approached WITHOUT the
//   wedging realign loop. Re-enable this soak (drop the `&& false`) once the
//   drift (#421) is re-fixed in a way that does not regress two-minter liveness.
//
//   [pre-#422 rationale, restored by the revert:]
//   The SCP `current_slot_index` DRIFTS ahead of the block height on the
//   established minters (e.g. SCP slot ~36 while the ledger is at height 26),
//   because a stale/duplicate-height minting value can still be externalized
//   and its block rejected at apply-time without rewinding the SCP slot. The
//   joiner fast-forwards to `height + 1` (the documented `slot == height + 1`
//   invariant) but the established nodes are on a higher, drifted slot, so the
//   joiner's and the network's SCP slots no longer line up and they discard
//   each other's messages. Closing that requires re-aligning SCP slot index
//   with block height (a deeper protocol change tracked separately), so this
//   end-to-end soak stays gated until then. Re-enable (drop `&& false`) once the
//   SCP-slot/height drift is fixed.
// #428 status (participation/proposer gate — block minting while connected
// peers < min_peers): the gate removes the pre-quorum solo-latch fork, and
// this soak now gets MUCH further than before. Verified end-to-end over real
// loopback `botho run` processes (BOTHO_E2E_NODE=1, release binary):
//   - A+B (2-of-2) reliably reach the shared target height N=ringSize+4 with
//     IDENTICAL tip hashes (real SCP agreement, no divergent solo chain), and
//   - C joins, connects (peerCount=2), and catches up 0 -> >=N onto A's exact
//     chain (the #423 sync), so all three sit on ONE tip at height N.
// i.e. acceptance #5's "A+B phase reaches target reliably" + the catch-up are
// MET. What remains: the FINAL step — the 3-node network advancing PAST N with
// C's freshly-mined block accepted by all three — stalls. That is the
// pre-existing SCP-slot/height DRIFT (#421), re-opened by the #422 revert and
// independent of the proposer gate: once the established A+B minters' SCP slot
// drifts ahead of the ledger height, the freshly-synced joiner C and the
// drifted minters discard each other's messages, so the 3-of-3 step jams.
//
// Per #428's STOP guardrail we do NOT force this green: the soak stays gated on
// the full 3-node end state until #421 (SCP-slot/height drift) is re-fixed in a
// way that does not regress two-minter liveness. Re-enable (drop `&& false`)
// then. The proposer gate's own acceptance (no pre-quorum solo block; staggered
// 2-node no fork; A+B target + C catch-up) is proven separately (#428).
// #421 status (SCP-slot/height drift — hybrid A1 + C): the DRIFT itself is now
// FIXED. Verified end-to-end over real loopback `botho run` processes
// (BOTHO_E2E_NODE=1, release binary):
//   - Steps 1-3 PASS: A+B (2-of-2) reach the shared target N=ringSize+4 with
//     IDENTICAL tips (no fork, no wedge), C joins, connects, and catches up
//     0 -> N onto A's EXACT chain (all three sit on one tip at height N).
//   - Option C (this PR) PROVABLY works: a fresh joiner C that booted at slot 1
//     while the established minters had DRIFTED their SCP slot far ahead
//     (e.g. slot 29 at ledger height 24) ANCHORS FORWARD to the leaders' live
//     slot ("Anchoring SCP slot forward ... to_slot=29") instead of discarding
//     their messages as future slots. On baseline (pre-fix) C stays stuck at
//     slot 1 forever; with this PR all three nodes align on the same slot index.
//     i.e. the #421 drift convergence symptom is closed.
//
// #432 status (quorum.fault_model = crash 2f+1): the n=3 JOIN-BOUNDARY blocker
// is FIXED. The FINAL "C mines a block accepted past N" end-state previously
// stalled because the n=3 recommended quorum was a 3-of-3 UNANIMOUS (n-of-n)
// set: a joiner C still catching up acted like a fault (the SCP cluster-health
// concern from #427). #432 defaults `quorum.fault_model = crash`, so the n=3
// recommended threshold is the 2f+1 simple majority `floor(3/2)+1 = 2` (2-of-3)
// instead of 3-of-3. With 2-of-3, this CAPSTONE PROVABLY PASSES over real
// loopback `botho run` processes (BOTHO_E2E_NODE=1, release binary): verified
// end-to-end across multiple full runs —
//   - A+B (2-of-2) reach the shared target N=ringSize+4 with IDENTICAL tips,
//   - C joins, connects, catches up 0 -> N onto A's EXACT chain (#423),
//   - the 3-node net ADVANCES PAST N to a single converged tip (e.g. height 27
//     with all of A/B/C on IDENTICAL hashes), and
//   - C MINES an accepted block and EARNS an on-chain coinbase
//     (confirmed balance 50000000000000) — proving a freshly-joined node's
//     block is accepted by the 2-of-3 majority.
// #420 no-fork safety is preserved (any two 2-of-3 subsets intersect; block-apply
// tip checks unchanged); the n=2 threshold is unchanged (crash n=2 is still
// 2-of-2).
//
// #433 status (intermittent n=2 solo-mode fork): FIXED. The n=2 blocker that
// kept this soak gated is gone. It was a two-minter (n=2) safety defect: in a
// fraction of runs the two established minters A and B BOTH fell into SCP
// solo-mode ("Advanced to next slot (solo mode)" / "Solo mode: directly
// externalizing values") despite each reporting "Quorum satisfied: 2-of-2", and
// mined DIVERGENT solo chains that never reconciled (an A/B-only run with no C
// forked, disagreeing as low as height 20 and running to different tips at
// 111/115).
//
// ROOT CAUSE (#433): peers that connect during the pre-consensus "wait for
// peers" startup loop in `commands::run` had their `PeerDiscovered` events
// CONSUMED by that loop, so the main event loop never called
// `reconfigure_quorum` for them. The consensus service was therefore seeded
// from the STATIC config (a 1-of-1 solo quorum for a recommended/min_peers=1
// node). The participation gate (#429) then opened (connected >= min_peers)
// while the quorum was still 1-of-1, so the next tick took the SOLO
// direct-externalize path and the node mined a divergent solo chain forever (no
// further PeerDiscovered ever arrived, so the quorum was never reconfigured out
// of solo). Two such nodes forked.
//
// FIX (#433): (1) seed the INITIAL SCP quorum from the peers already connected
// at startup (`rebuild_scp_quorum_set`) so a node that already peered boots in
// federated SCP (e.g. 2-of-2), never solo; and (2) a defense-in-depth
// transitional-solo guard in the consensus service that withholds the solo
// direct-externalize whenever a peer is connected but the quorum is still 1-of-1
// (`min_peers >= 1 && connected_peers >= 1 && is_solo_mode()`). A genuine lone
// node (min_peers == 0) is unaffected and still mints solo. Verified empirically
// over real loopback `botho run` processes (release binary): the A+B 2-minter
// scenario now runs FEDERATED SCP and converges on identical tips + identical
// historical hashes across 10/10 runs with ZERO solo-mode externalizes (the
// pre-fix binary forked in ~2 of 8 runs with the identical harness). In the
// full 3-node capstone below, steps 1-4(a) now pass on EVERY run: A+B converge
// (no n=2 fork), C catches up onto A's exact chain, and ALL THREE converge past
// N onto one identical tip — with ZERO solo-mode externalizes.
//
// WHY THIS SOAK STAYS GATED (per the STOP guardrail — a SEPARATE n=3 issue, NOT
// #433): the FINAL step — node C winning its OWN coinbase (non-zero balance)
// within the window — is still INTERMITTENT (~2 of 3 runs pass). The
// convergence always succeeds, but C frequently loses the PoW/SCP race for a
// block of its own: the established A+B drift their SCP slot ahead of C, so C's
// proposals land on an earlier slot and get discarded as "future slots"
// (node_impl), and C's block is rarely the one externalized. That is the
// pre-existing n=3 SCP-slot/height DRIFT join-boundary liveness concern
// (#421/#427-family) — independent of the #433 n=2 solo fork that this PR fixes.
// Per the guardrail we do NOT force this green by masking that flake. Re-enable
// (drop `&& false`) once the n=3 join-boundary drift is closed so a freshly
// joined minter reliably gets a block accepted. The #433 n=2 fix itself is
// complete, unit-tested, and empirically proven above.
const enabled = wasmBuilt && RUN_LOCAL_NODE && false
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
      // heightN is the LIVE tip the instant both crossed N, so the block at that
      // exact height may still be settling for a moment (the latest slot can be
      // momentarily in flight before SCP externalizes one value). Poll for
      // agreement within a bounded window: a genuine persistent fork never
      // converges and still fails, but a transient tip race resolves quickly.
      {
        const deadline = Date.now() + 60_000
        let ha: string | null = null
        let hb: string | null = null
        while (Date.now() < deadline) {
          ha = await blockHashAt(rpcA, heightN)
          hb = await blockHashAt(rpcB, heightN)
          if (ha && hb && ha === hb) break
          await sleep(1000)
        }
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
