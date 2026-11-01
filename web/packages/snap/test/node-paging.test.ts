/**
 * `chain_getOutputs` paging in the Snap's node adapters (#1656). PURE-LOGIC,
 * no SES / wasm — named `*.test.ts` so the workspace-root vitest run (which CI
 * executes) covers it; the `*.snap.ts` jest suites need a built bundle.
 *
 * The node clamps one call to 1000 blocks. The Snap's send/balance path asks
 * for `[0, tip]` and must not silently stop at the cap.
 */
import { describe, expect, it } from 'vitest';

import { getOutputsWithMeta, makeSendRpc, pagedOutputsCall, type NodeCall } from '../src/node';

/** A node with blocks `0..=tip`, one output each, capped at `cap` per call. */
function cappedNode(tip: number, cap: number) {
  const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
  const call = (async (method: string, params: Record<string, unknown>) => {
    calls.push({ method, params });
    if (method === 'node_getStatus') return { chainHeight: tip };
    if (method !== 'chain_getOutputs') throw new Error(`unexpected ${method}`);
    const from = params.start_height as number;
    const end = params.end_height as number;
    const blocks = [];
    for (let h = from; h <= Math.min(end, from + cap - 1, tip); h++) {
      blocks.push({
        height: h,
        outputs: [{
          txHash: h.toString(16).padStart(64, '0'),
          outputIndex: 0,
          targetKey: 'cd'.repeat(32),
          publicKey: 'ef'.repeat(32),
          amountCommitment: '0100000000000000',
        }],
      });
    }
    return blocks;
  }) as NodeCall;
  const outputCalls = () =>
    calls.filter((c) => c.method === 'chain_getOutputs').map((c) => c.params.start_height);
  return { call, outputCalls };
}

describe('Snap chain_getOutputs paging (#1656)', () => {
  it('makeSendRpc().getOutputs(0, tip) returns every output across the cap', async () => {
    const node = cappedNode(2_050, 1000);
    const rows = await makeSendRpc(node.call).getOutputs(0, 2_050);
    expect(rows).toHaveLength(2_051);
    expect(node.outputCalls()).toEqual([0, 1000, 2000]);
  });

  it('getOutputsWithMeta keeps every height when the range runs past the tip', async () => {
    const node = cappedNode(12, 5);
    const rows = await getOutputsWithMeta(node.call)(4, 999);
    expect(rows.map((r) => r.height)).toEqual([4, 5, 6, 7, 8, 9, 10, 11, 12]);
    expect(node.outputCalls()).toEqual([4, 9, 13]); // last page is the empty past-the-tip probe
  });

  it('passes other methods straight through', async () => {
    const node = cappedNode(7, 1000);
    expect(await pagedOutputsCall(node.call)('node_getStatus', {})).toEqual({ chainHeight: 7 });
  });
});
