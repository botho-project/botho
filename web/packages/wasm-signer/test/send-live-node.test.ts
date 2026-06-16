/**
 * Live-node end-to-end test for the client-side send path.
 *
 * This is GATED on `BOTHO_LIVE_RPC` + `BOTHO_LIVE_MNEMONIC` so it never runs in
 * CI (no node available there). When those env vars are set it drives the SAME
 * primitives the browser wallet uses — `deriveKeypairs` (@botho/core) and the
 * wasm `buildAndSign`/`scanOwnedOutputs` (@botho/wasm-signer) — against a REAL
 * botho node's JSON-RPC, submits the signed tx, and asserts the node accepts it
 * (`tx_submit` returns a txHash). This is the make-or-break proof of key parity
 * + the whole build/sign/submit path end to end.
 *
 * It mirrors `buildSendTransaction`'s orchestration but loads the wasm via the
 * Node init path (raw bytes) instead of the browser `fetch` path, because under
 * vitest/node the browser `loadSigner()` cannot `fetch` the wasm URL.
 *
 * Run against a local mining node (see PR description for setup):
 *   BOTHO_LIVE_RPC=http://127.0.0.1:17199/rpc \
 *   BOTHO_LIVE_MNEMONIC="abandon ... art" \
 *   pnpm vitest run packages/web-wallet/src/contexts/send-live-node.test.ts
 */

import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

import { describe, it, expect } from 'vitest'
import { deriveKeypairs, parseAddress, deriveAddress } from '@botho/core'

// Read env without depending on @types/node in this package.
const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {}
const RPC = env.BOTHO_LIVE_RPC
const MNEMONIC = env.BOTHO_LIVE_MNEMONIC

const here = dirname(fileURLToPath(import.meta.url))
// packages/web-wallet/src/contexts -> packages/wasm-signer/pkg
// packages/wasm-signer/test -> packages/wasm-signer/pkg
const pkgDir = join(here, '..', 'pkg')
const wasmGlue = join(pkgDir, 'bth_wasm_signer.js')
const wasmBin = join(pkgDir, 'bth_wasm_signer_bg.wasm')

const toHex = (b: Uint8Array) =>
  Array.from(b).map((x) => x.toString(16).padStart(2, '0')).join('')

function leHexToBigInt(hex: string): bigint {
  let result = 0n
  for (let i = hex.length - 2; i >= 0; i -= 2) {
    result = (result << 8n) | BigInt(parseInt(hex.slice(i, i + 2), 16))
  }
  return result
}

let rpcId = 1
async function rpc<T>(method: string, params: Record<string, unknown>): Promise<T> {
  const res = await fetch(RPC!, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', method, params, id: rpcId++ }),
  })
  const json = (await res.json()) as { result?: T; error?: { message: string } }
  if (json.error) throw new Error(`${method}: ${json.error.message}`)
  return json.result as T
}

interface WasmMod {
  default: (init: { module_or_path: BufferSource }) => Promise<unknown>
  buildAndSign: (request: unknown) => string
  scanOwnedOutputs: (request: unknown) => Array<{
    targetKey: string
    publicKey: string
    amount: bigint
    subaddressIndex: bigint
  }>
  ringSize: () => number
  minFee: () => bigint
}

async function loadWasmNode(): Promise<WasmMod> {
  const mod = (await import(/* @vite-ignore */ wasmGlue)) as unknown as WasmMod
  await mod.default({ module_or_path: readFileSync(wasmBin) })
  return mod
}

const wasmBuilt = existsSync(wasmGlue) && existsSync(wasmBin)
const maybe = RPC && MNEMONIC && wasmBuilt ? describe : describe.skip

