/**
 * Payment-request (pull payment) ingress through the `@metamask/snaps-jest` SES
 * harness (issue #1108).
 *
 * A payment-request link (`/pay#…`) is the *pull* complement to the claim-link
 * *push* flow (#1094). It carries only the requester's PUBLIC address plus an
 * optional amount and memo — nothing secret. `botho_previewPaymentRequest` is a
 * PURE parse (no node RPC), so unlike the claim/send sweeps it is fully
 * deterministic against the SES harness with NO funded fixtures and NO #1051
 * caveat: these tests cover the real preview surface end-to-end.
 *
 * The address in every well-formed fixture is the Snap's OWN derived stealth
 * address (fetched once via `botho_getAddress`) so it is a guaranteed-valid v2
 * address — the boundary address-format validation (`isValidAddress`) is what we
 * assert rejects a malformed `to`.
 *
 * The prefill-send test demonstrates the intended "no new payer RPC" path: the
 * previewed `{ to, amountPicocredits }` is threaded straight into the existing
 * param-driven `botho_send`, whose confirmation dialog then renders those fields.
 * We cancel before submit (a live send inherits the #1051 gap), so this needs no
 * funded output.
 */

import { beforeAll, describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';
import { buildPaymentRequestFragment, buildPaymentRequestLink } from '@botho/core';

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

/** Craft a raw base64url'd wire payload directly (for malformed-input fixtures). */
function encodeWire(value: unknown): string {
  return Buffer.from(JSON.stringify(value), 'utf8').toString('base64url');
}

interface PreviewResult {
  to: string;
  amountPicocredits?: string;
  memo?: string;
}

describe('botho snap: payment-request (pull) ingress (#1108)', () => {
  // One install + one derived address, reused across the pure-parse cases (SES
  // lockdown + wasm is slow, and preview persists nothing).
  let request: Awaited<ReturnType<typeof installSnap>>['request'];
  let addr: string;

  beforeAll(async () => {
    const installed = await installSnap();
    request = installed.request;
    const { address } = unwrap<{ address: string }>(
      await request({ method: 'botho_getAddress' }),
    );
    addr = address;
  });

  /* ================================================================== */
  /* botho_previewPaymentRequest — pure parse, zero node RPC             */
  /* ================================================================== */

  it('parses a well-formed link with amount + memo (no dialog, no node call)', async () => {
    const fragment = buildPaymentRequestFragment({
      to: addr,
      amount: 5_000_000_000_000n,
      memo: 'Lunch',
    });
    const result = unwrap<PreviewResult>(
      await request({ method: 'botho_previewPaymentRequest', params: { link: fragment } }),
    );
    expect(result.to).toBe(addr);
    expect(result.amountPicocredits).toBe('5000000000000');
    expect(result.memo).toBe('Lunch');
  });

  it('omits amountPicocredits when the link leaves the amount open', async () => {
    const fragment = buildPaymentRequestFragment({ to: addr });
    const result = unwrap<PreviewResult>(
      await request({ method: 'botho_previewPaymentRequest', params: { link: fragment } }),
    );
    expect(result.to).toBe(addr);
    expect(result.amountPicocredits).toBeUndefined();
    expect(result.memo).toBeUndefined();
  });

  it('passes a unicode memo through verbatim (SES-safe UTF-8 codec)', async () => {
    const memo = 'Café ☕ — 支払い 💸';
    const fragment = buildPaymentRequestFragment({ to: addr, memo });
    const result = unwrap<PreviewResult>(
      await request({ method: 'botho_previewPaymentRequest', params: { link: fragment } }),
    );
    expect(result.memo).toBe(memo);
  });

  it('accepts a bare fragment, a leading-# fragment, and a full URL alike', async () => {
    const fragment = buildPaymentRequestFragment({ to: addr, amount: 7n });
    const url = buildPaymentRequestLink('https://botho.io', { to: addr, amount: 7n });
    for (const link of [fragment, `#${fragment}`, url]) {
      const result = unwrap<PreviewResult>(
        await request({ method: 'botho_previewPaymentRequest', params: { link } }),
      );
      expect(result.to).toBe(addr);
      expect(result.amountPicocredits).toBe('7');
    }
  });

  /* ================================================================== */
  /* Error branches — all typed InvalidParamsError, all pre-dialog      */
  /* ================================================================== */

  it('rejects malformed base64 / non-JSON garbage with InvalidParamsError', async () => {
    const response = await request({
      method: 'botho_previewPaymentRequest',
      params: { link: '!!!not-a-valid-link!!!' },
    });
    expect(errorOf(response)?.message).toMatch(/invalid payment request/i);
  });

  it('rejects valid base64 that is not JSON', async () => {
    // "hello" base64url'd decodes fine but is not JSON.
    const response = await request({
      method: 'botho_previewPaymentRequest',
      params: { link: 'aGVsbG8' },
    });
    expect(errorOf(response)?.message).toMatch(/invalid payment request/i);
  });

  it('rejects a payload missing the recipient', async () => {
    const response = await request({
      method: 'botho_previewPaymentRequest',
      params: { link: encodeWire({ amount: '5' }) },
    });
    expect(errorOf(response)?.message).toMatch(/invalid payment request/i);
  });

  it('rejects a syntactically-fine but format-invalid recipient address', async () => {
    // parsePaymentRequestFragment accepts any non-empty `to`; the Snap boundary
    // rejects it because it is not a valid Botho address.
    const fragment = buildPaymentRequestFragment({ to: 'tbotho://2/not-a-real-address' });
    const response = await request({
      method: 'botho_previewPaymentRequest',
      params: { link: fragment },
    });
    expect(errorOf(response)?.message).toMatch(/not a valid botho address/i);
  });

  it('rejects an empty link param', async () => {
    const response = await request({
      method: 'botho_previewPaymentRequest',
      params: { link: '' },
    });
    expect(errorOf(response)?.message).toBeTruthy();
  });

  /* ================================================================== */
  /* botho_showPaymentRequest — informational alert dialog              */
  /* ================================================================== */

  it('renders an alert dialog echoing the address, amount, and memo', async () => {
    const fragment = buildPaymentRequestFragment({
      to: addr,
      amount: 2_500_000_000_000n,
      memo: 'Invoice #42',
    });
    const response = request({
      method: 'botho_showPaymentRequest',
      params: { link: fragment },
    });
    const ui = await response.getInterface();
    expect(ui.type).toBe('alert');
    const rendered = JSON.stringify(ui.content);
    expect(rendered).toContain('Payment request');
    expect(rendered).toContain(addr); // public address echoed in the clear
    expect(rendered).toContain('2.5 BTH'); // requested amount rendered
    expect(rendered).toContain('Invoice #42'); // memo passthrough

    await (ui as { ok(): Promise<void> }).ok();
    const result = unwrap<PreviewResult>(await response);
    expect(result.to).toBe(addr);
    expect(result.amountPicocredits).toBe('2500000000000');
  });

  it('shows "any amount" copy when the request leaves the amount open', async () => {
    const fragment = buildPaymentRequestFragment({ to: addr });
    const response = request({
      method: 'botho_showPaymentRequest',
      params: { link: fragment },
    });
    const ui = await response.getInterface();
    expect(JSON.stringify(ui.content)).toContain('Any amount');
    await (ui as { ok(): Promise<void> }).ok();
    const result = unwrap<PreviewResult>(await response);
    expect(result.amountPicocredits).toBeUndefined();
  });

  /* ================================================================== */
  /* Prefill-send: preview -> feed the fields into the existing         */
  /* param-driven botho_send (no new payer RPC). Cancel before submit   */
  /* (a live send inherits the #1051 gap).                              */
  /* ================================================================== */

  it('prefills botho_send from the preview and shows the requested figures', async () => {
    const node: MockNode = await startMockNode();
    try {
      const fragment = buildPaymentRequestFragment({ to: addr, amount: 3_000_000_000_000n });

      // 1. Preview the request link (pure parse, no node call).
      const preview = unwrap<PreviewResult>(
        await request({ method: 'botho_previewPaymentRequest', params: { link: fragment } }),
      );
      const amountPicocredits = preview.amountPicocredits ?? '';
      expect(amountPicocredits).toBe('3000000000000');

      // 2. Thread the previewed fields straight into the existing botho_send.
      const send = request({
        method: 'botho_send',
        params: {
          rpcUrl: node.url,
          recipientAddress: preview.to,
          amountPicocredits,
        },
      });
      const ui = await send.getInterface();
      expect(ui.type).toBe('confirmation');
      const rendered = JSON.stringify(ui.content);
      expect(rendered).toContain(preview.to); // prefilled recipient
      expect(rendered).toContain('3 BTH'); // prefilled amount

      // Cancel before any on-chain submit (live send is #1051-gated).
      await (ui as { cancel(): Promise<void> }).cancel();
      const rejected = await send;
      expect(errorOf(rejected)?.message).toMatch(/reject/i);
      expect(node.calls.map((c) => c.method)).not.toContain('tx_submit');
    } finally {
      await node.close();
    }
  });
});
