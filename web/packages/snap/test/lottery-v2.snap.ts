/**
 * V2 lottery payout rows in the Snap (#1645, P8 of #1286). PURE-LOGIC tests
 * (no `installSnap`, no wasm), the same pattern as `units.snap.ts`.
 *
 * The Snap reuses `@botho/wasm-signer`'s scan / key-image / send code, which
 * discovers and spends a V2 payout through its `lotteryContext` (covered by
 * `wasm-signer/test/lottery-v2-wasm.test.ts` against the compiled WASM). What
 * the Snap itself owns is (1) decoding `chain_getOutputs` rows without losing
 * the V2 fields or the legacy lottery flag, and (2) persisting a discovered
 * payout's context so balance/spend recovery still works after a resume.
 */

import { describe, expect, it } from '@jest/globals';

import { getOutputsWithMeta, makeSendRpc, type NodeCall } from '../src/node';
import { toOwnedOutput, toPersistedOwnedOutput } from '../src/state';

const context = { version: 2, baseIndex: 3, tweak: '4b'.repeat(32), ordinal: 0, height: 90 };
const ordinary = {
  txHash: 'ab'.repeat(32),
  outputIndex: 3,
  cryptoOutputIndex: 3,
  targetKey: 'cd'.repeat(32),
  publicKey: 'ef'.repeat(32),
  amountCommitment: '0100000000000000',
  clusterTags: [[7, 1000]],
  kemCiphertext: null,
};
const coinbase = { ...ordinary, outputIndex: 0xffffffff, cryptoOutputIndex: 0, coinbase: true };
const payout = {
  ...ordinary,
  txHash: '0b'.repeat(32),
  outputIndex: 1,
  targetKey: 'aa'.repeat(32),
  lottery: true,
  lotteryContext: context,
  winnerOutpoint: `${'ab'.repeat(32)}:3`,
};
const { cryptoOutputIndex: _none, ...legacyBase } = ordinary;
const legacy = { ...legacyBase, outputIndex: 2, lottery: true };

const call: NodeCall = (async (method: string) => {
  if (method !== 'chain_getOutputs') throw new Error(`unexpected ${method}`);
  return [{ height: 90, outputs: [ordinary, coinbase, payout, legacy] }];
}) as NodeCall;

describe('chain_getOutputs V2 lottery rows (#1645)', () => {
  it('keeps the V2 context, legacy flag and crypto index in both fetchers', async () => {
    const flat = await makeSendRpc(call).getOutputs(0, 90);
    const withMeta = await getOutputsWithMeta(call)(0, 90);
    for (const rows of [flat, withMeta]) {
      expect(rows.map((r) => r.outputIndex)).toStrictEqual([3, 0, 3, 2]);
      expect(rows[0].lotteryContext).toBeUndefined();
      expect(rows[2]).toMatchObject({
        lottery: true,
        lotteryContext: context,
        winnerOutpoint: `${'ab'.repeat(32)}:3`,
        amount: 1n,
      });
      // Legacy (context-less) payouts stay flagged so the shared orchestrator
      // keeps them out of inputs and decoys (#1450).
      expect(rows[3].lottery).toBe(true);
      expect(rows[3].lotteryContext).toBeUndefined();
    }
    expect(withMeta[2]).toMatchObject({ txHash: '0b'.repeat(32), height: 90 });
  });
});

describe('persisted V2 payouts (#1645)', () => {
  const owned = {
    targetKey: 'aa'.repeat(32),
    publicKey: 'ef'.repeat(32),
    amount: 5n,
    subaddressIndex: 0n,
    outputIndex: 3,
    kemCiphertext: null,
  };

  it('round-trips the derivation context through the persisted record', () => {
    const persisted = toPersistedOwnedOutput(
      { ...owned, lotteryContext: context },
      { blockHeight: 90, txHash: '0b'.repeat(32) },
    );
    expect(JSON.parse(JSON.stringify(persisted)).lotteryContext).toStrictEqual(context);
    expect(toOwnedOutput(persisted).lotteryContext).toStrictEqual(context);
  });

  it('leaves ordinary records (and pre-#1645 blobs) without a context key', () => {
    const persisted = toPersistedOwnedOutput(owned, { blockHeight: 90, txHash: 'ab'.repeat(32) });
    expect('lotteryContext' in persisted).toBe(false);
    expect('lotteryContext' in toOwnedOutput(persisted)).toBe(false);
  });
});
