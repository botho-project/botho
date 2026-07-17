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
 * #1037: `buildSendTransaction` must thread the optional, DEDICATED bridge
 * deposit memo (`bridgeDepositMemo`) through to the wasm signer's `buildAndSign`
 * request, so a BTH→wBTH bridge deposit carries the order memo the watcher
 * matches on. Crucially, this is a channel SEPARATE from any human free-text
 * note, so an ordinary send never routes text into the signer's strict 64-byte
 * validator. These tests use a FAKE signer (injected via {@link setSigner}) that
 * captures the `SignRequest` it is handed, so they assert the plumbing without
 * needing the compiled wasm artifact.
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

describe('buildSendTransaction bridge-deposit memo threading (#1037)', () => {
  afterEach(() => resetSigner())

  it('passes the bridge deposit memo into the buildAndSign request', async () => {
    const { signer, lastRequest } = fakeSigner()
    setSigner(signer)

    const orderMemo = 'deadbeef001122334455667788990011'.padEnd(128, '0')
    const { txHex } = await buildSendTransaction(params(orderMemo))

    expect(txHex).toBe('deadbeef')
    const req = lastRequest()
    expect(req).not.toBeNull()
    expect(req?.bridgeDepositMemo).toBe(orderMemo)
  })

  it('leaves bridgeDepositMemo undefined for an ordinary send', async () => {
    const { signer, lastRequest } = fakeSigner()
    setSigner(signer)

    await buildSendTransaction(params())

    expect(lastRequest()?.bridgeDepositMemo).toBeUndefined()
  })
})
