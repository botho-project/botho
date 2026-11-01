import { afterEach, describe, expect, it, vi } from 'vitest'
import { readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { buildSendTransaction, resetSigner, setSigner, type WasmSigner, type SignRequest } from '../src/index'

// Execute the real send.ts orchestration. Only RPC/ownership/signing boundaries
// are adapters: this measures ring membership, not WASM crypto or ownership.
const root = resolve(import.meta.dirname, '../../../..')
const fixtureRoot = resolve(root, 'scripts/research/ct-economics')
const config = JSON.parse(readFileSync(resolve(fixtureRoot, 'scenarios.json'), 'utf8'))
const pools: Record<string, number[][]> = JSON.parse(readFileSync(resolve(fixtureRoot, 'pools.json'), 'utf8'))
function key(id: number) {
  const bytes = Buffer.alloc(32)
  bytes.writeBigUInt64LE(BigInt(id))
  return bytes.toString('hex')
}
function id(k: string) { return Number(Buffer.from(k, 'hex').readBigUInt64LE()) }
// send.ts draws ring decoys with crypto.getRandomValues (#1693). Replace it
// with a seeded PRNG (mulberry32) ONLY here so the recorded web route stays a
// byte-stable research fixture; production always uses the real CSPRNG.
function seededGetRandomValues(seed: number) {
  let a = seed >>> 0
  return <T extends ArrayBufferView | null>(array: T): T => {
    const view = array as unknown as Uint32Array
    for (let i = 0; i < view.length; i++) {
      a = (a + 0x6d2b79f5) >>> 0
      let t = a
      t = Math.imul(t ^ (t >>> 15), t | 1)
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
      view[i] = (t ^ (t >>> 14)) >>> 0
    }
    return array
  }
}

describe('CT economics actual web selection route', () => {
  afterEach(() => { resetSigner(); vi.restoreAllMocks() })
  it('records all scenarios, including failures and second-input rotations', async () => {
    const report: Record<string, unknown> = {}
    for (const scenario of config.scenarios) {
      const rows: Record<string, unknown>[] = []
      for (const nInputs of [1, 2]) {
        let captured: SignRequest | undefined
        vi.spyOn(globalThis.crypto, 'getRandomValues').mockImplementation(
          seededGetRandomValues(config.seeds[0] * 2 + nInputs) as typeof globalThis.crypto.getRandomValues,
        )
        // Owned inputs sit at the scenario's real age; every row carries its
        // block height so send.ts can apply the native age band (#1693).
        const owned = Array.from({ length: nInputs }, (_, i) => ({
          targetKey: key(10_000 + i), publicKey: key(20_000 + i),
          amount: 1_000_000_000_000_000n, subaddressIndex: 0n,
          height: config.height - scenario.real_age,
        }))
        setSigner({
          ringSize: () => 20,
          minFee: () => 0n,
          scanOwnedOutputs: () => owned,
          computeOwnedOutputKeyImages: () => owned.map((o, i) => ({ ...o, keyImage: key(30_000 + i) })),
          buildAndSign: (request: SignRequest) => { captured = request; return 'deadbeef' },
        } as unknown as WasmSigner)
        const outputs = pools[scenario.name].map(([i, age]) => ({ targetKey: key(i), publicKey: key(i + 40_000), amount: 1_000_000_000_000n, height: config.height - age }))
        try {
          await buildSendTransaction({
            keys: { spendPrivateKey: key(50_000), viewPrivateKey: key(50_001) },
            recipient: { spend_public_key: key(50_002), view_public_key: key(50_003), kem_public_key: 'ee'.repeat(1184) },
            senderKemPublicKey: 'ee'.repeat(1184),
            amount: BigInt(nInputs) * 1_000_000_000_000_000n - 1n, fee: 0n,
            rpc: {
              getChainHeight: async () => config.height,
              getOutputs: async () => [...owned, ...outputs],
              areKeyImagesSpent: async (keys) => keys.map((keyImage) => ({ keyImage, spent: false, spentHeight: null, pending: false })),
            },
          })
          expect(captured?.inputs).toHaveLength(nInputs)
          const rings = captured!.inputs.map((input) => input.decoys.map((d) => id(d.target_key)))
          for (const ring of rings) {
            expect(ring).toHaveLength(19)
            expect(new Set(ring).size).toBe(19)
            expect(ring.every((i) => i < 10_000)).toBe(true)
          }
          rows.push({ inputs: nInputs, rings })
        } catch (error) {
          // Only the actual selector's explicit shortfall is an expected failure.
          expect(String(error)).toContain('Not enough decoys on chain')
          expect(pools[scenario.name].length).toBeLessThan(19)
          expect(captured).toBeUndefined()
          rows.push({ inputs: nInputs, error: 'insufficient_decoys' })
        }
      }
      report[scenario.name] = rows
    }
    expect(Object.keys(report)).toHaveLength(16)
    const serialized = JSON.stringify(report, null, 2) + '\n'
    const path = resolve(fixtureRoot, 'web-selection.json')
    if (process.env.CT_ECONOMICS_WRITE_WEB === '1') writeFileSync(path, serialized)
    else expect(JSON.parse(serialized)).toEqual(JSON.parse(readFileSync(path, 'utf8')))
  })
})
