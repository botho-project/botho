/**
 * Send flow against a MOCKED node RPC (issue #815 deliverables 3 + 4), through
 * the `@metamask/snaps-jest` SES harness.
 *
 * A live-testnet send is NOT verifiable right now (betanet is frozen at height
 * 202, minting paused — #1051), and a successful on-chain send needs real
 * owned-output fixtures only a live node can mint. So these tests validate the
 * full send PLUMBING against a mock: the confirmation dialog, the user
 * approve/reject branches, the wrong-network / bad-input guards, and that the
 * pipeline reaches the node and never submits without funds. Live end-to-end
 * send validation is a documented follow-up (README.md).
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

const ONE_BTH = '1000000000000'; // 1 BTH in picocredits (1e12)

describe('botho snap: send against a mocked node', () => {
  let node: MockNode;

  beforeEach(async () => {
    node = await startMockNode(); // empty testnet chain
  });
  afterEach(async () => {
    await node.close();
  });

  /** The snap's own address makes a valid, self-consistent recipient. */
  async function ownAddress(request: (r: unknown) => Promise<{ response: unknown }>): Promise<string> {
    return unwrap<{ address: string }>(
      await (request as (r: unknown) => Promise<{ response: unknown }>)({ method: 'botho_getAddress' }),
    ).address;
  }

  it('shows a send confirmation dialog with the amount and recipient', async () => {
    const { request } = await installSnap();
    const to = await ownAddress(request as never);

    const response = request({
      method: 'botho_send',
      params: { rpcUrl: node.url, recipientAddress: to, amountPicocredits: ONE_BTH },
    });

    const ui = await response.getInterface();
    expect(ui.type).toBe('confirmation');
    const rendered = JSON.stringify(ui.content);
    expect(rendered).toContain('Confirm send');
    expect(rendered).toContain('1 BTH'); // amount row
    expect(rendered).toContain(to); // recipient copyable

    // Reject: the snap must abort and never submit.
    await (ui as { cancel(): Promise<void> }).cancel();
    const rejected = await response;
    expect(errorOf(rejected)?.message).toMatch(/reject/i);
    expect(node.calls.map((c) => c.method)).not.toContain('tx_submit');
  });

  it('on approval, runs the build pipeline and surfaces "no funds" without submitting', async () => {
    const { request } = await installSnap();
    const to = await ownAddress(request as never);

    const response = request({
      method: 'botho_send',
      params: { rpcUrl: node.url, recipientAddress: to, amountPicocredits: ONE_BTH },
    });
    const ui = await response.getInterface();
    await (ui as { ok(): Promise<void> }).ok();

    const result = await response;
    // Empty mocked chain => the shared builder has nothing to scan/spend.
    expect(errorOf(result)?.message).toMatch(/no outputs|no spendable|insufficient/i);
    // The pipeline reached the node (scan) but never submitted a transaction.
    const methods = node.calls.map((c) => c.method);
    expect(methods).toContain('chain_getOutputs');
    expect(methods).not.toContain('tx_submit');
  });

  it('rejects a malformed recipient address before showing a dialog', async () => {
    const { request } = await installSnap();
    const response = await request({
      method: 'botho_send',
      params: { rpcUrl: node.url, recipientAddress: 'botho://1/legacy', amountPicocredits: ONE_BTH },
    });
    expect(errorOf(response)?.message).toBeTruthy();
    // A v1 address is rejected by the parser; no send dialog, no submit.
    expect(node.calls.map((c) => c.method)).not.toContain('tx_submit');
  });

  it('rejects a non-positive amount', async () => {
    const { request } = await installSnap();
    const to = await ownAddress(request as never);
    const response = await request({
      method: 'botho_send',
      params: { rpcUrl: node.url, recipientAddress: to, amountPicocredits: '0' },
    });
    expect(errorOf(response)?.message).toMatch(/greater than 0/i);
  });
});
