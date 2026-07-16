import { afterEach, describe, expect, it, vi } from 'vitest'
import { setSigner, resetSigner, type WasmSigner } from '@botho/wasm-signer'
import {
  deriveDefaultSubaddressPublicKeys,
  formatAddress,
  createClaimLinkMnemonic,
} from '@botho/core'

// Local test helper: real classical keys + placeholder PQ bytes of the correct
// v2 lengths (real ML-KEM/ML-DSA derivation lives in @botho/wasm-signer).
function deriveAddress(mnemonic: string): string {
  const { viewPublic, spendPublic } = deriveDefaultSubaddressPublicKeys(mnemonic, 0)
  return formatAddress(viewPublic, spendPublic, new Uint8Array(1184), new Uint8Array(1952), 'testnet')
}
import { scanEphemeral, sweepEphemeral, MIN_TX_FEE, SWEEP_FEE_RESERVE } from './claim-link-ops'
import type { RemoteNodeAdapter } from '@botho/adapters'

/**
 * Unit tests for the claim-link transaction ops (#460).
 *
 * Mirrors the `history.test.ts` seam: inject a fully-controlled fake
 * `WasmSigner` via `setSigner` and a stub adapter so we can assert the net/fee
 * math, the already-claimed / not-confirmed handling, and the sweep error paths
 * deterministically — no real crypto or live node.
 *
 * The fake signer "owns" any output whose targetKey starts with "eph". A
 * targetKey appended with ":spent" is reported spent by the adapter stub.
 */

function fakeSigner(): WasmSigner {
  return {
    buildAndSign: () => 'deadbeef',
    ringSize: () => 3,
    minFee: () => MIN_TX_FEE,
    // Placeholder v2-length PQ keys so `deriveKemPublicKey` (used by the sweep
    // path for change encapsulation, #978) resolves without real crypto.
    derivePqPublicKeysFromSeed: () => ({
      kemPublicKey: '00'.repeat(1184),
      dsaPublicKey: '00'.repeat(1952),
    }),
    scanOwnedOutputs: ({ outputs }) =>
      outputs
        .filter((o) => o.targetKey.startsWith('eph'))
        .map((o) => ({
          targetKey: o.targetKey,
          publicKey: o.publicKey,
          amount: typeof o.amount === 'bigint' ? o.amount : BigInt(o.amount),
          subaddressIndex: 0n,
        })),
    computeOwnedOutputKeyImages: ({ outputs }) =>
      outputs.map((o) => ({
        targetKey: o.targetKey,
        publicKey: o.publicKey,
        amount: o.amount,
        subaddressIndex: o.subaddressIndex,
        keyImage: `ki-${o.targetKey}`,
      })),
  }
}

interface StubOpts {
  outputs?: Array<{ targetKey: string; publicKey: string; amount: bigint }>
  spentKeyImages?: Set<string>
  submitOk?: boolean
  connected?: boolean
}

function stubAdapter(opts: StubOpts = {}): RemoteNodeAdapter {
  const {
    outputs = [],
    spentKeyImages = new Set(),
    submitOk = true,
    connected = true,
  } = opts
  return {
    isConnected: () => connected,
    getBlockHeight: async () => 100,
    estimateFee: async () => ({ fee: MIN_TX_FEE, clusterFactorDisplay: '1.00x' }),
    getRawOutputs: async () => outputs,
    areKeyImagesSpent: async (keyImages: string[]) =>
      keyImages.map((keyImage) => ({
        keyImage,
        spent: spentKeyImages.has(keyImage),
        spentHeight: spentKeyImages.has(keyImage) ? 50 : null,
        pending: false,
      })),
    submitTransaction: vi.fn(async () =>
      submitOk
        ? { success: true, txHash: 'sweeptx123' }
        : { success: false, error: 'double-spend detected' },
    ),
  } as unknown as RemoteNodeAdapter
}

const EPH_MNEMONIC = createClaimLinkMnemonic()

