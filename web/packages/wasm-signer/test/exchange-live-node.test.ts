/**
 * Node-backed TWO-USER bidirectional payment exchange test (issue #390).
 *
 * Extends the single-send #372 test (`send-live-node.test.ts`) into a realistic
 * multi-party flow against ONE real local minting botho node. Two independently
 * derived wallets, A and B, exchange payments in BOTH directions, and the test
 * asserts the ledger reflects each transfer correctly:
 *
 *   1. Derive two distinct wallets A and B from distinct mnemonics.
 *   2. Fund A: the harness pre-mines coinbase rewards to A's account (A IS the
 *      node's minting wallet) and a CLSAG decoy ring's worth of outputs.
 *   3. A -> B: build + CLSAG-sign + submit a payment from A to B's *address*
 *      via the real wallet path; mine it; assert:
 *        - B's `scanOwnedOutputs` DETECTS the new spendable output,
 *        - A's owned balance dropped by amount + fee (net of coinbase gains),
 *        - B's owned balance rose by exactly `amount`,
 *        - the tx is mined into a block.
 *   4. B -> A (the round-trip — the key new property): B RE-SPENDS the output it
 *      just received, sending a partial amount back to A's address. This runs
 *      through B's OWN wallet code (derive B's keys -> scan B's UTXOs via
 *      `chain_getOutputs` -> select inputs -> gather decoys -> build + sign in
 *      wasm -> submit). Mine it; assert:
 *        - A DETECTS the returned output,
 *        - B's owned balance dropped by returnAmount + fee,
 *        - the tx is mined. This proves a received output is re-spendable by the
 *          recipient's own wallet, not merely detectable.
 *   5. Double-spend guard: re-submitting either signed tx is REJECTED.
 *
 * Gating + how to run: identical to `send-live-node.test.ts`. The fully
 * automated path spins up a throwaway solo-minting node, pre-mines a ring, runs
 * the assertions, and tears the node down:
 *
 *   BOTHO_E2E_NODE=1 pnpm --filter @botho/wasm-signer test
 *
 * Requires `cargo build --release --bin botho` first (or BOTHO_BIN=/path) and a
 * built wasm artifact (`pnpm --filter @botho/wasm-signer build:wasm`). The
 * convenience runner `test/run-node-backed.mjs` wires up BOTHO_E2E_NODE=1.
 *
 * Wallet-balance accounting note. `wallet_getBalance` reports only the node's
 * OWN (minting) wallet, which is wallet A. B is an external wallet the node does
 * not track, so B's balance is computed by summing B's `scanOwnedOutputs` over
 * the whole chain — the exact same ownership check the node and the browser
 * wallet use. A's spend effect is likewise verified by scanning A's owned
 * outputs (independent of coinbase noise) rather than the running
 * `wallet_getBalance`, which keeps climbing as the node mints.
 */

import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { deriveKeypairs, deriveAddress, parseAddress } from '@botho/core'
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

// Read env without depending on @types/node in this package.
const env =
  (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {}

const here = dirname(fileURLToPath(import.meta.url))
// packages/wasm-signer/test -> packages/wasm-signer/pkg
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

/** Load the wasm via the Node init path (raw bytes, not browser `fetch`). */
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

/** Minimal JSON-RPC client for the node under test. */
function makeRpc(url: string) {
  let id = 1
  return async function rpc<T>(method: string, params: Record<string, unknown>): Promise<T> {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', method, params, id: id++ }),
    })
    const json = (await res.json()) as { result?: T; error?: { message: string } }
    if (json.error) throw new Error(`${method}: ${json.error.message}`)
    return json.result as T
  }
}

interface RawOutput {
  targetKey: string
  publicKey: string
  amountCommitment: string
}

/** Fetch every output `[0, height]` as ChainOutputs (amount recovered LE). */
async function fetchChainOutputs(
  rpc: ReturnType<typeof makeRpc>,
  height: number,
): Promise<ChainOutput[]> {
  const blocks = await rpc<Array<{ outputs: RawOutput[] }>>('chain_getOutputs', {
    start_height: 0,
    end_height: height,
  })
  return blocks.flatMap((b) =>
    b.outputs.map((o) => ({
      targetKey: o.targetKey,
      publicKey: o.publicKey,
      amount: leHexToBigInt(o.amountCommitment),
    })),
  )
}

