import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

import { describe, expect, it } from 'vitest'

import type { SignRequest, WasmSigner } from '../src/index'

const here = dirname(fileURLToPath(import.meta.url))
const pkgDir = join(here, '..', 'pkg')
const wasmGlue = join(pkgDir, 'bth_wasm_signer.js')
const wasmBin = join(pkgDir, 'bth_wasm_signer_bg.wasm')

const wasmBuilt = existsSync(wasmGlue) && existsSync(wasmBin)

/**
 * Load the `--target web` wasm-pack module in a Node (vitest) context by
 * handing the init function the raw wasm bytes (the web glue accepts a
 * `BufferSource`/`WebAssembly.Module`, avoiding the `fetch`/URL path that only
 * works in a browser).
 */
async function loadSignerNode(): Promise<WasmSigner> {
  const mod = (await import(/* @vite-ignore */ wasmGlue)) as {
    default: (init: { module_or_path: BufferSource }) => Promise<unknown>
    buildAndSign: (request: unknown) => string
    scanOwnedOutputs: (request: unknown) => unknown
    computeOwnedOutputKeyImages: (request: unknown) => unknown
    ringSize: () => number
    minFee: () => bigint
  }
  const bytes = readFileSync(wasmBin)
  await mod.default({ module_or_path: bytes })
  return {
    buildAndSign: (request: SignRequest) => mod.buildAndSign(request),
    scanOwnedOutputs: (request) => mod.scanOwnedOutputs(request) as ReturnType<WasmSigner['scanOwnedOutputs']>,
    computeOwnedOutputKeyImages: (request) =>
      mod.computeOwnedOutputKeyImages(request) as ReturnType<
        WasmSigner['computeOwnedOutputKeyImages']
      >,
    ringSize: () => mod.ringSize(),
    minFee: () => mod.minFee(),
  }
}

const maybe = wasmBuilt ? describe : describe.skip

maybe('wasm signer (requires build:wasm)', () => {
  it('exposes network constants matching the protocol', async () => {
    const signer = await loadSignerNode()
    // The node requires a CLSAG ring size of 20.
    expect(signer.ringSize()).toBe(20)
    // Minimum fee is 0.0001 BTH = 100_000_000 picocredits.
    expect(signer.minFee()).toBe(100_000_000n)
  })

  it('rejects a request with insufficient funds with a clear error', async () => {
    const signer = await loadSignerNode()
    const ringSize = signer.ringSize()

    // 32-byte hex zero key placeholders. We only need the request to pass
    // structural parsing far enough to hit the balance check; the inputs sum
    // (1) is far below amount + fee, so it must fail with "insufficient funds".
    const zero = '00'.repeat(32)
    const decoys = Array.from({ length: ringSize - 1 }, () => ({
      target_key: zero,
      public_key: zero,
      amount: 1,
    }))

    // A valid Ristretto private/public key is required for parsing to succeed;
    // use the canonical Ristretto basepoint-derived test vector is overkill —
    // instead assert that an obviously underfunded request is rejected. We pass
    // syntactically valid 32-byte hex; the signer fails on the balance check
    // (insufficient funds) before any crypto that would reject zero keys,
    // because input/amount validation happens first.
    const request: SignRequest = {
      spendPrivateKey: zero,
      viewPrivateKey: zero,
      inputs: [
        {
          target_key: zero,
          public_key: zero,
          amount: 1,
          subaddress_index: 0,
          decoys,
        },
      ],
      recipient: { spend_public_key: zero, view_public_key: zero },
      amount: 5_000_000_000,
      fee: 100_000_000,
      createdAtHeight: 1000,
    }

    expect(() => signer.buildAndSign(request)).toThrow(/insufficient funds/)
  })

  it('rejects a fee below the network minimum', async () => {
    const signer = await loadSignerNode()
    const zero = '00'.repeat(32)
    const request: SignRequest = {
      spendPrivateKey: zero,
      viewPrivateKey: zero,
      inputs: [
        { target_key: zero, public_key: zero, amount: 10_000_000_000, subaddress_index: 0, decoys: [] },
      ],
      recipient: { spend_public_key: zero, view_public_key: zero },
      amount: 5_000_000_000,
      fee: 1, // below MIN_TX_FEE
      createdAtHeight: 1000,
    }
    expect(() => signer.buildAndSign(request)).toThrow(/below minimum/)
  })

  it('scanOwnedOutputs returns empty for outputs not owned by the account', async () => {
    const signer = await loadSignerNode()
    // The Ristretto basepoint compressed encoding is a valid public key; reuse
    // it as both target/public so parsing succeeds. The account keys below do
    // not own this output, so the node-identical ownership check returns none.
    const basepoint =
      'e2f2ae0a6abc4e71a884a961c500515f58e30b6aa582dd8db6a65945e08d2d76'
    const owned = signer.scanOwnedOutputs({
      spendPrivateKey: '01'.repeat(32),
      viewPrivateKey: '02'.repeat(32),
      outputs: [{ targetKey: basepoint, publicKey: basepoint, amount: 1000 }],
    })
    expect(owned).toEqual([])
  })
})
