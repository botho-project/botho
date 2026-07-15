/**
 * Node-backed THREE-USER exchange-until-lottery-payout test (issue #394).
 *
 * Extends the two-user exchange (#390) into the redistribution lottery — Botho's
 * core anti-hoarding economics — exercised end-to-end through the real wallet
 * path against ONE local minting node:
 *
 *   fee generation -> 80% lottery pool / 20% burn -> per-block lottery draw ->
 *   payout to a winning UTXO's owner.
 *
 * Flow:
 *   1. Three wallets A, B, C. A == the node's minting wallet (coinbase-funded);
 *      B and C are externally derived. A distributes funds to B and C, SPLIT
 *      into several UTXOs each so A/B/C together hold a meaningful share of the
 *      eligible lottery-candidate set (raising their per-draw win probability).
 *   2. Exchange loop A->B->C->A... Each transfer pays a fee; 80% of fees feed the
 *      lottery pool. Each transfer is mined (the harness node mints and applies
 *      the lottery per block). Balances/spendable use the #392 spent-filtering.
 *   3. After each mined block, scan A/B/C for a NEW owned output that is a
 *      lottery payout (an output they own that did NOT come from a tx they sent,
 *      cross-checked against the block's lottery outputs surfaced by
 *      `chain_getOutputs`). Continue until ONE of A/B/C wins, bounded by a
 *      max-rounds cap so the test can't hang.
 *   4. Assert: the winner is one of A/B/C; the payout output is owned by them;
 *      the payout is > 0 and <= one block reward (the anti-grinding cap); the
 *      block carrying the payout mined successfully (so consensus `verify_drawing`
 *      passed); and fee/pool/burn accounting is consistent over the run
 *      (~20% of fees burned; payouts drawn from the pool).
 *
 * Lottery eligibility tuning. The production draw requires UTXOs to be 720
 * blocks old (`LotteryDrawConfig::default`), which a short test cannot reach. The
 * harness therefore starts the node with the test-only overrides
 * `BOTHO_LOTTERY_MIN_UTXO_AGE` / `BOTHO_LOTTERY_MIN_UTXO_VALUE` (see
 * `botho/src/consensus/lottery.rs::draw_config_from_env`), so freshly created
 * UTXOs become eligible within a couple of blocks. Both the block proposer and
 * the validator in the solo node read the same env, so the draw stays
 * consensus-deterministic (the block still has to pass `verify_drawing` to be
 * accepted, which the assertions confirm).
 *
 * Gating + how to run: identical to the other node-backed tests.
 *
 *   BOTHO_E2E_NODE=1 node packages/wasm-signer/test/run-node-backed.mjs
 *
 * Requires `cargo build --release -p botho` first (or BOTHO_BIN=/path) and a
 * built wasm artifact (`pnpm --filter @botho/wasm-signer build:wasm`).
 */

import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { deriveKeypairs, deriveDefaultSubaddressPublicKeys } from '@botho/core'
import {
  buildSendTransaction,
  spendableBalance,
  setSigner,
  resetSigner,
  type ChainOutput,
  type KeyImageSpentStatus,
  type SendRpc,
  type WasmSigner,
} from '../src/index'
import { startNodeBackedHarness, type NodeHarness } from './node-harness'

const env =
  (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {}

const here = dirname(fileURLToPath(import.meta.url))
const pkgDir = join(here, '..', 'pkg')
const wasmGlue = join(pkgDir, 'bth_wasm_signer.js')
const wasmBin = join(pkgDir, 'bth_wasm_signer_bg.wasm')
const wasmBuilt = existsSync(wasmGlue) && existsSync(wasmBin)

const RUN_LOCAL_NODE = env.BOTHO_E2E_NODE === '1'
const PRE_RUNNING_RPC = env.BOTHO_LIVE_RPC
const PRE_RUNNING_MNEMONIC = env.BOTHO_LIVE_MNEMONIC

const enabled = wasmBuilt && (RUN_LOCAL_NODE || (PRE_RUNNING_RPC && PRE_RUNNING_MNEMONIC))
const maybe = enabled ? describe : describe.skip

const toHex = (b: Uint8Array) =>
  Array.from(b)
    .map((x) => x.toString(16).padStart(2, '0'))
    .join('')

function leHexToBigInt(hex: string): bigint {
  let result = 0n
  for (let i = hex.length - 2; i >= 0; i -= 2) {
    result = (result << 8n) | BigInt(parseInt(hex.slice(i, i + 2), 16))
  }
  return result
}

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

/**
 * JSON-RPC client with rate-limit backoff. The node caps unauthenticated
 * callers at 100 req/min (sliding window); this test scans the whole chain many
 * times (via `buildSendTransaction` / `spendableBalance` / detection scans), so
 * it transparently retries on "Rate limit exceeded" after a short wait until the
 * window frees up, rather than failing. All other RPC errors propagate.
 */
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
          // Sliding 60s window; wait for it to drain, then retry.
          await sleep(2000)
          continue
        }
        throw new Error(`${method}: ${json.error.message}`)
      }
      return json.result as T
    }
  }
}