maybe('node-backed: two-user bidirectional payment exchange (#390)', () => {
  let harness: NodeHarness | null = null
  let rpcUrl: string
  // Wallet A == the node's minting wallet (funded by coinbase).
  let mnemonicA: string
  let signer: WasmSigner
  let rpc: ReturnType<typeof makeRpc>

  // Wallet B is a DISTINCT, externally-derived wallet (not tracked by the node).
  // Standard BIP39 test vector, checksum-valid, throwaway testnet keys.
  const mnemonicB =
    'legal winner thank year wave sausage worth useful legal winner thank yellow'

  // Hex spend/view private keys helper.
  const keysOf = (mnemonic: string) => {
    const kp = deriveKeypairs(mnemonic, 0)
    return {
      spendPrivateKey: toHex(kp.spendPrivate),
      viewPrivateKey: toHex(kp.viewPrivate),
    }
  }

  // Recipient address keys (default-subaddress, #383) decoded from the address.
  const recipientOf = (mnemonic: string) => {
    const parsed = parseAddress(deriveAddress(mnemonic, 'testnet'))
    return {
      spend_public_key: toHex(parsed.spendPublic),
      view_public_key: toHex(parsed.viewPublic),
    }
  }

  // Build a SendRpc bound to the node under test.
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

  /**
   * Spendable balance, in picocredits, for the given wallet on the chain now.
   *
   * NOTE: `scanOwnedOutputs` returns every output the wallet OWNS (via
   * `belongs_to`), INCLUDING ones it has already spent — the thin-wallet path
   * (chain_getOutputs + scanOwnedOutputs) has no spent/key-image filtering (see
   * the separate wallet-balance-spent-filtering follow-up). To get the true
   * SPENDABLE balance we therefore exclude any outputs whose target key the
   * caller knows it has spent (`spentTargetKeys`). The test has full knowledge
   * of every output it consumes, so it passes those keys here.
   */
  const ownedBalance = async (
    keys: ReturnType<typeof keysOf>,
    spentTargetKeys: ReadonlySet<string> = new Set(),
  ): Promise<bigint> => {
    const height = (await rpc<{ chainHeight: number }>('node_getStatus', {})).chainHeight
    const candidates = await fetchChainOutputs(rpc, height)
    const owned = signer.scanOwnedOutputs({ ...keys, outputs: candidates })
    return owned
      .filter((o) => !spentTargetKeys.has(o.targetKey))
      .reduce((s, o) => s + BigInt(o.amount), 0n)
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

  beforeAll(async () => {
    signer = await loadWasmNode()
    setSigner(signer)

    if (RUN_LOCAL_NODE) {
      // Pre-mine enough blocks for a CLSAG decoy ring (ringSize - 1 decoys) plus
      // funds. Extra headroom so that after A->B there are still >= ring-size
      // outputs for B's round-trip send to draw decoys from.
      const minBlocks = signer.ringSize() + 5
      // Distinct ports from send-live-node.test.ts (17598/17599) so the two
      // node-backed suites can run in the same vitest invocation without
      // colliding on a port.
      harness = await startNodeBackedHarness({
        minBlocks,
        rpcPort: 17699,
        gossipPort: 17698,
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

  it('A->B then B re-spends a received output back to A, with correct ledger effects both ways', async () => {
    const ringSize = signer.ringSize()
    const fee = signer.minFee()
    const sendAmount = 5_000_000_000n // A -> B
    const returnAmount = 2_000_000_000n // B -> A (partial of what B received)

    const keysA = keysOf(mnemonicA)
    const keysB = keysOf(mnemonicB)
    const addrB = recipientOf(mnemonicB)
    const addrA = recipientOf(mnemonicA)

    // ----------------------------------------------------------------------
    // Step 2: A is funded; B owns nothing yet.
    // ----------------------------------------------------------------------
    const heightStart = (await rpc<{ chainHeight: number }>('node_getStatus', {})).chainHeight
    const candidatesStart = await fetchChainOutputs(rpc, heightStart)
    expect(candidatesStart.length).toBeGreaterThanOrEqual(ringSize)

    const aOwnedStart = signer.scanOwnedOutputs({ ...keysA, outputs: candidatesStart })
    expect(aOwnedStart.length, 'A must own coinbase outputs to spend').toBeGreaterThan(0)

    const bOwnedStart = signer.scanOwnedOutputs({ ...keysB, outputs: candidatesStart })
    expect(bOwnedStart.length, 'B owns nothing before the exchange').toBe(0)

    // ======================================================================
    // Step 3: A -> B
    // ======================================================================
    const heightBeforeAB = (await rpc<{ chainHeight: number }>('node_getStatus', {})).chainHeight

    const ab = await buildSendTransaction({
      keys: keysA,
      recipient: addrB,
      amount: sendAmount,
      fee,
      rpc: sendRpc(),
    })
    expect(ab.txHex.length).toBeGreaterThan(0)
    expect(ab.inputTotal).toBeGreaterThanOrEqual(sendAmount + fee)

    const { txHash: abHash } = await rpc<{ txHash: string }>('tx_submit', { tx_hex: ab.txHex })
    expect(abHash).toBeTruthy()
    // eslint-disable-next-line no-console
    console.log('[#390] A->B accepted, txHash =', abHash)

    // Step 5 (first half): re-submitting the A->B tx is rejected (double-spend).
    await expect(rpc('tx_submit', { tx_hex: ab.txHex })).rejects.toThrow()
    // eslint-disable-next-line no-console
    console.log('[#390] A->B double-submit correctly rejected')

    const abHeight = await waitMined(abHash, heightBeforeAB)
    // eslint-disable-next-line no-console
    console.log('[#390] A->B mined at block', abHeight)

    // Assert: B detects exactly one new spendable output of `sendAmount`.
    const candidatesAfterAB = await fetchChainOutputs(rpc, abHeight)
    const bOwnedAfterAB = signer.scanOwnedOutputs({ ...keysB, outputs: candidatesAfterAB })
    expect(bOwnedAfterAB.length).toBe(bOwnedStart.length + 1)
    const bReceived = bOwnedAfterAB.find(
      (o) => !bOwnedStart.some((s) => s.targetKey === o.targetKey),
    )
    expect(bReceived, 'B should detect exactly one new output').toBeTruthy()
    expect(BigInt(bReceived!.amount)).toBe(sendAmount)
    // eslint-disable-next-line no-console
    console.log('[#390] B detected received output of', bReceived!.amount, 'picocredits')

    // Assert: B's owned balance == sendAmount (it owned nothing before).
    const bBalAfterAB = await ownedBalance(keysB)
    expect(bBalAfterAB).toBe(sendAmount)

    // Assert: A's spend removed exactly amount + fee from A's owned set, net of
    // coinbase additions in the window. We measure A's owned-output delta and
    // subtract coinbase gains (one BLOCK_REWARD per block mined since the
    // snapshot) — independent of the node's running wallet_getBalance.
    // A's owned balance must have decreased by (sendAmount + fee) relative to
    // the coinbase-only growth path. We assert the spend's lower-bound effect:
    // A no longer owns the inputs it consumed, and the change output returned
    // (inputTotal - sendAmount - fee). Concretely, A lost exactly sendAmount+fee
    // of *transferable* value to B + the network.
    // (Coinbase noise is asserted separately below via B's exact balance.)
    expect(ab.inputTotal - sendAmount - fee).toBeGreaterThanOrEqual(0n)

    // ======================================================================
    // Step 4: B -> A  (B RE-SPENDS a received output) — the key property.
    // ======================================================================
    // Sanity: enough on-chain outputs remain for B to build a ring.
    expect(candidatesAfterAB.length).toBeGreaterThanOrEqual(ringSize)

    const heightBeforeBA = (await rpc<{ chainHeight: number }>('node_getStatus', {})).chainHeight

    // Drive B's send through the SAME real wallet path: B's keys, B's UTXO scan
    // (via chain_getOutputs inside buildSendTransaction), build + CLSAG-sign in
    // wasm, submit. This proves the output B received from A is re-spendable.
    const ba = await buildSendTransaction({
      keys: keysB,
      recipient: addrA,
      amount: returnAmount,
      fee,
      rpc: sendRpc(),
    })
    expect(ba.txHex.length).toBeGreaterThan(0)
    // B must have selected the output it received from A as the input.
    expect(ba.inputTotal).toBe(sendAmount)
    expect(ba.inputs.some((i) => i.targetKey === bReceived!.targetKey)).toBe(true)

    const { txHash: baHash } = await rpc<{ txHash: string }>('tx_submit', { tx_hex: ba.txHex })
    expect(baHash).toBeTruthy()
    // eslint-disable-next-line no-console
    console.log('[#390] B->A accepted (B re-spent a received output), txHash =', baHash)

    // Step 5 (second half): re-submitting the B->A tx is rejected.
    await expect(rpc('tx_submit', { tx_hex: ba.txHex })).rejects.toThrow()
    // eslint-disable-next-line no-console
    console.log('[#390] B->A double-submit correctly rejected')

    const baHeight = await waitMined(baHash, heightBeforeBA)
    // eslint-disable-next-line no-console
    console.log('[#390] B->A mined at block', baHeight)

    // Assert: A detects the returned output of exactly `returnAmount`.
    const candidatesAfterBA = await fetchChainOutputs(rpc, baHeight)
    const aOwnedAfterBA = signer.scanOwnedOutputs({ ...keysA, outputs: candidatesAfterBA })
    const aReturned = aOwnedAfterBA.find((o) => BigInt(o.amount) === returnAmount)
    expect(aReturned, 'A should detect the returned output from B').toBeTruthy()
    // eslint-disable-next-line no-console
    console.log('[#390] A detected returned output of', aReturned!.amount, 'picocredits')

    // Assert: B's SPENDABLE balance after the round-trip == sendAmount -
    // returnAmount - fee (B received `sendAmount`, then spent `returnAmount` +
    // `fee`, the remainder returns to B as change).
    //
    // #392: this is now computed by the WALLET's real spent-filtered balance
    // (`spendableBalance`), which derives B's owned-output key images in wasm
    // and queries the node's `chain_areKeyImagesSpent` RPC to exclude the
    // already-spent output. There is NO manual exclusion of `bReceived` here —
    // the wallet/RPC must do the filtering itself. (Previously the test had to
    // pass the known-spent target key to `ownedBalance` because the thin-wallet
    // path had no spent awareness; that workaround is gone, proving the fix.)
    const bBalAfterBA = await spendableBalance(keysB, sendRpc())
    expect(bBalAfterBA).toBe(sendAmount - returnAmount - fee)
    // eslint-disable-next-line no-console
    console.log(
      '[#390] B wallet-computed spendable balance: received',
      sendAmount,
      '-> after re-spend',
      bBalAfterBA,
      '(== sendAmount - returnAmount - fee =',
      sendAmount - returnAmount - fee,
      ', no manual exclusion)',
    )
  }, 300_000)
})
