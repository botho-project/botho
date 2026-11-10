import { afterEach, describe, expect, it } from 'vitest'

import {
  MAX_FEE_PASSES,
  buildSendTransaction,
  parseDraftFeeQuote,
  quoteDraftFeeVia,
  resetSigner,
  setSigner,
  type BuildSendParams,
  type DraftFeeQuote,
  type SignRequest,
  type WasmSigner,
} from '../src/index'

/**
 * #2001: since protocol 10.0.0 a spend owes the class-exit charge from block 1
 * (#1685) and the import floor read from its inputs (#1687). The web wallet,
 * the snap and claim links all send through `buildSendTransaction`, which
 * signs, quotes the draft with the node's `estimateFee` (`tx_hex`), and
 * re-signs at `resignFee` until the fee settles. A FAKE signer records each
 * `buildAndSign` request; the fake node refuses a draft below its floor, as
 * the mempool does.
 */

const INPUT_KEY = 'a'.repeat(64)
const DECOY_KEY = 'c'.repeat(64)
const PUB_KEY = 'b'.repeat(64)
const KEM_PUBLIC = 'ee'.repeat(1184)
const MIN_TX_FEE = 100_000_000n
// The stress_node 10.0.0 case: a ~2e9 floor against a MIN_TX_FEE estimate.
const FLOOR = 2_000_000_074n

function fakeSigner(): { signer: WasmSigner; requests: SignRequest[] } {
  const requests: SignRequest[] = []
  const owned = {
    targetKey: INPUT_KEY,
    publicKey: PUB_KEY,
    amount: 1_000_000_000_000n,
    subaddressIndex: 0n,
  }
  const signer = {
    ringSize: () => 2,
    minFee: () => MIN_TX_FEE,
    scanOwnedOutputs: () => [owned],
    computeOwnedOutputKeyImages: () => [{ ...owned, keyImage: 'ff'.repeat(32) }],
    // The tx hex encodes the signed fee so the fake node can read it back.
    buildAndSign: (request: SignRequest) => {
      requests.push(request)
      return BigInt(request.fee).toString(16).padStart(16, '0')
    },
    derivePqPublicKeysFromSeed: () => ({ kemPublicKey: '', dsaPublicKey: '' }),
    deriveAddressFromSeed: () => '',
  } as unknown as WasmSigner
  return { signer, requests }
}

/** The node's quote (`bth_cluster_tax::settle_fee` / `resign_fee`). */
function nodeQuote(txHex: string, floor: bigint): Record<string, unknown> {
  const headroom = floor / 64n
  const max = (a: bigint, b: bigint) => (a > b ? a : b)
  return {
    minimumFee: Number(max(floor, MIN_TX_FEE)),
    consensusFeeFloor: Number(floor),
    relayFeeFloor: Number(floor),
    settleFee: Number(max(floor + headroom, MIN_TX_FEE)),
    resignFee: Number(max(floor + 2n * headroom, MIN_TX_FEE)),
    txFee: Number(BigInt('0x' + txHex)),
  }
}

/** A node whose fee floor for every draft is `floor`; it refuses underpayment. */
function fakeNode(floor: bigint) {
  const quoted: bigint[] = []
  const quoteDraftFee = quoteDraftFeeVia(async (method, params) => {
    expect(method).toBe('estimateFee')
    const txHex = params.tx_hex as string
    quoted.push(BigInt('0x' + txHex))
    return nodeQuote(txHex, floor)
  })
  const submit = (txHex: string) => {
    if (BigInt('0x' + txHex) < (floor > MIN_TX_FEE ? floor : MIN_TX_FEE)) {
      throw new Error('Failed to add transaction: Fee too low')
    }
    return 'accepted'
  }
  return { quoted, quoteDraftFee, submit }
}

function params(quoteDraftFee?: (txHex: string) => Promise<DraftFeeQuote>): BuildSendParams {
  return {
    keys: { spendPrivateKey: '11'.repeat(32), viewPrivateKey: '22'.repeat(32) },
    recipient: {
      spend_public_key: '33'.repeat(32),
      view_public_key: '44'.repeat(32),
      kem_public_key: KEM_PUBLIC,
    },
    senderKemPublicKey: KEM_PUBLIC,
    amount: 500_000_000_000n,
    fee: MIN_TX_FEE,
    rpc: {
      // Epoch 0, where 10.0.0 already prices the exit charge.
      getChainHeight: async () => 100,
      getOutputs: async () => [
        { targetKey: INPUT_KEY, publicKey: PUB_KEY, amount: 1_000_000_000_000n, height: 50 },
        { targetKey: DECOY_KEY, publicKey: PUB_KEY, amount: 1_000_000_000_000n, height: 50 },
      ],
      areKeyImagesSpent: async (keyImages) =>
        keyImages.map((keyImage) => ({ keyImage, spent: false, spentHeight: null, pending: false })),
      ...(quoteDraftFee ? { quoteDraftFee } : {}),
    },
  }
}

describe('buildSendTransaction prices against the node fee floor (#2001)', () => {
  afterEach(() => resetSigner())

  it('an underpaying epoch-0 spend is refused; the quoted re-sign is accepted', async () => {
    const { signer, requests } = fakeSigner()
    setSigner(signer)
    const node = fakeNode(FLOOR)
    // The pre-#2001 path: one signature at the local estimate. Refused.
    const legacy = await buildSendTransaction(params())
    expect(legacy.fee).toBe(MIN_TX_FEE)
    expect(() => node.submit(legacy.txHex)).toThrow('Fee too low')

    requests.length = 0
    const priced = await buildSendTransaction(params(node.quoteDraftFee))
    const resign = FLOOR + 2n * (FLOOR / 64n)
    expect(requests.map((r) => BigInt(r.fee))).toEqual([MIN_TX_FEE, resign])
    expect(node.quoted).toEqual([MIN_TX_FEE, resign])
    expect(priced.fee).toBe(resign)
    expect(node.submit(priced.txHex)).toBe('accepted')
  })

  it('a background spend settles at the relay minimum in one pass', async () => {
    const { signer, requests } = fakeSigner()
    setSigner(signer)
    const node = fakeNode(3_960n)
    const priced = await buildSendTransaction(params(node.quoteDraftFee))
    expect(requests).toHaveLength(1)
    expect(priced.fee).toBe(MIN_TX_FEE)
  })

  it('gives up after MAX_FEE_PASSES when the floor keeps rising', async () => {
    const { signer, requests } = fakeSigner()
    setSigner(signer)
    let floor = FLOOR
    const quoteDraftFee = quoteDraftFeeVia(async (_m, p) => {
      floor *= 2n
      return nodeQuote(p.tx_hex as string, floor)
    })
    await expect(buildSendTransaction(params(quoteDraftFee))).rejects.toThrow(
      'Could not settle a fee',
    )
    expect(requests).toHaveLength(MAX_FEE_PASSES)
  })

  it('parses node quotes through BigInt and refuses a node without the draft quote', () => {
    const q = parseDraftFeeQuote({
      minimumFee: 1,
      consensusFeeFloor: '1',
      relayFeeFloor: 1,
      settleFee: '18446744073709551615',
      resignFee: 2,
    })
    expect(q.settleFee).toBe(18446744073709551615n)
    expect(q.txFee).toBeUndefined()
    // A pre-#2001 node answers the legacy preview shape.
    expect(() => parseDraftFeeQuote({ minimumFee: 1, recommendedFee: 1 })).toThrow(
      'consensusFeeFloor',
    )
  })
})
