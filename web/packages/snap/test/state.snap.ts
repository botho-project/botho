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
