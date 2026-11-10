import { afterEach, describe, expect, it, vi } from 'vitest'
import { isFeeAboveApprovedError, setSigner, resetSigner, type WasmSigner } from '@botho/wasm-signer'
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
  outputs?: Array<{ targetKey: string; publicKey: string; amount: bigint; height?: number }>
  spentKeyImages?: Set<string>
  submitOk?: boolean
  connected?: boolean
  /** The node's fee floor for every draft (`estimateFee` tx_hex, #2001). */
  nodeFloor?: bigint
}

function stubAdapter(opts: StubOpts = {}): RemoteNodeAdapter {
  const {
    outputs = [],
    spentKeyImages = new Set(),
    submitOk = true,
    connected = true,
    nodeFloor = 0n,
  } = opts
  const headroom = nodeFloor / 64n
  const atLeastMin = (fee: bigint) => (fee > MIN_TX_FEE ? fee : MIN_TX_FEE)
  return {
    isConnected: () => connected,
    getBlockHeight: async () => 100,
    estimateFee: async () => ({ fee: MIN_TX_FEE, clusterFactorDisplay: '1.00x' }),
    // The fake signer's tx hex does not encode its fee, so the quote omits
    // `txFee` and the orchestrator compares its own signed fee.
    estimateDraftFee: async () => ({
      minimumFee: String(atLeastMin(nodeFloor)),
      consensusFeeFloor: String(nodeFloor),
      relayFeeFloor: String(nodeFloor),
      settleFee: String(atLeastMin(nodeFloor + headroom)),
      resignFee: String(atLeastMin(nodeFloor + 2n * headroom)),
    }),
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
    // Need >= ringSize-1 = 2 decoys on chain besides the input, with block
    // heights for age-band decoy selection (#1693).
    const adapter = stubAdapter({
      outputs: [
        { targetKey: 'eph-1', publicKey: 'pk1', amount: gross, height: 50 },
        { targetKey: 'decoy-1', publicKey: 'pk2', amount: 1n, height: 50 },
        { targetKey: 'decoy-2', publicKey: 'pk3', amount: 1n, height: 50 },
      ],
    })
    const result = await sweepEphemeral(adapter, EPH_MNEMONIC, DEST)
    expect(result.txHash).toBe('sweeptx123')
    expect(result.net).toBe(gross - MIN_TX_FEE)
  })

  /** A link whose node fee floor (2e9) is far above the scanned fee (#2001). */
  function floorAdapter() {
    const gross = 5_000_000_000_000n + SWEEP_FEE_RESERVE
    const nodeFloor = 2_000_000_074n
    const adapter = stubAdapter({
      outputs: [
        { targetKey: 'eph-1', publicKey: 'pk1', amount: gross, height: 50 },
        { targetKey: 'decoy-1', publicKey: 'pk2', amount: 1n, height: 50 },
        { targetKey: 'decoy-2', publicKey: 'pk3', amount: 1n, height: 50 },
      ],
      nodeFloor,
    })
    return { adapter, gross, required: nodeFloor + 2n * (nodeFloor / 64n) }
  }

  it('refuses a node fee floor above the shown fee without a prompt; nothing submitted (#2001)', async () => {
    setSigner(fakeSigner())
    const { adapter, required } = floorAdapter()
    const err = await sweepEphemeral(adapter, EPH_MNEMONIC, DEST).catch((e: unknown) => e)
    expect(isFeeAboveApprovedError(err)).toBe(true)
    expect((err as { requiredFee: bigint }).requiredFee).toBe(required)
    expect(adapter.submitTransaction).not.toHaveBeenCalled()
  })

  it('re-asks with the required fee and sweeps the rest on approval (#2001)', async () => {
    setSigner(fakeSigner())
    const { adapter, gross, required } = floorAdapter()
    const confirmFee = vi.fn(async () => true)
    const result = await sweepEphemeral(adapter, EPH_MNEMONIC, DEST, {
      approvedFee: MIN_TX_FEE,
      confirmFee,
    })
    expect(confirmFee).toHaveBeenCalledWith(required, MIN_TX_FEE)
    expect(result.net).toBe(gross - required)
    expect(adapter.submitTransaction).toHaveBeenCalledTimes(1)
  })

  it('a declined re-confirmation submits nothing (#2001)', async () => {
    setSigner(fakeSigner())
    const { adapter } = floorAdapter()
    await expect(
      sweepEphemeral(adapter, EPH_MNEMONIC, DEST, {
        confirmFee: async () => false,
        onReject: () => new Error('fee not approved'),
      }),
    ).rejects.toThrow('fee not approved')
    expect(adapter.submitTransaction).not.toHaveBeenCalled()
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
        { targetKey: 'eph-1', publicKey: 'pk1', amount: gross, height: 50 },
        { targetKey: 'decoy-1', publicKey: 'pk2', amount: 1n, height: 50 },
        { targetKey: 'decoy-2', publicKey: 'pk3', amount: 1n, height: 50 },
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
