/**
 * Node-backed end-to-end test for CLAIMABLE PAYMENT LINKS (#460).
 *
 * Exercises the full bearer claim-link cycle against a REAL botho node, driving
 * the SAME code the browser wallet uses: `@botho/core` claim-link helpers
 * (generate ephemeral mnemonic, build/parse the link) + `@botho/core` address
 * derivation + the `@botho/wasm-signer` `buildSendTransaction` / `spendableBalance`
 * send/scan path. NO node, consensus, RPC, or address-format change — a claim
 * link is just a normal CLSAG send to (then from) an ephemeral wallet.
 *
 *   1. CREATE: the sender funds a fresh ephemeral wallet (amount + sweep-fee
 *      reserve) with a normal CLSAG send -> the funding tx mines.
 *   2. PARSE:  the link round-trips (entropy <-> mnemonic <-> ephemeral address).
 *   3. SCAN:   the ephemeral wallet's spendable balance == funded amount.
 *   4. CLAIM:  sweep the ephemeral output to a DISTINCT recipient, paying the
 *      sweep fee from the funded output -> recipient receives ~amount.
 *   5. DOUBLE-CLAIM: after the sweep, the ephemeral wallet is empty (key image
 *      spent) -> a second scan reads back 0 ("already claimed").
 *
 * Gating: identical to send-live-node.test.ts. Run with a throwaway local node:
 *   BOTHO_E2E_NODE=1 pnpm --filter @botho/wasm-signer test
 * or against a running node (must be MINTING so txs confirm):
 *   BOTHO_LIVE_RPC=http://127.0.0.1:17501/rpc \
 *   BOTHO_LIVE_MNEMONIC="<funded sender phrase>" \
 *   pnpm --filter @botho/wasm-signer test
 */

import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

