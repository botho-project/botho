/**
 * Persisted state + windowed/incremental scanning (issue #1091).
 *
 * Two layers:
 *
 *  1. PURE-LOGIC tests of the `src/state.ts` helpers (no `installSnap`, no wasm),
 *     the same pattern as `units.snap.ts`. These pin the persisted-state CONTRACT
 *     the sibling consumers (#1092 history, #1093 contacts) depend on: the
 *     namespaced `{ version, scan }` shape, owned outputs carrying
 *     `blockHeight`/`txHash` with NO persisted spent status, target-key dedupe,
 *     window boundaries, reorg-buffer resume, and network/version invalidation.
 *
 *  2. BEHAVIOURAL tests through the real SES `@metamask/snaps-jest` harness against
 *     the mocked node. The simulation harness (v4.x) exposes no state getter, so
 *     persistence is proven the observable way it matters — off `node.calls`: a
 *     first read scans the whole `(0, tip]` range in windows; a second read at the
 *     same tip fetches only the reorg-buffer window (state survived + resumed);
 *     a read against a different network rescans from genesis (invalidation).
 */

import { afterEach, describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';

import { startMockNode, type MockNode } from './mock-node';
import {
  STATE_VERSION,
  WINDOW_SIZE,
  REORG_BUFFER,
  emptyScanState,
  usableScanState,
  scanStartHeight,
  windows,
  mergeOwnedOutputs,
  reconcileOwnedOutputs,
  spentTargetKeys,
  deriveHistory,
  toPersistedOwnedOutput,
  toOwnedOutput,
  type PersistedOwnedOutput,
  type SnapState,
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

/* ================================================================== */
/* 1. Pure-logic contract (no SES install)                            */
/* ================================================================== */

describe('persisted scan-state helpers (#1091)', () => {
  const ownedFixture: OwnedOutput = {
    targetKey: 'aa'.repeat(32),
    publicKey: 'bb'.repeat(32),
    amount: 1_500_000_000_000n, // 1.5 BTH
    subaddressIndex: 1n, // change output — must survive persistence
    outputIndex: 3,
    kemCiphertext: 'cc'.repeat(8),
  };

  it('serialises an owned output with immutable receive facts and NO spent status', () => {
    const p = toPersistedOwnedOutput(ownedFixture, { blockHeight: 42, txHash: 'de'.repeat(32) });

    expect(p.amount).toBe('1500000000000'); // bigint -> JSON-safe string
    expect(p.subaddressIndex).toBe('1'); // change index preserved (key-image correctness)
    expect(p.outputIndex).toBe(3);
    expect(p.kemCiphertext).toBe('cc'.repeat(8));
    expect(p.blockHeight).toBe(42); // history (#1092) needs no rescan
    expect(p.txHash).toBe('de'.repeat(32));
    // Spent status is deliberately NOT persisted — recomputed live each read.
    expect(Object.keys(p)).not.toContain('spent');
    expect(Object.keys(p)).not.toContain('spentHeight');
  });

  it('round-trips a persisted output back into an OwnedOutput for the spent check', () => {
    const p = toPersistedOwnedOutput(ownedFixture, { blockHeight: 42, txHash: 'de'.repeat(32) });
    const back = toOwnedOutput(p);
    expect(back.amount).toBe(1_500_000_000_000n);
    expect(back.subaddressIndex).toBe(1n);
    expect(back.outputIndex).toBe(3);
    expect(back.targetKey).toBe(ownedFixture.targetKey);
  });

  it('dedupes merged owned outputs by one-time target key (reorg re-scan is idempotent)', () => {
    const a = toPersistedOwnedOutput(ownedFixture, { blockHeight: 42, txHash: 'de'.repeat(32) });
    const b: PersistedOwnedOutput = {
      ...a,
      targetKey: 'ff'.repeat(32),
      blockHeight: 43,
      txHash: 'ab'.repeat(32),
    };
    // Re-discovering `a` in the reorg buffer must not append a duplicate.
    const merged = mergeOwnedOutputs([a], [a, b]);
    expect(merged).toHaveLength(2);
    expect(merged.map((o) => o.targetKey).sort()).toEqual([a.targetKey, b.targetKey].sort());
  });

  it('splits [start, tip] into fixed-size inclusive windows', () => {
    expect(windows(0, 100)).toEqual([[0, 100]]); // short chain: one window
    expect(windows(0, WINDOW_SIZE * 2 + 500)).toEqual([
      [0, WINDOW_SIZE - 1],
      [WINDOW_SIZE, WINDOW_SIZE * 2 - 1],
      [WINDOW_SIZE * 2, WINDOW_SIZE * 2 + 500],
    ]);
    expect(windows(100, 100)).toEqual([[100, 100]]); // single-block tail
    expect(windows(120, 100)).toEqual([]); // nothing to scan (chain shrank)
  });

  it('resumes a reorg-buffer below the checkpoint, clamped at genesis', () => {
    expect(scanStartHeight(100)).toBe(100 - REORG_BUFFER);
    expect(scanStartHeight(REORG_BUFFER - 1)).toBe(0); // clamp: never negative
    expect(scanStartHeight(0)).toBe(0);
  });

  it('invalidates foreign-network or stale-schema state (forces a full rescan)', () => {
    const good: SnapState = {
      version: STATE_VERSION,
      scan: emptyScanState('botho-testnet'),
    };
    expect(usableScanState(good, 'botho-testnet')).toBe(good.scan);
    // Different network -> discard.
    expect(usableScanState(good, 'botho-mainnet')).toBeNull();
    // Version mismatch -> discard.
    expect(usableScanState({ ...good, version: STATE_VERSION + 1 }, 'botho-testnet')).toBeNull();
    // No prior state -> discard.
    expect(usableScanState(null, 'botho-testnet')).toBeNull();
  });

  it('namespaces state as { version, scan } so siblings extend without migration', () => {
    const state: SnapState = { version: STATE_VERSION, scan: emptyScanState('botho-testnet') };
    // Top-level keys are the reserved namespace surface for #1092/#1093.
    expect(state.version).toBe(1);
    expect(state.scan?.ownedOutputs).toEqual([]);
    expect(state.scan?.lastScannedHeight).toBe(0);
  });
});

/* ================================================================== */
/* 1b. reconcile-on-rescan: the reorg-staleness prune (#1099)         */
/* ================================================================== */

describe('reconcileOwnedOutputs — prune-on-rescan safety invariant (#1099)', () => {
  /** Build a persisted owned output with a distinct target key at a given height. */
  function out(tag: string, blockHeight: number, amount = '1000'): PersistedOwnedOutput {
    // 64-hex target key from the tag so keys are unique + deterministic.
    const targetKey = (tag.charCodeAt(0).toString(16).padStart(2, '0')).repeat(32);
    return {
      targetKey,
      publicKey: 'bb'.repeat(32),
      amount,
      subaddressIndex: '0',
      outputIndex: 0,
      kemCiphertext: null,
      blockHeight,
      txHash: `${tag}${tag}`.repeat(16).slice(0, 64),
    };
  }
  const keys = (xs: PersistedOwnedOutput[]) => xs.map((o) => o.targetKey).sort();
  /** Sum owned amounts the way `incrementalScanBalance` would (all unspent). */
  const sum = (xs: PersistedOwnedOutput[]) => xs.reduce((s, o) => s + BigInt(o.amount), 0n);

  it('prunes a stale (reorged-out) output inside the re-fetched range', () => {
    const A = out('A', 95); // in [90,100], node no longer returns it
    const B = out('B', 50); // below the re-fetched range
    const result = reconcileOwnedOutputs([A, B], /* discovered */ [], 90, 100);

    // A pruned (reorged out of its window); B (below start) retained.
    expect(keys(result)).toEqual(keys([B]));
    // It drops out of the balance sum...
    expect(sum(result)).toBe(BigInt(B.amount));
    // ...and out of history entirely (no live-spent set needed — it's gone).
    const history = deriveHistory(result, new Set<string>(), 100);
    expect(history.map((h) => h.txHash)).not.toContain(A.txHash);
    expect(history.map((h) => h.txHash)).toContain(B.txHash);
  });

  it('NEVER prunes a still-returned output in the re-fetched range (funds-safety)', () => {
    const A = out('A', 95);
    // Node re-returns A unchanged this scan.
    const result = reconcileOwnedOutputs([A], /* discovered */ [A], 90, 100);
    expect(keys(result)).toEqual(keys([A])); // retained
    expect(result).toHaveLength(1); // and not duplicated
    expect(sum(result)).toBe(BigInt(A.amount)); // balance intact
  });

  it('never prunes an output below the re-fetched range even when absent from discovered', () => {
    const B = out('B', 50);
    const result = reconcileOwnedOutputs([B], /* discovered */ [], 90, 100);
    expect(keys(result)).toEqual(keys([B])); // below start -> never re-fetched -> retained
  });

  it('prunes an output stranded above the tip when the chain shrank', () => {
    const A = out('A', 98); // above the reported tip
    const B = out('B', 50);
    const result = reconcileOwnedOutputs([A, B], /* discovered */ [], 90, 93);
    expect(keys(result)).toEqual(keys([B])); // A (>tip) pruned, B retained
  });

  it('deep shrink (tip < start): prunes everything above tip, prunes nothing at/below it, queries nothing extra', () => {
    const h5 = out('a', 5);
    const h6 = out('b', 6);
    const h50 = out('c', 50);
    const h95 = out('d', 95);
    // start=90, tip=5 -> windows(90,5) is empty -> discovered is necessarily empty.
    expect(windows(90, 5)).toEqual([]);
    const result = reconcileOwnedOutputs([h5, h6, h50, h95], [], 90, 5);
    // Only h=5 (<= tip) survives; h6/h50/h95 are all > tip -> provably gone.
    expect(keys(result)).toEqual(keys([h5]));
  });

  it('retains a spent-but-still-mined output (present in discovered) and renders it direction:spent', () => {
    const A = out('A', 95, '2000');
    // The node still MINES A (spending does not remove it from chain_getOutputs),
    // so it is re-returned in discovered and must be retained...
    const reconciled = reconcileOwnedOutputs([A], /* discovered */ [A], 90, 100);
    expect(keys(reconciled)).toEqual(keys([A]));

    // ...even though the LIVE spent-check reports A as spent (spendable excludes it).
    const spendable: OwnedOutput[] = []; // A has been spent -> not spendable
    const spent = spentTargetKeys(reconciled, spendable);
    expect(spent.has(A.targetKey)).toBe(true);

    const history = deriveHistory(reconciled, spent, 100);
    const entry = history.find((h) => h.txHash === A.txHash);
    expect(entry).toBeDefined();
    expect(entry?.direction).toBe('spent'); // rendered spent, NOT pruned
  });

  it('is idempotent on the happy path (re-returned unchanged, no duplicates)', () => {
    const A = out('A', 95);
    const B = out('B', 50);
    const result = reconcileOwnedOutputs([A, B], /* discovered */ [A], 90, 100);
    expect(keys(result)).toEqual(keys([A, B])); // order-insensitive set equality
    expect(result).toHaveLength(2); // no duplicate of A
  });

  it('reconciles correctly across multiple windows (low-boundary output not pruned by a later window)', () => {
    // A big range spans >1 window; A sits at the low window boundary, C near the tip.
    const start = 0;
    const tip = WINDOW_SIZE + 500; // two windows: [0, WINDOW_SIZE-1], [WINDOW_SIZE, tip]
    expect(windows(start, tip).length).toBeGreaterThan(1);

    const A = out('A', 0); // low window boundary, discovered in window 1
    const C = out('C', WINDOW_SIZE + 100); // discovered in window 2
    const stale = out('S', 5); // persisted but reorged out -> not in discovered
    // discovered accumulates across BOTH windows before the single reconcile.
    const allDiscovered = [A, C];
    const result = reconcileOwnedOutputs([A, stale], allDiscovered, start, tip);

    expect(keys(result)).toEqual(keys([A, C])); // A kept, C added, stale pruned
  });

  it('reorg-buffer boundary: blockHeight === start is in-range; start-1 is retained', () => {
    const atStart = out('A', 90); // exactly at start -> in the re-fetched range
    const belowStart = out('B', 89); // one below -> out of range -> retained
    // Neither is re-returned this scan.
    const result = reconcileOwnedOutputs([atStart, belowStart], [], 90, 100);
    expect(keys(result)).toEqual(keys([belowStart])); // atStart pruned, belowStart retained
  });
});

/* ================================================================== */
/* 2. Behavioural: incremental scan through the SES harness           */
/* ================================================================== */

describe('botho snap: windowed incremental scan against a mocked node (#1091)', () => {
  let node: MockNode;

  afterEach(async () => {
    if (node) await node.close();
  });

  it('fresh wallet on an empty chain: balance 0 via a single window', async () => {
    node = await startMockNode({ chainHeight: 100 }); // < WINDOW_SIZE -> one window
    const { request } = await installSnap();

    const { spendablePicocredits } = unwrap<{ spendablePicocredits: string }>(
      await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }),
    );
    expect(spendablePicocredits).toBe('0');

    const methods = node.calls.map((c) => c.method);
    expect(methods).toContain('node_getStatus'); // wrong-network guard ran
    expect(outputWindows(node)).toEqual([{ start: 0, end: 100 }]); // scanned (0, tip]
  });

  it('resumes from the checkpoint: a second read at the same tip fetches no new windows', async () => {
    // Tip spanning multiple windows makes the incremental win visible in call counts.
    const tip = WINDOW_SIZE * 2 + 500;
    node = await startMockNode({ chainHeight: tip });
    const { request } = await installSnap();

    // First read: full genesis scan across every window.
    const first = unwrap<{ spendablePicocredits: string }>(
      await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }),
    );
    expect(first.spendablePicocredits).toBe('0');
    const firstWindows = outputWindows(node);
    expect(firstWindows).toEqual(windows(0, tip).map(([start, end]) => ({ start, end })));
    expect(firstWindows.length).toBeGreaterThan(1); // genuinely multi-window

    // Second read at the SAME tip: state survived + resumed, so only the reorg
    // buffer (a single trailing window) is re-fetched — NOT the whole chain.
    const second = unwrap<{ spendablePicocredits: string }>(
      await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }),
    );
    expect(second.spendablePicocredits).toBe('0'); // same balance

    const resumeWindows = outputWindows(node).slice(firstWindows.length);
    expect(resumeWindows).toEqual([{ start: scanStartHeight(tip), end: tip }]);
    expect(resumeWindows.length).toBeLessThan(firstWindows.length);
    // The resume window starts near the tip, never back at genesis.
    expect(resumeWindows[0].start).toBeGreaterThanOrEqual(tip - REORG_BUFFER);
  });

  it('invalidates persisted state from another network and rescans from genesis', async () => {
    // 1. Seed state on testnet.
    node = await startMockNode({ network: 'botho-testnet', chainHeight: 100 });
    const { request } = await installSnap();
    unwrap(await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }));
    await node.close();

    // 2. Point the SAME Snap install at a loopback node reporting a DIFFERENT
    //    network (loopback is exempt from the wrong-network guard, but the
    //    persisted scan is still network-bound and must be discarded).
    node = await startMockNode({ network: 'botho-devnet', chainHeight: 100 });
    unwrap(await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }));

    // The foreign-network read rescanned from genesis (start_height 0), rather
    // than resuming the testnet checkpoint's reorg-buffer window near the tip.
    const starts = outputWindows(node).map((w) => w.start);
    expect(starts).toContain(0);
  });
});
