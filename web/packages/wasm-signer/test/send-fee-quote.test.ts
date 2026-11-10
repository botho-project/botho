import { afterEach, describe, expect, it } from 'vitest'

import {
  FeeAboveApprovedError,
  MAX_FEE_PASSES,
  buildSendTransaction,
  buildWithFeeConsent,
  isFeeAboveApprovedError,
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

function params(
  quoteDraftFee?: (txHex: string) => Promise<DraftFeeQuote>,
  maxFee?: bigint,
): BuildSendParams {
  return {
    ...(maxFee !== undefined ? { maxFee } : {}),
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
    // The user approved up to 1 BTH of fee: the loop may re-sign under it.
    const priced = await buildSendTransaction(params(node.quoteDraftFee, 1_000_000_000_000n))
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
    await expect(
      buildSendTransaction(params(quoteDraftFee, 1n << 62n)),
    ).rejects.toThrow('Could not settle a fee')
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

/**
 * #2001 Judge review: the node is not trusted with the fee. A send signs at
 * most the user-approved `maxFee`; a higher quote stops the loop with a typed
 * error BEFORE signing at it, and the UI must re-ask the user.
 */
describe('buildSendTransaction never signs above the approved fee (#2001)', () => {
  afterEach(() => resetSigner())

  it('a quote above maxFee aborts with the required fee and nothing to submit', async () => {
    const { signer, requests } = fakeSigner()
    setSigner(signer)
    const node = fakeNode(FLOOR)
    const submitted: string[] = []
    const err = await buildSendTransaction(params(node.quoteDraftFee))
      .then((r) => {
        submitted.push(r.txHex)
        return null
      })
      .catch((e: unknown) => e)
    expect(err).toBeInstanceOf(FeeAboveApprovedError)
    expect(isFeeAboveApprovedError(err)).toBe(true)
    const e = err as FeeAboveApprovedError
    expect(e.approvedFee).toBe(MIN_TX_FEE)
    expect(e.requiredFee).toBe(FLOOR + 2n * (FLOOR / 64n))
    // Only the draft at the approved fee was ever signed; nothing returned.
    expect(requests.map((r) => BigInt(r.fee))).toEqual([MIN_TX_FEE])
    expect(submitted).toEqual([])
  })

  it('an accepted re-confirmation signs at exactly the confirmed fee and settles', async () => {
    const { signer, requests } = fakeSigner()
    setSigner(signer)
    const node = fakeNode(FLOOR)
    const asked: Array<[bigint, bigint]> = []
    const { result, fee } = await buildWithFeeConsent({
      approvedFee: MIN_TX_FEE,
      build: (f) => buildSendTransaction({ ...params(node.quoteDraftFee), fee: f, maxFee: f }),
      reconfirm: async (required, approved) => {
        asked.push([required, approved])
        return true
      },
      onReject: () => new Error('rejected'),
    })
    const resign = FLOOR + 2n * (FLOOR / 64n)
    expect(asked).toEqual([[resign, MIN_TX_FEE]])
    expect(fee).toBe(resign)
    expect(result.fee).toBe(resign)
    // Never signed above what the user had approved at the time.
    expect(requests.map((r) => BigInt(r.fee))).toEqual([MIN_TX_FEE, resign])
    expect(node.submit(result.txHex)).toBe('accepted')
  })

  it('a declined re-confirmation throws the rejection and returns nothing', async () => {
    const { signer, requests } = fakeSigner()
    setSigner(signer)
    const node = fakeNode(FLOOR)
    await expect(
      buildWithFeeConsent({
        approvedFee: MIN_TX_FEE,
        build: (f) => buildSendTransaction({ ...params(node.quoteDraftFee), fee: f, maxFee: f }),
        reconfirm: async () => false,
        onReject: () => new Error('User rejected the higher fee'),
      }),
    ).rejects.toThrow('User rejected the higher fee')
    expect(requests.map((r) => BigInt(r.fee))).toEqual([MIN_TX_FEE])
  })

  it('a hostile quote (fee ~ the whole balance) is refused without reselecting inputs', async () => {
    const { signer, requests } = fakeSigner()
    setSigner(signer)
    // The wallet holds 1 BTH; the node demands ~0.99 BTH.
    const hostile = 990_000_000_000n
    const quoteDraftFee = quoteDraftFeeVia(async (_m, p) => nodeQuote(p.tx_hex as string, hostile))
    const approved = 200_000_000n
    await expect(
      buildSendTransaction({ ...params(quoteDraftFee), amount: 1_000_000n, fee: approved }),
    ).rejects.toSatisfy(
      (e: unknown) => isFeeAboveApprovedError(e) && e.approvedFee === approved && e.requiredFee > hostile,
    )
    expect(requests).toHaveLength(1)
    expect(BigInt(requests[0].fee)).toBe(approved)
  })

  it('a sweep never lowers the recipient amount below the approved fee', async () => {
    const { signer, requests } = fakeSigner()
    setSigner(signer)
    const node = fakeNode(FLOOR)
    const gross = 1_000_000_000_000n
    await expect(
      buildSendTransaction({
        ...params(node.quoteDraftFee),
        amount: gross - MIN_TX_FEE,
        fee: MIN_TX_FEE,
        subtractFeeFromAmount: true,
      }),
    ).rejects.toBeInstanceOf(FeeAboveApprovedError)
    expect(requests.map((r) => BigInt(r.amount))).toEqual([gross - MIN_TX_FEE])
  })

  it('refuses a seed fee above maxFee before signing anything', async () => {
    const { signer, requests } = fakeSigner()
    setSigner(signer)
    await expect(
      buildSendTransaction({ ...params(), fee: 2n * MIN_TX_FEE, maxFee: MIN_TX_FEE }),
    ).rejects.toBeInstanceOf(FeeAboveApprovedError)
    expect(requests).toHaveLength(0)
  })
})