afterEach(() => resetSigner())

describe('scanEphemeral', () => {
  it('computes net = gross - fee for a funded ephemeral wallet', async () => {
    setSigner(fakeSigner())
    const gross = 5_000_000_000_000n + SWEEP_FEE_RESERVE
    const adapter = stubAdapter({
      outputs: [{ targetKey: 'eph-1', publicKey: 'pk1', amount: gross }],
    })
    const scan = await scanEphemeral(adapter, EPH_MNEMONIC)
    expect(scan.gross).toBe(gross)
    expect(scan.fee).toBe(MIN_TX_FEE)
    expect(scan.net).toBe(gross - MIN_TX_FEE)
  })

  it('reports zero when nothing is owned (not yet confirmed)', async () => {
    setSigner(fakeSigner())
    const adapter = stubAdapter({
      outputs: [{ targetKey: 'foreign-1', publicKey: 'pk', amount: 9n }],
    })
    const scan = await scanEphemeral(adapter, EPH_MNEMONIC)
    expect(scan.gross).toBe(0n)
    expect(scan.net).toBe(0n)
  })

  it('reports zero when the owned output is already spent (claimed)', async () => {
    setSigner(fakeSigner())
    const adapter = stubAdapter({
      outputs: [{ targetKey: 'eph-1', publicKey: 'pk1', amount: 5_000_000_000_000n }],
      spentKeyImages: new Set(['ki-eph-1']),
    })
    const scan = await scanEphemeral(adapter, EPH_MNEMONIC)
    expect(scan.gross).toBe(0n)
  })
})

describe('sweepEphemeral', () => {
  const DEST = deriveAddress(createClaimLinkMnemonic())

  it('sweeps a funded link and returns the net + tx hash', async () => {
    setSigner(fakeSigner())
    const gross = 5_000_000_000_000n + SWEEP_FEE_RESERVE
    // Need >= ringSize-1 = 2 decoys on chain besides the input.
    const adapter = stubAdapter({
      outputs: [
        { targetKey: 'eph-1', publicKey: 'pk1', amount: gross },
        { targetKey: 'decoy-1', publicKey: 'pk2', amount: 1n },
        { targetKey: 'decoy-2', publicKey: 'pk3', amount: 1n },
      ],
    })
    const result = await sweepEphemeral(adapter, EPH_MNEMONIC, DEST)
    expect(result.txHash).toBe('sweeptx123')
    expect(result.net).toBe(gross - MIN_TX_FEE)
  })

  it('throws "nothing to claim" when the link is empty / already claimed', async () => {
    setSigner(fakeSigner())
    const adapter = stubAdapter({
      outputs: [{ targetKey: 'eph-1', publicKey: 'pk1', amount: 5_000_000_000_000n }],
      spentKeyImages: new Set(['ki-eph-1']),
    })
    await expect(sweepEphemeral(adapter, EPH_MNEMONIC, DEST)).rejects.toThrow(/nothing to claim/i)
  })

  it('surfaces a double-spend submit failure (race loser)', async () => {
    setSigner(fakeSigner())
    const gross = 5_000_000_000_000n + SWEEP_FEE_RESERVE
    const adapter = stubAdapter({
      outputs: [
        { targetKey: 'eph-1', publicKey: 'pk1', amount: gross },
        { targetKey: 'decoy-1', publicKey: 'pk2', amount: 1n },
        { targetKey: 'decoy-2', publicKey: 'pk3', amount: 1n },
      ],
      submitOk: false,
    })
    await expect(sweepEphemeral(adapter, EPH_MNEMONIC, DEST)).rejects.toThrow(/double-spend/i)
  })

  it('throws when not connected', async () => {
    setSigner(fakeSigner())
    const adapter = stubAdapter({ connected: false })
    await expect(sweepEphemeral(adapter, EPH_MNEMONIC, DEST)).rejects.toThrow(/connected/i)
  })
})
