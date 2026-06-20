import { afterEach, describe, expect, it } from 'vitest'

import {
  buildOwnedHistory,
  resetSigner,
  setSigner,
  type ChainOutputWithMeta,
  type HistoryRpc,
  type KeyImageSpentStatus,
  type WasmSigner,
} from '../src/index'

/**
 * Unit tests for the client-side transaction-history builder (#459).
 *
 * The bug being fixed: the node adapter's old `getTransactionHistory` had no
 * wallet keys, so it mapped EVERY on-chain output to a `{ receive, amount: 0 }`
 * entry — ~100+ bogus "received 0 BTH" rows. `buildOwnedHistory` instead reuses
 * the wasm OWNERSHIP scan (the same primitive balance uses) so only the user's
 * outputs appear, with their REAL decoded amounts.
 *
 * These tests inject a fully-controlled fake `WasmSigner` via `setSigner` (the
 * documented test seam), so we can assert the filtering / amount / spent / sort
 * logic deterministically without real crypto or a live node. A `targetKey`
 * starting with "owned" is treated as belonging to the wallet; everything else
 * (decoys / other users' outputs) is foreign and must NOT appear in history.
 */

/** A fake signer: "owns" any output whose targetKey starts with "owned". */
function fakeSigner(): WasmSigner {
  return {
    buildAndSign: () => {
      throw new Error('not used')
    },
    ringSize: () => 20,
    minFee: () => 100_000_000n,
    scanOwnedOutputs: ({ outputs }) =>
      outputs
        .filter((o) => o.targetKey.startsWith('owned'))
        .map((o) => ({
          targetKey: o.targetKey,
          publicKey: o.publicKey,
          amount: typeof o.amount === 'bigint' ? o.amount : BigInt(o.amount),
          subaddressIndex: 0n,
        })),
    // Key image is deterministically derived from the target key for the test.
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

const KEYS = { spendPrivateKey: '00'.repeat(32), viewPrivateKey: '11'.repeat(32) }

function rpcWith(
  outputs: ChainOutputWithMeta[],
  spent: Record<string, Partial<KeyImageSpentStatus>> = {},
): HistoryRpc {
  return {
    getChainHeight: async () => 1000,
    getOutputsWithMeta: async () => outputs,
    areKeyImagesSpent: async (keyImages) =>
      keyImages.map((keyImage) => ({
        keyImage,
        spent: spent[keyImage]?.spent ?? false,
        spentHeight: spent[keyImage]?.spentHeight ?? null,
        pending: spent[keyImage]?.pending ?? false,
      })),
  }
}

afterEach(() => resetSigner())

describe('buildOwnedHistory (#459)', () => {
  it('returns ONLY owned outputs with their real amounts — no 0-BTH spam', async () => {
    setSigner(fakeSigner())

    // 1 owned output among 100 foreign ones (the bug produced 101 zero-receives).
    const foreign: ChainOutputWithMeta[] = Array.from({ length: 100 }, (_, i) => ({
      txHash: `tx-foreign-${i}`,
      height: i + 1,
      targetKey: `foreign-${i}`,
      publicKey: `pk-foreign-${i}`,
      amount: 5_000_000_000n,
    }))
    const owned: ChainOutputWithMeta = {
      txHash: 'tx-owned',
      height: 42,
      targetKey: 'owned-1',
      publicKey: 'pk-owned-1',
      amount: 10_000_000_000_000n, // 10 BTH (picocredits)
    }

    const history = await buildOwnedHistory(KEYS, rpcWith([...foreign, owned]))

    // Exactly one entry, the owned receive — none of the 100 foreign outputs.
    expect(history).toHaveLength(1)
    expect(history[0]).toMatchObject({
      txHash: 'tx-owned',
      type: 'receive',
      amount: 10_000_000_000_000n,
      blockHeight: 42,
      spent: false,
    })
    // No entry has amount 0 (the old spam).
    expect(history.every((e) => e.amount > 0n)).toBe(true)
  })

  it('emits a spend entry (and flags the receive) for a spent owned output', async () => {
    setSigner(fakeSigner())

    const owned: ChainOutputWithMeta = {
      txHash: 'tx-owned',
      height: 10,
      targetKey: 'owned-1',
      publicKey: 'pk-owned-1',
      amount: 7_000_000_000_000n,
    }
    const history = await buildOwnedHistory(
      KEYS,
      rpcWith([owned], { 'ki-owned-1': { spent: true, spentHeight: 55 } }),
    )

    const receive = history.find((e) => e.type === 'receive')
    const spend = history.find((e) => e.type === 'spend')
    expect(receive).toMatchObject({ amount: 7_000_000_000_000n, spent: true })
    expect(spend).toMatchObject({ amount: 7_000_000_000_000n, blockHeight: 55 })
  })

  it('returns newest-first', async () => {
    setSigner(fakeSigner())

    const outputs: ChainOutputWithMeta[] = [
      { txHash: 't1', height: 5, targetKey: 'owned-a', publicKey: 'p', amount: 1n },
      { txHash: 't2', height: 90, targetKey: 'owned-b', publicKey: 'p', amount: 2n },
      { txHash: 't3', height: 30, targetKey: 'owned-c', publicKey: 'p', amount: 3n },
    ]
    const history = await buildOwnedHistory(KEYS, rpcWith(outputs))
    expect(history.map((e) => e.blockHeight)).toEqual([90, 30, 5])
  })

  it('returns empty when the wallet owns nothing (no spam)', async () => {
    setSigner(fakeSigner())
    const outputs: ChainOutputWithMeta[] = Array.from({ length: 50 }, (_, i) => ({
      txHash: `t-${i}`,
      height: i,
      targetKey: `foreign-${i}`,
      publicKey: 'p',
      amount: 9_999n,
    }))
    expect(await buildOwnedHistory(KEYS, rpcWith(outputs))).toEqual([])
  })
})
