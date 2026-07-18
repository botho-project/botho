/**
 * Balance + receive dialogs against a MOCKED node ingress (issue #815
 * deliverables 3 + 4), through the `@metamask/snaps-jest` SES harness. No live
 * betanet is required — `startMockNode` stands in for the node RPC.
 */

import { afterEach, beforeEach, describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';

import { startMockNode, type MockNode } from './mock-node';

function unwrap<T>(response: { response: unknown }): T {
  const res = response.response as { result?: T; error?: { message?: string } };
  if (res.error) {
    throw new Error(`snap returned error: ${JSON.stringify(res.error)}`);
  }
  return res.result as T;
}

function errorOf(response: { response: unknown }): { message?: string } | undefined {
  return (response.response as { error?: { message?: string } }).error;
}

describe('botho snap: balance + receive against a mocked node', () => {
  let node: MockNode;

  beforeEach(async () => {
    node = await startMockNode(); // empty testnet chain, network=botho-testnet
  });
  afterEach(async () => {
    await node.close();
  });

  it('getBalance scans the mocked chain and reports 0 for a fresh wallet', async () => {
    const { request } = await installSnap();
    const { spendablePicocredits } = unwrap<{ spendablePicocredits: string }>(
      await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }),
    );
    expect(spendablePicocredits).toBe('0');

    // It reached the node: status guard + chain scan actually happened.
    const methods = node.calls.map((c) => c.method);
    expect(methods).toContain('node_getStatus');
    expect(methods).toContain('chain_getOutputs');
  });

  it('showBalance opens a balance dialog and returns the balance', async () => {
    const { request } = await installSnap();
    const response = request({ method: 'botho_showBalance', params: { rpcUrl: node.url } });

    const ui = await response.getInterface();
    expect(ui.type).toBe('alert');
    // The dialog renders a spendable-balance figure with the BTH unit.
    expect(JSON.stringify(ui.content)).toContain('BTH');
    await (ui as { ok(): Promise<void> }).ok();

    const { spendablePicocredits } = unwrap<{ spendablePicocredits: string }>(await response);
    expect(spendablePicocredits).toBe('0');
  });

  it('showReceive opens a dialog carrying the stealth receive address', async () => {
    const { request } = await installSnap();
    const response = request({ method: 'botho_showReceive' });

    const ui = await response.getInterface();
    expect(ui.type).toBe('alert');
    expect(JSON.stringify(ui.content)).toContain('tbotho://2/');
    await (ui as { ok(): Promise<void> }).ok();

    const { address } = unwrap<{ address: string }>(await response);
    expect(address.startsWith('tbotho://2/')).toBe(true);
  });

  it('rejects a malformed rpcUrl before any network access', async () => {
    const { request } = await installSnap();
    const response = await request({
      method: 'botho_getBalance',
      params: { rpcUrl: 'not-a-url' },
    });
    expect(errorOf(response)?.message).toMatch(/valid https/i);
  });
});
