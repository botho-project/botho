/**
 * V2 lottery payouts through the ACTUAL compiled WASM signer (#1645, P8 of
 * #1286).
 *
 * `test/fixtures/lottery-v2-payout.json` is a real V2 payout (a hybrid source
 * output owned by the fixture wallet and the payout `lottery_v2::derive`
 * produces for it) in the frozen `chain_getOutputs` row shape (#1641). The
 * native Rust tests (`src/core/lottery_v2_tests.rs`) assert that native
 * `recover_with_context` recovery yields `expected.*`; this suite asserts the
 * compiled WASM yields the same values, so WASM == native for the same payout.
 * It then drives the wallet's real `buildSendTransaction` orchestrator against
 * the WASM signer to discover the payout and build a spend of it.
 *
 * Requires the `--target web` artifact (`pnpm --filter @botho/wasm-signer
 * build:wasm`); skipped otherwise. Web CI builds it and runs this file.
 */
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { afterEach, describe, expect, it } from 'vitest'

import {
  buildOwnedHistory,
  buildSendTransaction,
  resetSigner,
  setSigner,
  spendableBalance,
  type ChainOutput,
  type DecoyOutput,
  type OwnedOutput,
  type RecipientAddress,
  type SendRpc,
  type WasmSigner,
} from '../src/index'

const here = dirname(fileURLToPath(import.meta.url))
const pkgDir = join(here, '..', 'pkg')
const wasmGlue = join(pkgDir, 'bth_wasm_signer.js')
const wasmBin = join(pkgDir, 'bth_wasm_signer_bg.wasm')
const wasmBuilt = existsSync(wasmGlue) && existsSync(wasmBin)
// Web CI builds the artifact and sets this so the vector check cannot skip.
if (process.env.BOTHO_REQUIRE_WASM === '1' && !wasmBuilt) {
  throw new Error(`BOTHO_REQUIRE_WASM=1 but the wasm artifact is missing under ${pkgDir}`)
}

interface Vector {
  spendPrivateKey: string
  viewPrivateKey: string
  seed: string
  senderKemPublicKey: string
  recipient: RecipientAddress
  source: ChainOutput & { txHash: string }
  payout: ChainOutput & { txHash: string }
  decoys: ChainOutput[]
  expected: {
    baseTarget: string
    subaddressIndex: number
    sourceKeyImage: string
    payoutKeyImage: string
  }
}

const vector = JSON.parse(
  readFileSync(join(here, 'fixtures', 'lottery-v2-payout.json'), 'utf8'),
) as Vector

async function loadSignerNode(): Promise<WasmSigner> {
  const mod = (await import(/* @vite-ignore */ wasmGlue)) as {
    default: (init: { module_or_path: BufferSource }) => Promise<unknown>
    buildAndSign: (request: unknown) => string
    scanOwnedOutputs: (request: unknown) => unknown
    computeOwnedOutputKeyImages: (request: unknown) => unknown
    ringSize: () => number
    minFee: () => bigint
  }
  await mod.default({ module_or_path: readFileSync(wasmBin) })
  return {
    buildAndSign: (request) => mod.buildAndSign(request),
    scanOwnedOutputs: (request) =>
      mod.scanOwnedOutputs(request) as ReturnType<WasmSigner['scanOwnedOutputs']>,
    computeOwnedOutputKeyImages: (request) =>
      mod.computeOwnedOutputKeyImages(request) as ReturnType<
        WasmSigner['computeOwnedOutputKeyImages']
      >,
    ringSize: () => mod.ringSize(),
    minFee: () => mod.minFee(),
  }
}

const keys = {
  spendPrivateKey: vector.spendPrivateKey,
  viewPrivateKey: vector.viewPrivateKey,
  seed: vector.seed,
}

/**
 * Mock node: the fixture rows in chain order, nothing spent. Row `i` is
 * confirmed at block height `i` (as in the history mock below) so the
 * age-band decoy picker (#1693) can resolve ages: at tip 40 every decoy-pool
 * row is >= 21 blocks old (past the 10-block floor), and the pool (source + 19
 * decoys = 20 rows) covers the 19 decoys a ring of 20 needs.
 */
function rpc(rows: ChainOutput[]): SendRpc {
  return {
    getChainHeight: async () => 40,
    getOutputs: async () => rows.map((r, i) => ({ ...r, height: i })),
    areKeyImagesSpent: async (keyImages) =>
      keyImages.map((keyImage) => ({ keyImage, spent: false, spentHeight: null, pending: false })),
  }
}

const maybe = wasmBuilt ? describe : describe.skip

