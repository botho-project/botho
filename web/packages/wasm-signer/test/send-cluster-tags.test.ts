import { afterEach, describe, expect, it } from 'vitest'

import {
  buildSendTransaction,
  resetSigner,
  setSigner,
  type BuildSendParams,
  type SignRequest,
  type WasmSigner,
} from '../src/index'

/**
 * #1687 (protocol 10.0.0): consensus rejects a spend whose outputs keep less
 * than 95% of a cluster's weight, so `buildSendTransaction` must pass each
 * input's stored cluster tags to the signer, which makes the outputs inherit
 * them. The ids travel as decimal strings from the node's lossless
 * `clusterTagsDec`: a cluster id is a full u64, which a JS number rounds.
 * A fake signer captures the `SignRequest`; no wasm artifact is needed.
 */

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
        // Block heights are required for age-band decoy selection (#1693).
        { targetKey: INPUT_KEY, publicKey: PUB_KEY, amount: 1_000_000_000_000n, height: 50 },
        { targetKey: DECOY_KEY, publicKey: PUB_KEY, amount: 1_000_000_000_000n, height: 50 },
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

// A coinbase cluster id: 8 bytes of a hash, far above 2^53.
const CLUSTER = '18364758544493064720'

describe('input cluster tags reach the signer (#1687)', () => {
  afterEach(() => resetSigner())

  async function send(input: Record<string, unknown>) {
    const { signer, lastRequest } = fakeSigner()
    setSigner(signer)
    const request = params()
    const rows = await request.rpc.getOutputs(0, 100)
    request.rpc.getOutputs = async () => [{ ...rows[0], ...input }, rows[1]]
    await buildSendTransaction(request)
    return lastRequest()
  }

  it('passes the lossless tags of the selected input', async () => {
    const request = await send({
      clusterTags: [[Number(CLUSTER), 950_000]],
      clusterTagsDec: [[CLUSTER, 950_000]],
    })
    expect(request?.inputs[0].cluster_tags).toEqual([[CLUSTER, 950_000]])
  })

  it('passes no tags for an untagged input', async () => {
    const request = await send({ clusterTags: [], clusterTagsDec: [] })
    expect(request?.inputs[0].cluster_tags).toEqual([])
  })

  it('fails closed on a tagged input served only in the lossy form', async () => {
    await expect(send({ clusterTags: [[Number(CLUSTER), 950_000]] })).rejects.toThrow(
      'clusterTagsDec',
    )
  })
})
