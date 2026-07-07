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

// ============================================================================
// netOwnedHistory (#675)
// ============================================================================

import { netOwnedHistory, type HistoryEntry } from '../src/index'

/**
 * Unit tests for the per-event netting layer (#675). `buildOwnedHistory`
 * reports raw per-output facts; rendering them 1:1 produced duplicate React
 * keys (receive + spend of the same output share a txHash), one row per
 * output for multi-output receives, and "-<whole input> / +<change>" instead
 * of the net send amount. Pure function — no signer/rpc needed.
 */
describe('netOwnedHistory', () => {
  const recv = (txHash: string, amount: bigint, blockHeight: number): HistoryEntry => ({
    txHash,
    type: 'receive',
    amount,
    blockHeight,
    spent: false,
    spentHeight: null,
  })
  const spend = (
    txHash: string,
    amount: bigint,
    spentHeight: number | null,
  ): HistoryEntry => ({
    txHash,
    type: 'spend',
    amount,
    blockHeight: spentHeight ?? 0,
    spent: true,
    spentHeight,
  })

  it('produces unique row ids (no duplicate React keys)', () => {
    // One output received in tx A at h=5, later spent at h=9 with change
    // received in tx B at h=9 — the raw entries carry duplicate txHashes.
    const rows = netOwnedHistory([
      recv('txA', 1_000n, 5),
      spend('txA', 1_000n, 9),
      recv('txB', 800n, 9),
    ])
    const ids = rows.map((r) => r.id)
    expect(new Set(ids).size).toBe(ids.length)
  })

  it('nets a send against its same-block change', () => {
    const rows = netOwnedHistory([
      recv('txA', 1_000n, 5),
      spend('txA', 1_000n, 9),
      recv('txB', 800n, 9), // change
    ])
    const send = rows.find((r) => r.type === 'send')!
    expect(send.amount).toBe(200n) // 1000 spent - 800 change
    expect(send.blockHeight).toBe(9)
    expect(send.status).toBe('confirmed')
    // The change receive is folded into the send, the original receive stays.
    expect(rows.filter((r) => r.type === 'receive')).toHaveLength(1)
    expect(rows.find((r) => r.type === 'receive')!.amount).toBe(1_000n)
  })

  it('collapses a multi-output receive into one row with the summed amount', () => {
    const rows = netOwnedHistory([recv('txM', 300n, 4), recv('txM', 700n, 4)])
    expect(rows).toHaveLength(1)
    expect(rows[0].type).toBe('receive')
    expect(rows[0].amount).toBe(1_000n)
  })

  it('keeps a same-block incoming payment visible when it exceeds the spend', () => {
    // Spent 100 at h=9 but received 800 at h=9 (a genuine incoming payment,
    // not change) — netting would go negative, so nothing is swallowed.
    const rows = netOwnedHistory([
      recv('txA', 100n, 5),
      spend('txA', 100n, 9),
      recv('txB', 800n, 9),
    ])
    const send = rows.find((r) => r.type === 'send')!
    expect(send.amount).toBe(100n) // gross, un-netted
    const receives = rows.filter((r) => r.type === 'receive')
    expect(receives.map((r) => r.amount).sort()).toEqual([100n, 800n].sort())
  })

  it('surfaces an unmined spend as a single pending send row, sorted first', () => {
    const rows = netOwnedHistory([
      recv('txA', 1_000n, 5),
      spend('txA', 1_000n, null), // key image only pending in mempools
      recv('txOld', 50n, 2),
    ])
    expect(rows[0].type).toBe('send')
    expect(rows[0].status).toBe('pending')
    expect(rows[0].amount).toBe(1_000n)
    // Confirmed rows follow, newest first.
    expect(rows.slice(1).every((r) => r.status === 'confirmed')).toBe(true)
  })

  it('merges multi-input spends confirmed in the same block', () => {
    const rows = netOwnedHistory([
      recv('txA', 600n, 3),
      recv('txB', 500n, 4),
      spend('txA', 600n, 9),
      spend('txB', 500n, 9),
      recv('txC', 100n, 9), // change
    ])
    const sends = rows.filter((r) => r.type === 'send')
    expect(sends).toHaveLength(1)
    expect(sends[0].amount).toBe(1_000n) // 1100 spent - 100 change
  })

  it('returns an empty list for no entries', () => {
    expect(netOwnedHistory([])).toEqual([])
  })
})