maybe('live node: client-built tx is accepted', () => {
  it('derives node-identical keys, builds+signs, and the node accepts it', async () => {
    const mnemonic = MNEMONIC!
    const wasm = await loadWasmNode()
    const ringSize = wasm.ringSize()
    const fee = wasm.minFee()

    const kp = deriveKeypairs(mnemonic, 0)
    const keys = {
      spendPrivateKey: toHex(kp.spendPrivate),
      viewPrivateKey: toHex(kp.viewPrivate),
    }

    // NOTE: `wallet_getAddress` returns the node's *default subaddress* keys,
    // which are intentionally NOT the account root public keys deriveKeypairs
    // returns (subaddress derivation hashes the account keys). So we do not
    // compare them directly. The true proof of key parity is that the scan
    // below (which uses belongs_to against the account keys) finds the node's
    // own coinbase outputs, and that the node accepts the resulting signature.

    // 2. Fetch chain outputs.
    const status = await rpc<{ chainHeight: number }>('node_getStatus', {})
    const height = status.chainHeight
    const blocks = await rpc<
      Array<{ outputs: Array<{ targetKey: string; publicKey: string; amountCommitment: string }> }>
    >('chain_getOutputs', { start_height: 0, end_height: height })
    const candidates = blocks.flatMap((b) =>
      b.outputs.map((o) => ({
        targetKey: o.targetKey,
        publicKey: o.publicKey,
        amount: leHexToBigInt(o.amountCommitment),
      })),
    )

    // 3. Scan owned outputs via the node-identical wasm check.
    const owned = wasm.scanOwnedOutputs({ ...keys, outputs: candidates })
    expect(owned.length).toBeGreaterThan(0)

    // 4. Select inputs + decoys (mirrors buildSendTransaction).
    const amount = 1_000_000_000n
    const target = amount + fee
    const sorted = [...owned].sort((a, b) => (BigInt(b.amount) > BigInt(a.amount) ? 1 : -1))
    const inputs: typeof owned = []
    let total = 0n
    for (const o of sorted) {
      inputs.push(o)
      total += BigInt(o.amount)
      if (total >= target) break
    }
    expect(total).toBeGreaterThanOrEqual(target)

    // Decoys exclude only the real inputs + the all-zero genesis placeholder
    // (decoys may be the wallet's own other outputs; see send.ts rationale).
    const inputKeys = new Set(inputs.map((o) => o.targetKey))
    const decoyPool = candidates.filter(
      (c) => !inputKeys.has(c.targetKey) && !/^0+$/.test(c.targetKey),
    )
    expect(decoyPool.length).toBeGreaterThanOrEqual(ringSize - 1)

    const spendInputs = inputs.map((input, i) => {
      const decoys = []
      for (let j = 0; j < ringSize - 1; j++) {
        decoys.push(decoyPool[(i * (ringSize - 1) + j) % decoyPool.length])
      }
      return {
        target_key: input.targetKey,
        public_key: input.publicKey,
        amount: BigInt(input.amount),
        subaddress_index: BigInt(input.subaddressIndex),
        decoys: decoys.map((d) => ({
          target_key: d.targetKey,
          public_key: d.publicKey,
          amount: BigInt(d.amount),
        })),
      }
    })

    // 5. Build + CLSAG-sign in wasm (recipient = our own address).
    const recipientKeys = parseAddress(deriveAddress(mnemonic, 'testnet'))
    const txHex = wasm.buildAndSign({
      spendPrivateKey: keys.spendPrivateKey,
      viewPrivateKey: keys.viewPrivateKey,
      inputs: spendInputs,
      recipient: {
        spend_public_key: toHex(recipientKeys.spendPublic),
        view_public_key: toHex(recipientKeys.viewPublic),
      },
      amount,
      fee,
      createdAtHeight: height,
    })
    expect(txHex.length).toBeGreaterThan(0)

    // 6. Submit to the real node and require acceptance.
    const result = await rpc<{ txHash: string }>('tx_submit', { tx_hex: txHex })
    expect(result.txHash).toBeTruthy()
    // eslint-disable-next-line no-console
    console.log('NODE ACCEPTED wallet-built tx, txHash =', result.txHash)
  }, 60_000)
})