interface RawOutput {
  txHash?: string
  outputIndex?: number
  targetKey: string
  publicKey: string
  amountCommitment: string
  /** True for lottery-payout outputs surfaced by `chain_getOutputs` (#394). */
  lottery?: boolean
}

/** Every output `[0, height]` as ChainOutputs, tagged with `isLottery`. */
async function fetchChainOutputsTagged(
  rpc: ReturnType<typeof makeRpc>,
  height: number,
): Promise<Array<ChainOutput & { isLottery: boolean }>> {
  const blocks = await rpc<Array<{ outputs: RawOutput[] }>>('chain_getOutputs', {
    start_height: 0,
    end_height: height,
  })
  return blocks.flatMap((b) =>
    b.outputs.map((o) => ({
      targetKey: o.targetKey,
      publicKey: o.publicKey,
      amount: leHexToBigInt(o.amountCommitment),
      isLottery: o.lottery === true,
    })),
  )
}

maybe('node-backed: three-user exchange until a lottery payout (#394)', () => {
  let harness: NodeHarness | null = null
  let rpcUrl: string
  let mnemonicA: string
  let signer: WasmSigner
  let rpc: ReturnType<typeof makeRpc>

  // Wallets B and C are DISTINCT externally-derived wallets (BIP39 test vectors).
  const mnemonicB =
    'legal winner thank year wave sausage worth useful legal winner thank yellow'
  const mnemonicC =
    'letter advice cage absurd amount doctor acoustic avoid letter advice cage above'

  const keysOf = (mnemonic: string) => {
    const kp = deriveKeypairs(mnemonic, 0)
    return {
      spendPrivateKey: toHex(kp.spendPrivate),
      viewPrivateKey: toHex(kp.viewPrivate),
    }
  }

  const recipientOf = (mnemonic: string) => {
    const { viewPublic, spendPublic } = deriveDefaultSubaddressPublicKeys(mnemonic, 0)
    return {
      spend_public_key: toHex(spendPublic),
      view_public_key: toHex(viewPublic),
    }
  }

  const sendRpc = (): SendRpc => ({
    getChainHeight: async () =>
      (await rpc<{ chainHeight: number }>('node_getStatus', {})).chainHeight,
    getOutputs: (start, end) =>
      rpc<Array<{ outputs: RawOutput[] }>>('chain_getOutputs', {
        start_height: start,
        end_height: end,
      }).then((blocks) =>
        blocks.flatMap((b) =>
          b.outputs.map((o) => ({
            targetKey: o.targetKey,
            publicKey: o.publicKey,
            amount: leHexToBigInt(o.amountCommitment),
          })),
        ),
      ),
    areKeyImagesSpent: (keyImages) =>
      rpc<KeyImageSpentStatus[]>('chain_areKeyImagesSpent', { keyImages }),
  })

  const chainHeight = async () =>
    (await rpc<{ chainHeight: number }>('node_getStatus', {})).chainHeight

  /**
   * Scan a pre-fetched chain snapshot for the outputs a wallet owns, each tagged
   * with whether it is a lottery payout (target+amount matches a lottery-flagged
   * on-chain output). Takes the snapshot as an argument so callers can fetch the
   * whole chain ONCE per round and reuse it for all three wallets (the node rate
   * limits at 100 req/min, so per-wallet full-chain fetches are wasteful).
   */
  const ownedTaggedFrom = (
    keys: ReturnType<typeof keysOf>,
    candidates: Array<ChainOutput & { isLottery: boolean }>,
  ) => {
    const owned = signer.scanOwnedOutputs({ ...keys, outputs: candidates })
    const lotteryKeys = new Set(
      candidates.filter((c) => c.isLottery).map((c) => `${c.targetKey}:${c.amount}`),
    )
    return owned.map((o) => ({
      ...o,
      isLottery: lotteryKeys.has(`${o.targetKey}:${BigInt(o.amount)}`),
    }))
  }

  /** Convenience: fetch the whole chain and scan one wallet (used sparingly). */
  const ownedTagged = async (keys: ReturnType<typeof keysOf>) => {
    const height = await chainHeight()
    const candidates = await fetchChainOutputsTagged(rpc, height)
    return ownedTaggedFrom(keys, candidates)
  }

  /** Poll until `txHash` is confirmed into a block; returns the block height. */
  const waitMined = async (txHash: string, minAbove: number): Promise<number> => {
    let height: number | null = null
    for (let i = 0; i < 120; i++) {
      const status = await rpc<{ status: string; blockHeight: number | null }>('tx_get', {
        tx_hash: txHash,
      })
      if (status.status === 'confirmed' && status.blockHeight != null) {
        height = status.blockHeight
        break
      }
      await new Promise((r) => setTimeout(r, 1000))
    }
    expect(height, `tx ${txHash} should be mined into a block`).not.toBeNull()
    expect(height!).toBeGreaterThan(minAbove)
    return height!
  }

  /** Wait until the chain has advanced past `from` (a new block was minted). */
  const waitForBlockAbove = async (from: number): Promise<number> => {
    for (let i = 0; i < 60; i++) {
      const h = await chainHeight()
      if (h > from) return h
      await new Promise((r) => setTimeout(r, 1000))
    }
    return chainHeight()
  }

  beforeAll(async () => {
    signer = await loadWasmNode()
    setSigner(signer)

    if (RUN_LOCAL_NODE) {
      const minBlocks = signer.ringSize() + 5
      harness = await startNodeBackedHarness({
        minBlocks,
        rpcPort: 17799,
        gossipPort: 17798,
        // Make freshly created UTXOs lottery-eligible quickly: age >= 1 block,
        // any positive value. Both proposer + validator read this env, so the
        // draw stays consensus-deterministic.
        lottery: { minUtxoAge: 1, minUtxoValue: 1 },
      })
      rpcUrl = harness.rpcUrl
      mnemonicA = harness.mnemonic
    } else {
      rpcUrl = PRE_RUNNING_RPC!
      mnemonicA = PRE_RUNNING_MNEMONIC!
    }
    rpc = makeRpc(rpcUrl)
  }, 300_000)

  afterAll(async () => {
    resetSigner()
    if (harness) await harness.stop()
  })

  it('mines until one of A/B/C wins a lottery payout, asserting ownership + cap + fee/pool/burn accounting', async () => {
    const fee = signer.minFee()

    const keysA = keysOf(mnemonicA)
    const keysB = keysOf(mnemonicB)
    const keysC = keysOf(mnemonicC)
    const addrA = recipientOf(mnemonicA)
    const addrB = recipientOf(mnemonicB)
    const addrC = recipientOf(mnemonicC)

    // The per-block payout cap (anti-grinding) is one block reward. Read it from
    // a mined block so the cap assertion is grounded in the chain's real reward.
    const tipHeight = await chainHeight()
    const tipBlock = await rpc<{ mintingReward: number }>('getBlockByHeight', {
      height: tipHeight,
    })
    const blockReward = BigInt(tipBlock.mintingReward)
    expect(blockReward).toBeGreaterThan(0n)

    // ----------------------------------------------------------------------
    // Step 1: A distributes funds to B and C, split into several UTXOs each, so
    // A/B/C together hold a meaningful share of the eligible candidate set.
    // ----------------------------------------------------------------------
    const SPLIT = 3 // outputs created per recipient
    const PER_OUTPUT = 4_000_000_000n // picocredits per distributed output

    const distribute = async (
      recipient: ReturnType<typeof recipientOf>,
      label: string,
    ): Promise<void> => {
      for (let i = 0; i < SPLIT; i++) {
        const before = await chainHeight()
        const tx = await buildSendTransaction({
          keys: keysA,
          recipient,
          amount: PER_OUTPUT,
          fee,
          rpc: sendRpc(),
        })
        const { txHash } = await rpc<{ txHash: string }>('tx_submit', { tx_hex: tx.txHex })
        await waitMined(txHash, before)
        // eslint-disable-next-line no-console
        console.log(`[#394] funded ${label} output ${i + 1}/${SPLIT} (${PER_OUTPUT} pc)`)
      }
    }

    await distribute(addrB, 'B')
    await distribute(addrC, 'C')

    // B and C now each own SPLIT spendable outputs.
    const bOwned = (await ownedTagged(keysB)).filter((o) => !o.isLottery)
    const cOwned = (await ownedTagged(keysC)).filter((o) => !o.isLottery)
    expect(bOwned.length).toBe(SPLIT)
    expect(cOwned.length).toBe(SPLIT)
    // eslint-disable-next-line no-console
    console.log(`[#394] B owns ${bOwned.length} outputs, C owns ${cOwned.length} outputs`)

    // ----------------------------------------------------------------------
    // Steps 2-3: exchange loop A->B->C->A..., mining each transfer, scanning for
    // a lottery payout to A/B/C after every mined block, until one wins.
    // ----------------------------------------------------------------------
    const wallets = [
      { name: 'A', keys: keysA, addr: addrA },
      { name: 'B', keys: keysB, addr: addrB },
      { name: 'C', keys: keysC, addr: addrC },
    ] as const

    // Track which lottery outputs we've already seen, so a "new" payout is one
    // that appeared this round (keyed by target:amount, unique per payout).
    const seenLotteryKeys = new Set<string>()
    const lotteryKeyOf = (o: { targetKey: string; amount: bigint | number }) =>
      `${o.targetKey}:${BigInt(o.amount)}`

    // Seed the seen-set with any lottery payouts already on the chain (e.g. to
    // the minter A from the distribution blocks) so we only count NEW ones and
    // can attribute the very first A/B/C win to the exchange loop deterministically.
    // One chain fetch, reused for all three wallets.
    {
      const seedHeight = await chainHeight()
      const seedSnapshot = await fetchChainOutputsTagged(rpc, seedHeight)
      for (const w of wallets) {
        for (const o of ownedTaggedFrom(w.keys, seedSnapshot).filter((o) => o.isLottery)) {
          seenLotteryKeys.add(lotteryKeyOf(o))
        }
      }
    }

    const sendAmount = 1_000_000_000n // each exchange transfer
    const MAX_ROUNDS = 40

    let winner:
      | { name: string; targetKey: string; amount: bigint; blockHeight: number }
      | null = null
    let totalFeesPaid = 0n // fees the exchanges + distribution paid (drives the pool/burn)

    // Account for the distribution fees too (they fed the pool/burn already).
    totalFeesPaid += fee * BigInt(SPLIT * 2)

    let round = 0
    while (round < MAX_ROUNDS && winner === null) {
      const from = wallets[round % 3]
      const to = wallets[(round + 1) % 3]

      const before = await chainHeight()

      // Build + submit the transfer through the real wallet path (spent-filtered
      // input selection, #392). If `from` lacks spendable funds this round, just
      // mine an empty block (still applies the lottery) and continue.
      let minedHeight: number
      try {
        const tx = await buildSendTransaction({
          keys: from.keys,
          recipient: to.addr,
          amount: sendAmount,
          fee,
          rpc: sendRpc(),
        })
        const { txHash } = await rpc<{ txHash: string }>('tx_submit', { tx_hex: tx.txHex })
        totalFeesPaid += fee
        minedHeight = await waitMined(txHash, before)
        // eslint-disable-next-line no-console
        console.log(
          `[#394] round ${round}: ${from.name}->${to.name} ${sendAmount}pc mined @${minedHeight}`,
        )
      } catch (err) {
        // Insufficient spendable funds (or no decoys yet): let the node mint one
        // more block (which still runs the lottery on the existing pool) and
        // retry next round.
        minedHeight = await waitForBlockAbove(before)
        // eslint-disable-next-line no-console
        console.log(
          `[#394] round ${round}: ${from.name}->${to.name} skipped (${(err as Error).message}); minted @${minedHeight}`,
        )
      }

      // Scan A/B/C for a NEW lottery payout after this mined block. One chain
      // fetch, reused for all three wallets (rate-limit friendly).
      const roundHeight = await chainHeight()
      const roundSnapshot = await fetchChainOutputsTagged(rpc, roundHeight)
      for (const w of wallets) {
        const owned = ownedTaggedFrom(w.keys, roundSnapshot)
        const newPayout = owned.find(
          (o) => o.isLottery && !seenLotteryKeys.has(lotteryKeyOf(o)),
        )
        // Record every lottery payout seen this round so we never double-count.
        for (const o of owned.filter((o) => o.isLottery)) {
          seenLotteryKeys.add(lotteryKeyOf(o))
        }
        if (newPayout && winner === null) {
          winner = {
            name: w.name,
            targetKey: newPayout.targetKey,
            amount: BigInt(newPayout.amount),
            blockHeight: minedHeight,
          }
          // eslint-disable-next-line no-console
          console.log(
            `[#394] LOTTERY PAYOUT: ${w.name} won ${newPayout.amount}pc (target ${newPayout.targetKey.slice(0, 16)}...) around block ${minedHeight}`,
          )
        }
      }

      round++
    }

    // ----------------------------------------------------------------------
    // Step 4: assertions.
    // ----------------------------------------------------------------------
    expect(
      winner,
      `no A/B/C lottery payout within ${MAX_ROUNDS} rounds — increase rounds / UTXO share or tune lottery params`,
    ).not.toBeNull()

    // Winner is one of A/B/C.
    expect(['A', 'B', 'C']).toContain(winner!.name)

    // Payout > 0 and <= the per-block cap (one block reward, anti-grinding).
    expect(winner!.amount).toBeGreaterThan(0n)
    expect(winner!.amount).toBeLessThanOrEqual(blockReward)
    // eslint-disable-next-line no-console
    console.log(
      `[#394] payout ${winner!.amount}pc is within (0, blockReward=${blockReward}] cap`,
    )

    // The block carrying the payout mined successfully (chain accepted it, so
    // consensus `verify_drawing` passed). Confirm the block exists.
    const payoutBlock = await rpc<{ height: number }>('getBlockByHeight', {
      height: winner!.blockHeight,
    })
    expect(payoutBlock.height).toBe(winner!.blockHeight)

    // The winner still OWNS the payout as a spendable output (its spendable
    // balance includes the payout), independent of its own sends. We assert the
    // payout target key appears among the winner's spendable (#392-filtered)
    // owned outputs.
    const winnerKeys =
      winner!.name === 'A' ? keysA : winner!.name === 'B' ? keysB : keysC
    const finalHeight = await chainHeight()
    const finalOutputs = await fetchChainOutputsTagged(rpc, finalHeight)
    const winnerOwned = signer.scanOwnedOutputs({ ...winnerKeys, outputs: finalOutputs })
    expect(winnerOwned.some((o) => o.targetKey === winner!.targetKey)).toBe(true)
    // And the wallet's spent-filtered spendable balance is positive (it holds
    // at least the still-unspent payout / change).
    const winnerSpendable = await spendableBalance(winnerKeys, sendRpc())
    expect(winnerSpendable).toBeGreaterThan(0n)

    // Fee / pool / burn accounting consistency over the run. Every fee was split
    // 80% pool / 20% burn by `compute_pool_accounting`; the node tracks the
    // cumulative burned amount and the live carryover pool. Assert:
    //   - total burned ~= 20% of all fees paid (within rounding of the integer
    //     permille split, summed per block), and
    //   - the carryover pool is consistent (>= 0; payouts were drawn FROM it, so
    //     it never went negative — a u128 that underflowed would be enormous).
    const status = await rpc<{ totalFeesBurned?: number | string; lotteryPool?: number | string }>(
      'getSupplyInfo',
      {},
    )
    if (status.totalFeesBurned !== undefined) {
      const burned = BigInt(status.totalFeesBurned)
      // Expected burn is the 20% share of total fees. The per-block integer
      // split can round each block's burn by <1 picocredit, so allow a small
      // slack proportional to the number of fee-bearing blocks.
      const expectedBurn = (totalFeesPaid * 200n) / 1000n
      const slack = BigInt(round + SPLIT * 2 + 4)
      expect(burned).toBeGreaterThanOrEqual(expectedBurn - slack)
      expect(burned).toBeLessThanOrEqual(expectedBurn + slack)
      // eslint-disable-next-line no-console
      console.log(
        `[#394] burn accounting: burned=${burned}pc ~= 20% of fees=${totalFeesPaid}pc (expected ${expectedBurn}pc +/- ${slack})`,
      )
    }
    if (status.lotteryPool !== undefined) {
      const pool = BigInt(status.lotteryPool)
      expect(pool).toBeGreaterThanOrEqual(0n)
      // Sanity: an underflowed u128 carryover would be astronomically large.
      expect(pool).toBeLessThan(2n ** 100n)
      // eslint-disable-next-line no-console
      console.log(`[#394] lottery pool carryover = ${pool}pc (payouts drawn from pool)`)
    }
  }, 600_000)
})
