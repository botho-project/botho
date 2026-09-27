import { afterEach, describe, expect, it } from 'vitest'

import {
  buildSendTransaction,
  resetSigner,
  setSigner,
  type BuildSendParams,
  type SignRequest,
  type WasmSigner,
} from '../src/index'

const INPUT_KEY = 'a'.repeat(64)
const DECOY_KEY = 'c'.repeat(64)
const PUB_KEY = 'b'.repeat(64)
const KEM_PUBLIC = 'ee'.repeat(1184)

/**
 * A fake {@link WasmSigner} that returns a fixed owned output as spendable and
 * records the last `buildAndSign` request. Ring size is 2 so a single decoy
 * suffices.
 */
function fakeSigner(): { signer: WasmSigner; lastRequest: () => SignRequest | null } {
  let captured: SignRequest | null = null
  const owned = {
    targetKey: INPUT_KEY,
    publicKey: PUB_KEY,
    amount: 1_000_000_000_000n,
    subaddressIndex: 0n,
  }
  const signer = {
    ringSize: () => 2,
    minFee: () => 100_000_000n,
    scanOwnedOutputs: () => [owned],
    computeOwnedOutputKeyImages: () => [{ ...owned, keyImage: 'ff'.repeat(32) }],
    buildAndSign: (request: SignRequest) => {
      captured = request
      return 'deadbeef'
    },
    derivePqPublicKeysFromSeed: () => ({ kemPublicKey: '', dsaPublicKey: '' }),
    deriveAddressFromSeed: () => '',
  } as unknown as WasmSigner
  return { signer, lastRequest: () => captured }
}

function params(bridgeDepositMemo?: string): BuildSendParams {
  return {
    keys: { spendPrivateKey: '11'.repeat(32), viewPrivateKey: '22'.repeat(32) },
    recipient: {
      spend_public_key: '33'.repeat(32),
      view_public_key: '44'.repeat(32),
      kem_public_key: KEM_PUBLIC,
    },
    senderKemPublicKey: KEM_PUBLIC,
    amount: 500_000_000_000n,
    fee: 100_000_000n,
    bridgeDepositMemo,
    rpc: {
      getChainHeight: async () => 100,
      getOutputs: async () => [
        { targetKey: INPUT_KEY, publicKey: PUB_KEY, amount: 1_000_000_000_000n },
        { targetKey: DECOY_KEY, publicKey: PUB_KEY, amount: 1_000_000_000_000n },
      ],
      areKeyImagesSpent: async (keyImages) =>
        keyImages.map((keyImage) => ({
          keyImage,
          spent: false,
          spentHeight: null,
          pending: false,
        })),
    },
  }
}


describe('legacy lottery payout exclusion', () => {
  afterEach(() => resetSigner())

  it('does not select a payout-only alias as a decoy', async () => {
    const { signer, lastRequest } = fakeSigner()
    setSigner(signer)
    const request = params()
    const ordinary = await request.rpc.getOutputs(0, 100)
    request.rpc.getOutputs = async () => [ordinary[0],
      { ...ordinary[1], targetKey: 'd'.repeat(64), lottery: true }, ordinary[1]]
    await buildSendTransaction(request)
    expect(lastRequest()?.inputs[0].decoys.map((decoy) => decoy.target_key)).toEqual([DECOY_KEY])
  })

  it('reports insufficient decoys without counting legacy payouts', async () => {
    const { signer, lastRequest } = fakeSigner()
    setSigner(signer)
    const request = params()
    const ordinary = await request.rpc.getOutputs(0, 100)
    request.rpc.getOutputs = async () => [ordinary[0], { ...ordinary[1], lottery: true }]
    await expect(buildSendTransaction(request)).rejects.toThrow('Not enough decoys')
    expect(lastRequest()).toBeNull()
  })

  it('explicitly rejects a selected legacy payout before signing', async () => {
    const { signer, lastRequest } = fakeSigner()
    setSigner(signer)
    const request = params()
    const ordinary = await request.rpc.getOutputs(0, 100)
    request.rpc.getOutputs = async () => [{ ...ordinary[0], lottery: true }, ordinary[1]]
    await expect(buildSendTransaction(request)).rejects.toThrow('Legacy lottery payouts')
    expect(lastRequest()).toBeNull()
  })

  it('selects ordinary funds when a larger owned payout is also present', async () => {
    const { signer, lastRequest } = fakeSigner()
    const ordinary = signer.scanOwnedOutputs({ spendPrivateKey: '', viewPrivateKey: '', outputs: [] })[0]
    const payout = { ...ordinary, targetKey: 'd'.repeat(64), amount: 2_000_000_000_000n, outputIndex: 1 }
    signer.scanOwnedOutputs = () => [payout, ordinary]
    signer.computeOwnedOutputKeyImages = () => [payout, ordinary].map((output, i) => ({ ...output, keyImage: String(i).repeat(64) }))
    setSigner(signer)
    const request = params()
    const candidates = await request.rpc.getOutputs(0, 100)
    request.rpc.getOutputs = async () => [{ ...payout, lottery: true }, ...candidates]
    await buildSendTransaction(request)
    expect(lastRequest()?.inputs.map((input) => input.target_key)).toEqual([INPUT_KEY])
  })

  it('allows the original winner when an alias has the same key but a different amount', async () => {
    const { signer, lastRequest } = fakeSigner()
    setSigner(signer)
    const request = params()
    const ordinary = await request.rpc.getOutputs(0, 100)
    request.rpc.getOutputs = async () => [...ordinary,
      { ...ordinary[0], amount: 20_000_000n, outputIndex: 1, lottery: true }]
    await buildSendTransaction(request)
    expect(lastRequest()).not.toBeNull()
  })
  it('keeps the original decoy envelope when an unflagged equal-target alias follows', async () => {
    const { signer, lastRequest } = fakeSigner()
    setSigner(signer)
    const request = params()
    const ordinary = await request.rpc.getOutputs(0, 100)
    request.rpc.getOutputs = async () => [...ordinary, { ...ordinary[1], amount: 20n }]
    await buildSendTransaction(request)
    expect(lastRequest()?.inputs[0].decoys).toEqual([{
      target_key: DECOY_KEY, public_key: PUB_KEY, amount: 1_000_000_000_000n,
    }])
  })

  it('does not count repeated targets toward the minimum decoy count', async () => {
    const { signer, lastRequest } = fakeSigner()
    signer.ringSize = () => 3
    setSigner(signer)
    const request = params()
    const ordinary = await request.rpc.getOutputs(0, 100)
    request.rpc.getOutputs = async () => [...ordinary, { ...ordinary[1] }]
    await expect(buildSendTransaction(request)).rejects.toThrow('Not enough decoys')
    expect(lastRequest()).toBeNull()
  })

  it('rejects an owned unflagged alias with a different amount', async () => {
    const { signer, lastRequest } = fakeSigner()
    const ordinary = signer.scanOwnedOutputs({ spendPrivateKey: '', viewPrivateKey: '', outputs: [] })[0]
    const alias = { ...ordinary, amount: 2_000_000_000_000n }
    signer.scanOwnedOutputs = () => [alias]
    signer.computeOwnedOutputKeyImages = () => [{ ...alias, keyImage: 'ff'.repeat(32) }]
    setSigner(signer)
    const request = params()
    const candidates = await request.rpc.getOutputs(0, 100)
    request.rpc.getOutputs = async () => [...candidates, alias]
    await expect(buildSendTransaction(request)).rejects.toThrow('Legacy lottery payouts')
    expect(lastRequest()).toBeNull()
  })

})
