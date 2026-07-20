/**
 * Transaction history view (issue #1092).
 *
 * History is a PURE projection over the scan state #1091 already persists
 * (`scan.ownedOutputs`, each carrying `blockHeight`/`txHash`) plus a LIVE
 * spent-check — no schema migration, no `STATE_VERSION` bump, no rescan.
 *
 * Two layers, mirroring `state.snap.ts`:
 *
 *  1. PURE-LOGIC tests of `deriveHistory` / `spentTargetKeys` (no `installSnap`,
 *     no wasm): descending block-height order, clamped `confirmations`,
 *     decimal-string amounts, received-vs-spent direction from a supplied
 *     spent-set, and `[]` for an empty scan.
 *
 *  2. BEHAVIOURAL tests through the real SES `@metamask/snaps-jest` harness
 *     against the mocked node: `botho_getHistory` on a fresh/empty chain returns
 *     `{ entries: [] }`; `botho_showHistory` renders a dialog with the empty
 *     state; and a history read AFTER a balance read reuses the persisted scan
 *     (issuing no `chain_getOutputs` window below the reorg-buffer resume point).
 */

import { afterEach, describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';

import { startMockNode, type MockNode } from './mock-node';
import {
  STATE_VERSION,
  REORG_BUFFER,
  scanStartHeight,
  deriveHistory,
  spentTargetKeys,
  type PersistedOwnedOutput,
} from '../src/state';
import type { OwnedOutput } from '@botho/wasm-signer';

function unwrap<T>(response: { response: unknown }): T {
  const res = response.response as { result?: T; error?: { message?: string } };
  if (res.error) {
    throw new Error(`snap returned error: ${JSON.stringify(res.error)}`);
  }
  return res.result as T;
}

/** Count / inspect the windowed output fetches the Snap issued. */
function outputWindows(node: MockNode): Array<{ start: number; end: number }> {
  return node.calls
    .filter((c) => c.method === 'chain_getOutputs')
    .map((c) => {
      const p = c.params as { start_height: number; end_height: number };
      return { start: p.start_height, end: p.end_height };
    });
}

/** A persisted owned output fixture at a given block/amount/target key. */
function persisted(
  targetKey: string,
  blockHeight: number,
  amount: string,
  txHash = `${targetKey.slice(0, 2)}`.repeat(32),
): PersistedOwnedOutput {
  return {
    targetKey,
    publicKey: 'bb'.repeat(32),
    amount,
    subaddressIndex: '0',
    outputIndex: 0,
    kemCiphertext: null,
    blockHeight,
    txHash,
  };
}

/* ================================================================== */
/* 1. Pure-logic projection (no SES install)                          */
/* ================================================================== */

describe('deriveHistory / spentTargetKeys projection (#1092)', () => {
  const a = persisted('aa'.repeat(32), 10, '1000000000000'); // 1 BTH @ block 10
  const b = persisted('bb'.repeat(32), 50, '2500000000000'); // 2.5 BTH @ block 50
  const c = persisted('cc'.repeat(32), 30, '500000000000'); //  0.5 BTH @ block 30

  it('sorts entries by block height descending (newest receive first)', () => {
    const entries = deriveHistory([a, b, c], new Set(), 100);
    expect(entries.map((e) => e.blockHeight)).toEqual([50, 30, 10]);
    expect(entries.map((e) => e.txHash)).toEqual([b.txHash, c.txHash, a.txHash]);
  });

  it('computes confirmations = max(0, tip - blockHeight) and clamps below zero', () => {
    const entries = deriveHistory([a, b], new Set(), 60);
    const byHeight = new Map(entries.map((e) => [e.blockHeight, e.confirmations]));
    expect(byHeight.get(50)).toBe(10); // 60 - 50
    expect(byHeight.get(10)).toBe(50); // 60 - 10

    // Post-reorg shrink: an output whose receive height is now ABOVE the tip
    // must clamp to 0 confirmations, never a negative depth.
    const shrunk = deriveHistory([b], new Set(), 40);
    expect(shrunk[0].confirmations).toBe(0);
  });

  it('carries amounts as decimal strings (JSON-safe), not bigints', () => {
    const entries = deriveHistory([a], new Set(), 100);
    expect(entries[0].amountPicocredits).toBe('1000000000000');
    expect(typeof entries[0].amountPicocredits).toBe('string');
  });

  it('marks entries in the supplied spent-set as spent, the rest as received', () => {
    const entries = deriveHistory([a, b, c], new Set([b.targetKey]), 100);
    const dir = new Map(entries.map((e) => [e.blockHeight, e.direction]));
    expect(dir.get(50)).toBe('spent'); // b is in the spent-set
    expect(dir.get(30)).toBe('received');
    expect(dir.get(10)).toBe('received');
  });

  it('returns [] for an empty scan', () => {
    expect(deriveHistory([], new Set(), 100)).toEqual([]);
  });

  it('does not bump STATE_VERSION (history is derived, not stored)', () => {
    // Guard the schema-extension-without-migration property #1092 relies on.
    expect(STATE_VERSION).toBe(1);
  });

  it('derives the spent-set as owned minus live-spendable (by target key)', () => {
    const spendable: OwnedOutput[] = [
      { targetKey: a.targetKey, publicKey: a.publicKey, amount: 1_000_000_000_000n, subaddressIndex: 0n },
      { targetKey: c.targetKey, publicKey: c.publicKey, amount: 500_000_000_000n, subaddressIndex: 0n },
    ];
    // b is owned but NOT spendable => spent (value left the wallet).
    const spent = spentTargetKeys([a, b, c], spendable);
    expect(spent.has(b.targetKey)).toBe(true);
    expect(spent.has(a.targetKey)).toBe(false);
    expect(spent.has(c.targetKey)).toBe(false);

    // Feeding that spent-set through deriveHistory flips only b to 'spent'.
    const entries = deriveHistory([a, b, c], spent, 100);
    expect(entries.find((e) => e.blockHeight === 50)?.direction).toBe('spent');
    expect(entries.find((e) => e.blockHeight === 10)?.direction).toBe('received');
  });
});

/* ================================================================== */
/* 2. Behavioural: history through the SES harness                    */
/* ================================================================== */

interface HistoryEntryJson {
  txHash: string;
  blockHeight: number;
  amountPicocredits: string;
  direction: 'received' | 'spent';
  confirmations: number;
}

describe('botho snap: transaction history against a mocked node (#1092)', () => {
  let node: MockNode;

  afterEach(async () => {
    if (node) await node.close();
  });

  it('botho_getHistory on a fresh/empty chain returns { entries: [] }', async () => {
    node = await startMockNode({ chainHeight: 100 });
    const { request } = await installSnap();

    const { entries } = unwrap<{ entries: HistoryEntryJson[] }>(
      await request({ method: 'botho_getHistory', params: { rpcUrl: node.url } }),
    );
    expect(entries).toEqual([]);

    const methods = node.calls.map((c) => c.method);
    expect(methods).toContain('node_getStatus'); // wrong-network guard ran
    expect(methods).toContain('chain_getOutputs'); // scanned the chain
  });

  it('botho_showHistory renders a dialog with the empty-state', async () => {
    node = await startMockNode({ chainHeight: 100 });
    const { request } = await installSnap();

    const response = request({ method: 'botho_showHistory', params: { rpcUrl: node.url } });
    const ui = await response.getInterface();
    expect(ui.type).toBe('alert');
    const rendered = JSON.stringify(ui.content);
    expect(rendered).toContain('Transaction history');
    expect(rendered).toContain('No transactions yet');

    await (ui as { ok(): Promise<void> }).ok();
    const result = unwrap<{ entries: HistoryEntryJson[]; count: number }>(await response);
    expect(result.entries).toEqual([]);
    expect(result.count).toBe(0);
  });

  it('reuses persisted scan state: history after a balance read issues no window below the reorg buffer', async () => {
    // Multi-window tip so the incremental win is visible in call counts.
    const tip = 3000;
    node = await startMockNode({ chainHeight: tip });
    const { request } = await installSnap();

    // Seed persisted scan state with a full genesis scan via the balance path.
    unwrap(await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }));
    const afterBalance = outputWindows(node).length;
    expect(afterBalance).toBeGreaterThan(1); // genuinely multi-window genesis scan

    // History at the SAME tip must resume from the persisted checkpoint, not
    // rescan from genesis: only the trailing reorg-buffer window is re-fetched.
    const { entries } = unwrap<{ entries: HistoryEntryJson[] }>(
      await request({ method: 'botho_getHistory', params: { rpcUrl: node.url } }),
    );
    expect(entries).toEqual([]); // empty chain, but the persisted-reuse property is what we assert

    const historyWindows = outputWindows(node).slice(afterBalance);
    expect(historyWindows).toEqual([{ start: scanStartHeight(tip), end: tip }]);
    // No window reaches back below the reorg-buffer resume point (no rescan).
    for (const w of historyWindows) {
      expect(w.start).toBeGreaterThanOrEqual(tip - REORG_BUFFER);
    }
  });
});
