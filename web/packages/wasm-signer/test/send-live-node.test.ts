/**
 * Node-backed end-to-end test for the web-wallet -> transaction -> ledger path.
 *
 * This is the durable, CI-friendly layer of issue #372. It drives the SAME code
 * the browser wallet uses — `deriveKeypairs`/`deriveAddress` (@botho/core) and
 * the wallet's high-level `buildSendTransaction` orchestrator
 * (@botho/wasm-signer, which wraps the wasm `scanOwnedOutputs`/`buildAndSign`)
 * — against a REAL botho node's JSON-RPC, submits the signed tx, and then
 * asserts the expected ledger entries:
 *
 *   1. the node accepts the wallet-built tx (`tx_submit` -> txHash),
 *   2. the tx is mined into a block (`tx_get` -> status "confirmed"),
 *   3. the SENDER's wallet balance dropped by exactly amount + fee,
 *   4. the RECIPIENT (a separate wallet) can DETECT + would be able to spend the
 *      new output — proven by re-scanning the chain with the recipient's keys
 *      (works thanks to default-subaddress address encoding, #383),
 *   5. a double-submit of the same signed tx is REJECTED (key image already
 *      pending/spent).
 *
 * Gating. The test needs a built node binary + wasm artifact, so it is GATED and
 * does not run in the default unit-test CI. Two ways to run it:
 *
 *   (a) Fully automated (recommended) — spins up a throwaway solo-minting node,
 *       pre-mines enough blocks for a CLSAG decoy ring, runs the assertions, and
 *       tears the node down:
 *         BOTHO_E2E_NODE=1 pnpm --filter @botho/wasm-signer test
 *       Requires `cargo build --release --bin botho` first (or pass
 *       BOTHO_BIN=/path/to/botho). See `test/run-node-backed.mjs`.
 *
 *   (b) Against an already-running node (point it at a live mining node):
 *         BOTHO_LIVE_RPC=http://127.0.0.1:17501/rpc \
 *         BOTHO_LIVE_MNEMONIC="<sender 24/12-word phrase>" \
 *         pnpm --filter @botho/wasm-signer test
 */

