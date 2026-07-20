/**
 * Claim-link ingress against a MOCKED node RPC (issue #1094), through the
 * `@metamask/snaps-jest` SES harness.
 *
 * A claim link is a bearer instrument: the fragment IS an ephemeral 12-word
 * mnemonic that owns the funds. Claiming reuses the normal CLSAG scan/send path
 * (no new node RPC). As with `botho_send`, a live on-chain sweep needs real
 * owned-output fixtures only a live minting node produces (betanet is frozen at
 * height 202 — #1051), so these tests validate the full claim PLUMBING against
 * the empty mock: the parse/derive/scan reads, the preview + confirm dialogs, the
 * approve/reject branches, and that the sweep pipeline reaches the node yet never
 * `tx_submit`s without funds. LIVE claim validation is inherited from the
 * existing #1051 send gap, not a new blocker (README.md).
 *
 * The wrong-network guard is loopback-exempt (the mock binds to 127.0.0.1), so
 * its throwing behaviour is covered at the pure level in `units.snap.ts`
 * (`assertNetworkAllowed`); here we assert both methods run `node_getStatus`
 * (i.e. route through `connectAndGuard`) before scanning.
 */

import { afterEach, beforeEach, describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';
import {
  createClaimLinkMnemonic,
  encodeClaimLinkFragment,
  buildClaimLink,
} from '@botho/core';

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

/** A fresh, well-formed claim link fragment (empty mock => 0 spendable). */
function freshLink(amountHint?: bigint): { mnemonic: string; fragment: string } {
  const mnemonic = createClaimLinkMnemonic();
  return { mnemonic, fragment: encodeClaimLinkFragment(mnemonic, amountHint) };
}

interface ClaimScanResult {
  grossPicocredits: string;
  feePicocredits: string;
  netPicocredits: string;
}

describe('botho snap: claim-link ingress against a mocked node (#1094)', () => {
  let node: MockNode;

  beforeEach(async () => {
    node = await startMockNode(); // empty testnet chain
  });
  afterEach(async () => {
    await node.close();
  });

  /* ================================================================== */
  /* botho_previewClaimLink (pure read — zero #1051 caveat)             */
  /* ================================================================== */

  it('previews a well-formed link: alert dialog + deterministic 0 net on the empty mock', async () => {
    const { request } = await installSnap();
    const { fragment } = freshLink();

    const response = request({
      method: 'botho_previewClaimLink',
      params: { rpcUrl: node.url, link: fragment },
    });

    const ui = await response.getInterface();
    expect(ui.type).toBe('alert');
    const rendered = JSON.stringify(ui.content);
    expect(rendered).toContain('Claim link');
    expect(rendered).toContain('BTH'); // amount rows rendered
    expect(rendered).toContain('Nothing to claim'); // empty-state on the empty mock

    await (ui as { ok(): Promise<void> }).ok();
    const result = unwrap<ClaimScanResult>(await response);
    expect(result.netPicocredits).toBe('0');
    expect(result.grossPicocredits).toBe('0');
    expect(BigInt(result.feePicocredits)).toBeGreaterThan(0n); // network min fee

    // The scan reached the node (wrong-network guard + output fetch).
    const methods = node.calls.map((c) => c.method);
    expect(methods).toContain('node_getStatus');
    expect(methods).toContain('chain_getOutputs');
    expect(methods).not.toContain('tx_submit'); // pure read
  });

  it('accepts a full URL, a bare fragment, and a leading-# fragment alike', async () => {
    // One install (SES lockdown + wasm is slow); previewClaimLink is a pure read
    // that persists nothing, so all three link shapes reuse the same snap.
    const { request } = await installSnap();
    const { mnemonic, fragment } = freshLink();
    const url = buildClaimLink('https://botho.io', mnemonic);
    for (const link of [fragment, `#${fragment}`, url]) {
      const response = request({
        method: 'botho_previewClaimLink',
        params: { rpcUrl: node.url, link },
      });
      // Preview shows an alert; acknowledge it so the request resolves.
      const ui = await response.getInterface();
      await (ui as { ok(): Promise<void> }).ok();
      const result = unwrap<ClaimScanResult>(await response);
      expect(result.netPicocredits).toBe('0');
    }
  });

  it('renders the cosmetic amount hint pre-scan but the scanned amount stays authoritative', async () => {
    const { request } = await installSnap();
    const { fragment } = freshLink(5_000_000_000_000n); // 5 BTH hint

    const response = request({
      method: 'botho_previewClaimLink',
      params: { rpcUrl: node.url, link: fragment },
    });
    const ui = await response.getInterface();
    const rendered = JSON.stringify(ui.content);
    expect(rendered).toContain('Link hint'); // cosmetic hint shown
    expect(rendered).toContain('5 BTH'); // the hint value
    expect(rendered).toContain('authoritative'); // labelled non-authoritative

    await (ui as { ok(): Promise<void> }).ok();
    // Scanned net overrides the 5 BTH hint: the empty mock yields 0.
    const result = unwrap<ClaimScanResult>(await response);
    expect(result.netPicocredits).toBe('0');
  });

  it('rejects a malformed link with InvalidParamsError — no network call, no dialog', async () => {
    const { request } = await installSnap();
    const response = await request({
      method: 'botho_previewClaimLink',
      params: { rpcUrl: node.url, link: 'not-a-valid-claim-link' },
    });
    expect(errorOf(response)?.message).toMatch(/invalid claim link/i);
    // Failed before any node round-trip.
    expect(node.calls).toHaveLength(0);
  });

  it('rejects an unsupported claim-link version before any network call', async () => {
    const { request } = await installSnap();
    const response = await request({
      method: 'botho_previewClaimLink',
      params: { rpcUrl: node.url, link: 'v9.abcdef' },
    });
    expect(errorOf(response)?.message).toMatch(/invalid claim link|version/i);
    expect(node.calls).toHaveLength(0);
  });

  it('rejects an empty link param before any network call', async () => {
    const { request } = await installSnap();
    const response = await request({
      method: 'botho_previewClaimLink',
      params: { rpcUrl: node.url, link: '' },
    });
    expect(errorOf(response)?.message).toBeTruthy();
    expect(node.calls).toHaveLength(0);
  });

  /* ================================================================== */
  /* botho_claimLink (sweep — botho_send fidelity, live gated on #1051) */
  /* ================================================================== */

  it('shows a confirmation dialog and, on cancel, aborts without submitting', async () => {
    const { request } = await installSnap();
    const { fragment } = freshLink();

    const response = request({
      method: 'botho_claimLink',
      params: { rpcUrl: node.url, link: fragment },
    });

    const ui = await response.getInterface();
    expect(ui.type).toBe('confirmation');
    const rendered = JSON.stringify(ui.content);
    expect(rendered).toContain('Confirm claim');
    expect(rendered).toContain('BTH');

    await (ui as { cancel(): Promise<void> }).cancel();
    const rejected = await response;
    expect(errorOf(rejected)?.message).toMatch(/reject/i);
    expect(node.calls.map((c) => c.method)).not.toContain('tx_submit');
  });

  it('on approval, runs the sweep pipeline and surfaces "nothing to claim" without submitting', async () => {
    const { request } = await installSnap();
    const { fragment } = freshLink();

    const response = request({
      method: 'botho_claimLink',
      params: { rpcUrl: node.url, link: fragment },
    });
    const ui = await response.getInterface();
    await (ui as { ok(): Promise<void> }).ok();

    const result = await response;
    // Empty mocked chain => the ephemeral wallet has nothing to sweep.
    expect(errorOf(result)?.message).toMatch(/nothing to claim|no spendable|no outputs|insufficient/i);
    const methods = node.calls.map((c) => c.method);
    expect(methods).toContain('chain_getOutputs'); // pipeline reached the node (scan)
    expect(methods).not.toContain('tx_submit'); // but never submitted without funds
  });

  /* ================================================================== */
  /* Bearer-secret hygiene (#474/#475)                                  */
  /* ================================================================== */

  it('never surfaces the ephemeral mnemonic in a dialog, result, or error', async () => {
    const { mnemonic, fragment } = freshLink();

    // The dialog/error templates legitimately share a few common English tokens
    // with the BIP39 wordlist (e.g. "claim", "empty"); exclude those so a random
    // mnemonic that happens to contain one does not false-fail. We then assert
    // (a) no remaining secret word appears as a standalone token in the output,
    // and (b) the whole mnemonic string never appears verbatim.
    const templateVocab =
      'claim link nothing to this is empty already claimed or not yet confirmed holds ' +
      'funds that will be swept into your wallet the sweep fee is paid from claimable ' +
      'you receive hint cosmetic scanned amount above authoritative node confirm invalid ' +
      'bth does cover';
    const templateTokens = new Set(templateVocab.match(/[a-z]+/g));
    const secretWords = mnemonic.split(' ').filter((w) => !templateTokens.has(w));
    expect(secretWords.length).toBeGreaterThan(0); // guard: filtering did not empty it

    const assertNoLeak = (blob: string): void => {
      const tokens = new Set((blob.toLowerCase().match(/[a-z]+/g) ?? []));
      for (const w of secretWords) expect(tokens.has(w)).toBe(false);
      expect(blob.includes(mnemonic)).toBe(false); // no verbatim secret
    };

    // Preview: check the dialog content AND the returned result.
    {
      const { request } = await installSnap();
      const response = request({
        method: 'botho_previewClaimLink',
        params: { rpcUrl: node.url, link: fragment },
      });
      const ui = await response.getInterface();
      assertNoLeak(JSON.stringify(ui.content));
      await (ui as { ok(): Promise<void> }).ok();
      assertNoLeak(JSON.stringify((await response).response));
    }

    // Claim (approve → "nothing to claim" error): the error must not leak it.
    {
      const { request } = await installSnap();
      const response = request({
        method: 'botho_claimLink',
        params: { rpcUrl: node.url, link: fragment },
      });
      const ui = await response.getInterface();
      assertNoLeak(JSON.stringify(ui.content));
      await (ui as { ok(): Promise<void> }).ok();
      assertNoLeak(JSON.stringify((await response).response));
    }
  });
});