maybe('V2 lottery payouts in the compiled WASM signer (#1645)', () => {
  afterEach(() => resetSigner())

  it('discovers the payout and derives the same key image as native recovery', async () => {
    const signer = await loadSignerNode()
    const owned = signer.scanOwnedOutputs({
      ...keys,
      outputs: [vector.source, vector.payout, ...vector.decoys],
    })
    expect(owned.map((o) => o.targetKey)).toEqual([
      vector.source.targetKey,
      vector.payout.targetKey,
    ])
    const payout = owned[1]
    expect(BigInt(payout.subaddressIndex)).toBe(BigInt(vector.expected.subaddressIndex))
    expect(payout.lotteryContext).toEqual(vector.payout.lotteryContext)
    expect(owned[0].lotteryContext ?? null).toBeNull()

    const images = signer.computeOwnedOutputKeyImages({ ...keys, outputs: owned })
    expect(images.map((o) => o.keyImage)).toEqual([
      vector.expected.sourceKeyImage,
      vector.expected.payoutKeyImage,
    ])
  })

  it('finds no ownership for a forged tweak or a context-less payout row', async () => {
    const signer = await loadSignerNode()
    const ctx = vector.payout.lotteryContext!
    const flipped = (parseInt(ctx.tweak.slice(0, 2), 16) ^ 1).toString(16).padStart(2, '0')
    const forged = { ...vector.payout, lotteryContext: { ...ctx, tweak: flipped + ctx.tweak.slice(2) } }
    const contextLess = { ...vector.payout, lotteryContext: null }
    const wrongBase = { ...vector.payout, lotteryContext: { ...ctx, baseIndex: ctx.baseIndex + 1 } }
    expect(signer.scanOwnedOutputs({ ...keys, outputs: [forged, contextLess, wrongBase] })).toEqual([])
  })

  it('refuses to derive a key image under a forged context (x·G = P check)', async () => {
    const signer = await loadSignerNode()
    const [payout] = signer.scanOwnedOutputs({ ...keys, outputs: [vector.payout] })
    const ctx = payout.lotteryContext!
    const forged: OwnedOutput = { ...payout, lotteryContext: { ...ctx, baseIndex: ctx.baseIndex + 1 } }
    expect(() => signer.computeOwnedOutputKeyImages({ ...keys, outputs: [forged] })).toThrow(
      /V2 lottery payout/,
    )
  })

  it('builds and self-verifies a spend of the payout via buildAndSign', async () => {
    const signer = await loadSignerNode()
    const [payout] = signer.scanOwnedOutputs({ ...keys, outputs: [vector.payout] })
    const decoys: DecoyOutput[] = vector.decoys.map((d) => ({
      target_key: d.targetKey,
      public_key: d.publicKey,
      amount: BigInt(d.amount),
    }))
    const txHex = signer.buildAndSign({
      ...keys,
      inputs: [
        {
          target_key: payout.targetKey,
          public_key: payout.publicKey,
          amount: BigInt(payout.amount),
          subaddress_index: BigInt(payout.subaddressIndex),
          output_index: payout.outputIndex,
          kem_ciphertext: payout.kemCiphertext,
          lottery_context: payout.lotteryContext,
          decoys,
        },
      ],
      recipient: vector.recipient,
      senderKemPublicKey: vector.senderKemPublicKey,
      amount: 2_000_000_000_000n,
      fee: signer.minFee(),
      createdAtHeight: 40n,
    })
    // The signer verifies the produced ring signatures before returning.
    expect(txHex).toMatch(/^[0-9a-f]+$/)
    // The CLSAG input's key image (bincode-serialized inside the tx) is the payout's.
    expect(txHex).toContain(vector.expected.payoutKeyImage)
  })

  it('wallet orchestration: balance + history include the payout and a send spends it', async () => {
    const signer = await loadSignerNode()
    setSigner(signer)
    const rows: ChainOutput[] = [vector.source, ...vector.decoys, vector.payout]

    // Balance: the source and the payout are both owned and unspent.
    const balance = await spendableBalance(keys, rpc(rows))
    expect(balance).toBe(BigInt(vector.source.amount) + BigInt(vector.payout.amount))

    // History shows the payout as its own receive.
    const history = await buildOwnedHistory(keys, {
      getChainHeight: async () => 40,
      getOutputsWithMeta: async () =>
        rows.map((r, i) => ({ ...r, txHash: `${i}`.padStart(64, '0'), height: i })),
      areKeyImagesSpent: rpc(rows).areKeyImagesSpent,
    })
    expect(history.map((h) => h.amount)).toContain(BigInt(vector.payout.amount))

    // A send larger than the source can only be funded by the V2 payout.
    const result = await buildSendTransaction({
      keys,
      recipient: vector.recipient,
      senderKemPublicKey: vector.senderKemPublicKey,
      amount: 2_000_000_000_000n,
      fee: signer.minFee(),
      rpc: rpc(rows),
    })
    expect(result.inputs.map((i) => i.targetKey)).toEqual([vector.payout.targetKey])
    expect(result.inputs[0].lotteryContext).toEqual(vector.payout.lotteryContext)
    expect(result.txHex).toContain(vector.expected.payoutKeyImage)
  })
})