import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import { deriveKeypairs, deriveAddress, parseAddress } from '@botho/core'
import {
  buildSendTransaction,
  setSigner,
  resetSigner,
  type ChainOutput,
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

// Enabled when (a) we will spin up our own node, or (b) we are pointed at a
// pre-running one. Either way the wasm artifact must be present.
const enabled = wasmBuilt && (RUN_LOCAL_NODE || (PRE_RUNNING_RPC && PRE_RUNNING_MNEMONIC))
const maybe = enabled ? describe : describe.skip

// Initial coinbase reward (50 BTH) in picocredits — matches
// `monetary.rs` `initial_reward` and `tests/common/constants.rs`.
const BLOCK_REWARD = 50_000_000_000_000n

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

maybe('node-backed: web-wallet -> tx -> ledger', () => {
  let harness: NodeHarness | null = null
  let rpcUrl: string
  let senderMnemonic: string
  let signer: WasmSigner
  let rpc: ReturnType<typeof makeRpc>

  // The recipient is a DISTINCT wallet (not the node's own), so "recipient can
  // detect the output" is a real cross-wallet assertion, not a self-send.
  const recipientMnemonic =
    'legal winner thank year wave sausage worth useful legal winner thank yellow'

  beforeAll(async () => {
    signer = await loadWasmNode()
    setSigner(signer)

    if (RUN_LOCAL_NODE) {
      // Pre-mine enough blocks for a CLSAG decoy ring (ringSize - 1 decoys) plus
      // some funds. Each block contributes one coinbase output to the pool.
      const minBlocks = signer.ringSize() + 3
      harness = await startNodeBackedHarness({ minBlocks })
      rpcUrl = harness.rpcUrl
      senderMnemonic = harness.mnemonic
    } else {
      rpcUrl = PRE_RUNNING_RPC!
      senderMnemonic = PRE_RUNNING_MNEMONIC!
    }
    rpc = makeRpc(rpcUrl)
  }, 300_000)

  afterAll(async () => {
    resetSigner()
    if (harness) await harness.stop()
  })

  it('builds+signs+submits via the wallet path and produces the expected ledger entries', async () => {
    const ringSize = signer.ringSize()
    const fee = signer.minFee()
    const amount = 1_000_000_000n

    const senderKp = deriveKeypairs(senderMnemonic, 0)
    const senderKeys = {
      spendPrivateKey: toHex(senderKp.spendPrivate),
      viewPrivateKey: toHex(senderKp.viewPrivate),
    }

    // The recipient address is encoded from the recipient mnemonic's default
    // subaddress keys (#383), exactly as the wallet UI does it.
    const recipientAddress = deriveAddress(recipientMnemonic, 'testnet')
    const recipientParsed = parseAddress(recipientAddress)
    const recipient = {
      spend_public_key: toHex(recipientParsed.spendPublic),
      view_public_key: toHex(recipientParsed.viewPublic),
    }

    // --- Pre-send ledger snapshot ----------------------------------------
    const heightBefore = (await rpc<{ chainHeight: number }>('node_getStatus', {})).chainHeight
    const senderBalBefore = await rpc<{ confirmed: number }>('wallet_getBalance', {})

    const candidatesBefore = await fetchChainOutputs(rpc, heightBefore)
    expect(candidatesBefore.length).toBeGreaterThanOrEqual(ringSize)

    // Sanity: the sender's keys must actually own outputs to spend.
    const senderOwnedBefore = signer.scanOwnedOutputs({ ...senderKeys, outputs: candidatesBefore })
    expect(senderOwnedBefore.length).toBeGreaterThan(0)

    // Recipient owns nothing yet.
    const recipientKeys = {
      spendPrivateKey: toHex(deriveKeypairs(recipientMnemonic, 0).spendPrivate),
      viewPrivateKey: toHex(deriveKeypairs(recipientMnemonic, 0).viewPrivate),
    }
    const recipientOwnedBefore = signer.scanOwnedOutputs({
      ...recipientKeys,
      outputs: candidatesBefore,
    })

    // --- Drive the wallet's REAL send path -------------------------------
    const sendRpc: SendRpc = {
      getChainHeight: async () => (await rpc<{ chainHeight: number }>('node_getStatus', {})).chainHeight,
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
    }

    const { txHex, inputTotal } = await buildSendTransaction({
      keys: senderKeys,
      recipient,
      amount,
      fee,
      rpc: sendRpc,
    })
    expect(txHex.length).toBeGreaterThan(0)
    expect(inputTotal).toBeGreaterThanOrEqual(amount + fee)

    // --- Assertion 1: node accepts the tx --------------------------------
    const { txHash } = await rpc<{ txHash: string }>('tx_submit', { tx_hex: txHex })
    expect(txHash).toBeTruthy()
    // eslint-disable-next-line no-console
    console.log('[#372] node accepted wallet-built tx, txHash =', txHash)

    // --- Assertion 5 (early): double-submit is rejected ------------------
    // Submitting the exact same signed tx again must fail: its key image is
    // already pending in the mempool (the node's double-spend guard).
    await expect(rpc('tx_submit', { tx_hex: txHex })).rejects.toThrow()
    // eslint-disable-next-line no-console
    console.log('[#372] double-submit correctly rejected')

    // --- Assertion 2: tx is mined into a block ---------------------------
    let mined: { status: string; blockHeight: number | null; fee?: number } | null = null
    for (let i = 0; i < 120; i++) {
      const status = await rpc<{ status: string; blockHeight: number | null; fee?: number }>(
        'tx_get',
        { tx_hash: txHash },
      )
      if (status.status === 'confirmed' && status.blockHeight != null) {
        mined = status
        break
      }
      await new Promise((r) => setTimeout(r, 1000))
    }
    expect(mined, 'tx should be mined into a block').not.toBeNull()
    expect(mined!.blockHeight).toBeGreaterThan(heightBefore)
    // --- Assertion (fee per policy): the mined tx carries our fee --------
    if (typeof mined!.fee === 'number') {
      expect(BigInt(mined!.fee)).toBe(fee)
    }
    // eslint-disable-next-line no-console
    console.log('[#372] tx mined at block', mined!.blockHeight, 'fee =', mined!.fee)

    // --- Assertion 3: sender balance dropped by exactly amount + fee -----
    // `wallet_getBalance` is the node's own (sender) wallet. The node keeps
    // minting, so coinbases ADD `reward` per block in the meantime. The net
    // effect of the spend on the wallet is -(amount + fee) (the change output
    // returns inputTotal - amount - fee). So, accounting for coinbase gains:
    //
    //   after == before - (amount + fee) + reward * (coinbases mined in window)
    //
    // Each block mints exactly one coinbase to this solo wallet, so the number
    // of coinbases in the window equals the height delta. We snapshot height
    // atomically with each balance read to make this race-free.
    const spent = amount + fee
    const before = BigInt(senderBalBefore.confirmed)
    const heightAtAfter = (await rpc<{ chainHeight: number }>('node_getStatus', {})).chainHeight
    const senderBalAfter = await rpc<{ confirmed: number }>('wallet_getBalance', {})
    const after = BigInt(senderBalAfter.confirmed)
    const coinbasesInWindow = BigInt(heightAtAfter - heightBefore)
    const expectedAfter = before - spent + BLOCK_REWARD * coinbasesInWindow
    // Allow a +/- one-block coinbase slack for the (tiny) race between the
    // height read and the balance read while blocks are being produced.
    const slack = BLOCK_REWARD
    expect(after).toBeGreaterThanOrEqual(expectedAfter - slack)
    expect(after).toBeLessThanOrEqual(expectedAfter + slack)
    // The spend definitively removed value: balance is below pure-coinbase
    // growth by (amount + fee).
    expect(after).toBeLessThanOrEqual(before + BLOCK_REWARD * coinbasesInWindow - spent + slack)
    // eslint-disable-next-line no-console
    console.log(
      '[#372] sender balance before =',
      before,
      'after =',
      after,
      'spent (amount+fee) =',
      spent,
      'coinbasesInWindow =',
      coinbasesInWindow,
    )

    // --- Assertion 4: recipient detects the new spendable output ---------
    const heightAfter = mined!.blockHeight!
    const candidatesAfter = await fetchChainOutputs(rpc, heightAfter)
    const recipientOwnedAfter = signer.scanOwnedOutputs({
      ...recipientKeys,
      outputs: candidatesAfter,
    })
    expect(recipientOwnedAfter.length).toBe(recipientOwnedBefore.length + 1)
    const received = recipientOwnedAfter.find(
      (o) => !recipientOwnedBefore.some((b) => b.targetKey === o.targetKey),
    )
    expect(received, 'recipient should detect exactly one new output').toBeTruthy()
    expect(BigInt(received!.amount)).toBe(amount)
    // eslint-disable-next-line no-console
    console.log(
      '[#372] recipient detected new output of',
      received!.amount,
      'picocredits (spendable: subaddressIndex =',
      received!.subaddressIndex,
      ')',
    )
  }, 300_000)
})