import { describe, it, expect, beforeAll, afterAll } from 'vitest'
import {
  deriveKeypairs,
  parseAddress,
  createClaimLinkMnemonic,
  buildClaimLink,
  parseClaimLinkFragment,
} from '@botho/core'
import {
  buildSendTransaction,
  deriveV2Address,
  spendableBalance,
  setSigner,
  resetSigner,
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

const MIN_TX_FEE = 100_000_000n
const SWEEP_FEE_RESERVE = 2n * MIN_TX_FEE
const LINK_NET = 1_000_000_000_000n // 1 BTH net to the recipient

const toHex = (b: Uint8Array) =>
  Array.from(b).map((x) => x.toString(16).padStart(2, '0')).join('')
function leHexToBigInt(hex: string): bigint {
  let r = 0n
  for (let i = hex.length - 2; i >= 0; i -= 2) r = (r << 8n) | BigInt(parseInt(hex.slice(i, i + 2), 16))
  return r
}

interface RawOutput {
  targetKey: string
  publicKey: string
  amountCommitment: string
}

interface WasmMod extends WasmSigner {
  default: (init: { module_or_path: BufferSource }) => Promise<unknown>
}
async function loadWasmNode(): Promise<WasmSigner> {
  const mod = (await import(/* @vite-ignore */ wasmGlue)) as unknown as WasmMod
  await mod.default({ module_or_path: readFileSync(wasmBin) })
  return {
    buildAndSign: (r) => mod.buildAndSign(r),
    scanOwnedOutputs: (r) => mod.scanOwnedOutputs(r),
    computeOwnedOutputKeyImages: (r) => mod.computeOwnedOutputKeyImages(r),
    ringSize: () => mod.ringSize(),
    minFee: () => mod.minFee(),
  }
}

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

const keysOf = (m: string) => {
  const kp = deriveKeypairs(m, 0)
  return { spendPrivateKey: toHex(kp.spendPrivate), viewPrivateKey: toHex(kp.viewPrivate) }
}
const recipientOf = (addr: string) => {
  const k = parseAddress(addr)
  return { spend_public_key: toHex(k.spendPublic), view_public_key: toHex(k.viewPublic) }
}

maybe('claim-link node-backed: create -> claim -> already-claimed (#460)', () => {
  let harness: NodeHarness | null = null
  let rpcUrl: string
  let senderMnemonic: string
  let signer: WasmSigner
  let rpc: ReturnType<typeof makeRpc>
  let sendRpc: SendRpc

  beforeAll(async () => {
    signer = await loadWasmNode()
    setSigner(signer)
    if (RUN_LOCAL_NODE) {
      const minBlocks = signer.ringSize() + 5
      harness = await startNodeBackedHarness({ minBlocks })
      rpcUrl = harness.rpcUrl
      senderMnemonic = harness.mnemonic
    } else {
      rpcUrl = PRE_RUNNING_RPC!
      senderMnemonic = PRE_RUNNING_MNEMONIC!
    }
    rpc = makeRpc(rpcUrl)
    sendRpc = {
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
      areKeyImagesSpent: (keyImages) =>
        rpc<KeyImageSpentStatus[]>('chain_areKeyImagesSpent', { keyImages }),
    }
  }, 300_000)

  afterAll(async () => {
    if (harness) await harness.stop()
    resetSigner()
  })

  async function waitForMined(txHash: string): Promise<number> {
    for (let i = 0; i < 180; i++) {
      const status = await rpc<{ status: string; blockHeight: number | null }>('tx_get', {
        tx_hash: txHash,
      })
      if (status.status === 'confirmed' && status.blockHeight != null) return status.blockHeight
      await new Promise((r) => setTimeout(r, 1000))
    }
    throw new Error(`tx ${txHash} was not mined in time`)
  }

  async function sendTx(senderM: string, destAddr: string, amount: bigint, fee: bigint) {
    const { txHex } = await buildSendTransaction({
      keys: keysOf(senderM),
      recipient: recipientOf(destAddr),
      amount,
      fee,
      rpc: sendRpc,
    })
    const { txHash } = await rpc<{ txHash: string }>('tx_submit', { tx_hex: txHex })
    return txHash
  }

  it('funds an ephemeral link, claims it to a fresh address, then reads back claimed', async () => {
    // 1. CREATE — fund a fresh ephemeral wallet (net + sweep-fee reserve).
    const ephMnemonic = createClaimLinkMnemonic()
    const ephAddress = await deriveV2Address(ephMnemonic)
    const url = buildClaimLink('https://wallet.botho.io', ephMnemonic, LINK_NET)

    const fundTx = await sendTx(senderMnemonic, ephAddress, LINK_NET + SWEEP_FEE_RESERVE, MIN_TX_FEE)
    expect(fundTx).toBeTruthy()
    await waitForMined(fundTx)

    // 2. PARSE — link round-trips back to the same ephemeral wallet.
    const parsed = parseClaimLinkFragment(url)
    expect(parsed.amountHint).toBe(LINK_NET)
    expect(await deriveV2Address(parsed.mnemonic)).toBe(ephAddress)

    // 3. SCAN — the ephemeral wallet now holds exactly the funded amount.
    const ephGross = await spendableBalance(keysOf(parsed.mnemonic), sendRpc)
    expect(ephGross).toBe(LINK_NET + SWEEP_FEE_RESERVE)

    // 4. CLAIM — sweep to a DISTINCT recipient, fee paid from the output.
    const destMnemonic = createClaimLinkMnemonic()
    const destAddress = await deriveV2Address(destMnemonic)
    const sweepNet = ephGross - MIN_TX_FEE
    const sweepTx = await sendTx(parsed.mnemonic, destAddress, sweepNet, MIN_TX_FEE)
    await waitForMined(sweepTx)

    const destBal = await spendableBalance(keysOf(destMnemonic), sendRpc)
    expect(destBal).toBe(sweepNet)
    // Recipient nets at least the intended amount (reserve covered the fee).
    expect(destBal).toBeGreaterThanOrEqual(LINK_NET)

    // 5. DOUBLE-CLAIM — the ephemeral output's key image is spent; re-scan is 0.
    const ephAfter = await spendableBalance(keysOf(parsed.mnemonic), sendRpc)
    expect(ephAfter).toBe(0n)
  }, 300_000)
})
