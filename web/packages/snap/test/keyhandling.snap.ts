/**
 * Key-handling security regression tests (#1096).
 *
 * The Botho Snap's key-handling surface was designed carefully — the doc-comments
 * across `derivation.ts` / `claim.ts` / `state.ts` promise "keys never leave the
 * sandbox", "the bearer secret is never surfaced", and "no secret is written to
 * persisted state". Those were *prose promises*, not *enforced invariants*: a
 * later refactor could silently add a secret to an error message, a debug field to
 * an RPC result, or a key to the `snap_manageState` blob and nothing would catch
 * it. This suite turns each promise into an ASSERTED invariant, mirroring the
 * closed web-wallet at-rest audit (#474 claim-link bearer secrets in plaintext /
 * #475 seed-at-rest). See `audits/2026-07-20-snap-keyhandling.md`.
 *
 * Two layers, matching `state.snap.ts`:
 *
 *  A. WRITE-BOUNDARY capture (no `installSnap`): a `snap_manageState`-capturing
 *     stub proves that the persisted blob produced by `incrementalScan` /
 *     `writeContacts` carries only public/derived data — the private `SignerKeys`
 *     passed IN never appear in what is written OUT (the #475/#476 class). This is
 *     the direct proof of the "no secret in state" invariant: the SES simulation
 *     harness (snaps-simulation 4.x) exposes no state getter, so the blob is
 *     inspected at the write boundary instead (documented in the audit).
 *
 *  B. SES-HARNESS behaviour (`@metamask/snaps-jest`): drives the real RPC methods
 *     inside the SES executor against a mocked node and asserts no secret material
 *     (a known bearer mnemonic, the derived wallet's phrase, its private-key/seed
 *     hex) appears in any RPC RESULT, any `snap_dialog` CONTENT, or any thrown
 *     ERROR — including after a persisted round-trip.
 */

import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

import { afterEach, describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';
import {
  deriveKeypairs,
  createClaimLinkMnemonic,
  encodeClaimLinkFragment,
} from '@botho/core';
import {
  mnemonicToSeedHex,
  type ChainOutputWithMeta,
  type SignerKeys,
  type WasmSigner,
} from '@botho/wasm-signer';

import { incrementalScan } from '../src/state';
import { writeContacts } from '../src/contacts';
import { startMockNode, type MockNode } from './mock-node';

const toHex = (b: Uint8Array): string =>
  Array.from(b)
    .map((x) => x.toString(16).padStart(2, '0'))
    .join('');

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

/** Recursively collect every `Copyable`/field `value` string in a dialog tree. */
function collectValues(node: unknown, acc: string[] = []): string[] {
  if (!node || typeof node !== 'object') return acc;
  const n = node as { value?: unknown; props?: { value?: unknown; children?: unknown }; children?: unknown };
  const value = n.props?.value ?? n.value;
  if (typeof value === 'string') acc.push(value);
  const children = n.props?.children ?? n.children;
  if (Array.isArray(children)) children.forEach((c) => collectValues(c, acc));
  else if (children) collectValues(children, acc);
  return acc;
}

/** The 24-word phrase rendered in a mnemonic-backup alert (the sensitive Copyable). */
function extractPhrase(content: unknown): string {
  const phrase = collectValues(content).find((v) => v.trim().split(/\s+/).length === 24);
  if (!phrase) throw new Error('no 24-word phrase found in dialog content');
  return phrase;
}

/** A fixed, KNOWN wallet whose secrets we search the persisted blob for. */
const KNOWN_MNEMONIC = createClaimLinkMnemonic();
const KNOWN_KP = deriveKeypairs(KNOWN_MNEMONIC, 0);
const KNOWN_KEYS: SignerKeys = {
  spendPrivateKey: toHex(KNOWN_KP.spendPrivate),
  viewPrivateKey: toHex(KNOWN_KP.viewPrivate),
  seed: mnemonicToSeedHex(KNOWN_MNEMONIC),
};
const KNOWN_SECRETS = [
  KNOWN_MNEMONIC,
  KNOWN_KEYS.spendPrivateKey,
  KNOWN_KEYS.viewPrivateKey,
  KNOWN_KEYS.seed as string,
];

/* ================================================================== */
/* A. Write-boundary: no secret is ever written to persisted state    */
/*    (#475/#476 class — proven at the snap_manageState boundary)      */
/* ================================================================== */

describe('key-handling: persisted-state write boundary (#1096, F4)', () => {
  afterEach(() => {
    delete (globalThis as unknown as { snap?: unknown }).snap;
  });

  /** Install a capturing `snap_manageState` stub; returns the write log. */
  function stubManageState(): { updates: Record<string, unknown>[]; current(): unknown } {
    let stored: Record<string, unknown> | null = null;
    const updates: Record<string, unknown>[] = [];
    (globalThis as unknown as { snap: unknown }).snap = {
      request: async ({
        method,
        params,
      }: {
        method: string;
        params?: { operation?: string; newState?: Record<string, unknown> };
      }) => {
        if (method !== 'snap_manageState') {
          throw new Error(`unexpected snap.request in capture stub: ${method}`);
        }
        switch (params?.operation) {
          case 'get':
            return stored;
          case 'update':
            stored = params.newState ?? null;
            if (stored) updates.push(stored);
            return null;
          case 'clear':
            stored = null;
            return null;
          default:
            return null;
        }
      },
    };
    return { updates, current: () => stored };
  }

  it('incrementalScan persists owned outputs (public) but NEVER the private keys it scans with', async () => {
    const captured = stubManageState();

    // A single owned output the fake signer "discovers" in the scanned window.
    const raw: ChainOutputWithMeta = {
      targetKey: 'aa'.repeat(32),
      publicKey: 'bb'.repeat(32),
      amount: 1_000_000_000_000n,
      outputIndex: 0,
      kemCiphertext: null,
      txHash: 'cc'.repeat(32),
      height: 5,
    };
    const fakeSigner = {
      scanOwnedOutputs: ({ outputs }: { outputs: Array<Record<string, unknown>> }) =>
        outputs.map((o) => ({
          targetKey: o.targetKey,
          publicKey: o.publicKey,
          amount: o.amount,
          subaddressIndex: 0n,
          outputIndex: o.outputIndex,
          kemCiphertext: o.kemCiphertext ?? null,
        })),
      computeOwnedOutputKeyImages: ({ outputs }: { outputs: Array<Record<string, unknown>> }) =>
        outputs.map((o, i) => ({ ...o, keyImage: `ki${i}` })),
    } as unknown as WasmSigner;

    await incrementalScan({
      signer: fakeSigner,
      keys: KNOWN_KEYS,
      network: 'botho-testnet',
      tip: 10,
      fetchWindow: async (lo, hi) => (lo <= 5 && hi >= 5 ? [raw] : []),
      sendRpc: {
        getChainHeight: async () => 10,
        getOutputs: async () => [],
        areKeyImagesSpent: async (keyImages) =>
          keyImages.map((keyImage) => ({ keyImage, spent: false, spentHeight: null, pending: false })),
      },
    });

    const blob = JSON.stringify(captured.current());
    // Non-vacuous: the write really happened and persisted the PUBLIC receive facts.
    expect(blob).toContain(raw.targetKey);
    expect(blob).toContain('botho-testnet');
    // The wallet keys were an INPUT to the scan but must never be written out.
    for (const secret of KNOWN_SECRETS) expect(blob.includes(secret)).toBe(false);
  });

  it('writeContacts persists only the (public) address book, no key material', async () => {
    const captured = stubManageState();
    await writeContacts([
      { id: 'deadbeef', label: 'My other wallet', address: 'tbotho://2/public-address' },
    ]);

    const blob = JSON.stringify(captured.current());
    expect(blob).toContain('My other wallet'); // non-vacuous: contact persisted
    expect(blob).toContain('tbotho://2/public-address');
    for (const secret of KNOWN_SECRETS) expect(blob.includes(secret)).toBe(false);
  });

  it('src/ contains no console.* sink (secrets can never reach a log)', () => {
    const srcDir = join(__dirname, '..', 'src');
    const offenders: string[] = [];
    const walk = (dir: string): void => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const p = join(dir, entry.name);
        if (entry.isDirectory()) walk(p);
        else if (entry.name.endsWith('.ts') && /\bconsole\s*\./.test(readFileSync(p, 'utf8'))) {
          offenders.push(p);
        }
      }
    };
    walk(srcDir);
    expect(offenders).toEqual([]);
  });
});

/* ================================================================== */
/* B. SES harness: no secret in any result / dialog / error           */
/* ================================================================== */

describe('key-handling: no secret in RPC result / dialog / error (#1096, F2/F3)', () => {
  let node: MockNode | undefined;

  afterEach(async () => {
    if (node) {
      await node.close();
      node = undefined; // don't re-close in a later test that starts no node
    }
  });

  it('claim link: the ephemeral bearer mnemonic never appears in a result, dialog, or error (F2)', async () => {
    node = await startMockNode();
    const mnemonic = createClaimLinkMnemonic();
    const fragment = encodeClaimLinkFragment(mnemonic);

    // Dialog/error templates legitimately share a few English tokens with the
    // BIP39 wordlist; exclude those so a random mnemonic that happens to contain
    // one does not false-fail. We then assert no remaining secret word appears as
    // a standalone token, and the whole phrase never appears verbatim.
    const templateVocab =
      'claim link nothing to this is empty already claimed or not yet confirmed holds ' +
      'funds that will be swept into your wallet the sweep fee is paid from claimable ' +
      'you receive hint cosmetic scanned amount above authoritative node confirm invalid ' +
      'bth does cover reject user rejected the';
    const templateTokens = new Set(templateVocab.match(/[a-z]+/g));
    const secretWords = mnemonic.split(' ').filter((w) => !templateTokens.has(w));
    expect(secretWords.length).toBeGreaterThan(0); // filtering did not empty it

    const assertNoLeak = (blob: string): void => {
      const tokens = new Set(blob.toLowerCase().match(/[a-z]+/g) ?? []);
      for (const w of secretWords) expect(tokens.has(w)).toBe(false);
      expect(blob.includes(mnemonic)).toBe(false);
    };

    const { request } = await installSnap();

    // Preview: dialog content + returned result.
    const preview = request({ method: 'botho_previewClaimLink', params: { rpcUrl: node.url, link: fragment } });
    const previewUi = await preview.getInterface();
    assertNoLeak(JSON.stringify(previewUi.content));
    await (previewUi as { ok(): Promise<void> }).ok();
    assertNoLeak(JSON.stringify((await preview).response));

    // Claim (approve → "nothing to claim" error on the empty mock): dialog + error.
    const claim = request({ method: 'botho_claimLink', params: { rpcUrl: node.url, link: fragment } });
    const claimUi = await claim.getInterface();
    assertNoLeak(JSON.stringify(claimUi.content));
    await (claimUi as { ok(): Promise<void> }).ok();
    const claimed = await claim;
    expect(errorOf(claimed)?.message).toBeTruthy();
    assertNoLeak(JSON.stringify(claimed.response));
  });

  it('botho_showMnemonic: masked before confirm, phrase only in a sensitive Copyable after, never in the result (F3)', async () => {
    const { request } = await installSnap();
    const response = request({ method: 'botho_showMnemonic' });

    // Pre-confirm dialog: the masked placeholder, not the real words.
    const confirmUi = await response.getInterface();
    expect(confirmUi.type).toBe('confirmation');
    const preConfirm = JSON.stringify(confirmUi.content);
    expect(preConfirm).toContain('••••');
    await (confirmUi as { ok(): Promise<void> }).ok();

    // Post-confirm alert: the real phrase, flagged sensitive.
    const alertUi = await response.getInterface();
    const revealed = JSON.stringify(alertUi.content);
    expect(revealed).toContain('"sensitive":true');
    const phrase = extractPhrase(alertUi.content);
    expect(phrase.split(/\s+/)).toHaveLength(24);
    await (alertUi as { ok(): Promise<void> }).ok();

    // The pre-confirm dialog must NOT render the real phrase (it shows the mask).
    // A per-word token check would false-positive on the backup body copy, which
    // legitimately shares common English words with the BIP39 wordlist ("them",
    // "your", …); the verbatim-phrase check is what precisely guards the mask.
    expect(preConfirm.includes(phrase)).toBe(false);
    // And no run of the real ordered words (defeats a reordered/partial render).
    const orderedWords = phrase.split(/\s+/);
    for (let i = 0; i + 3 < orderedWords.length; i++) {
      const window = orderedWords.slice(i, i + 4).join(' ');
      expect(preConfirm.toLowerCase().includes(window)).toBe(false);
    }

    // The RPC result is just {revealed:true} — never the phrase.
    const result = unwrap<{ revealed: boolean }>(await response);
    expect(result).toEqual({ revealed: true });
    expect(JSON.stringify(result).includes(phrase)).toBe(false);
  });

  it('botho_showMnemonic: rejecting reveals nothing (fixed decline error, no phrase) (F3)', async () => {
    const { request } = await installSnap();
    const response = request({ method: 'botho_showMnemonic' });
    const ui = await response.getInterface();
    await (ui as { cancel(): Promise<void> }).cancel();

    const rejected = await response;
    const message = errorOf(rejected)?.message ?? '';
    expect(message).toMatch(/declined to reveal/i);
    // A fixed i18n decline string — no 24-word phrase can hide in it.
    expect(message.split(/\s+/).length).toBeLessThan(12);
  });

  it('no derived wallet secret appears in any silent-read RPC result, even after a persisted round-trip (F2/F4)', async () => {
    node = await startMockNode();
    const { request } = await installSnap();

    // Learn the wallet's real secret material by revealing the mnemonic once,
    // then derive its private keys / seed in-test to search results for.
    const reveal = request({ method: 'botho_showMnemonic' });
    const confirmUi = await reveal.getInterface();
    await (confirmUi as { ok(): Promise<void> }).ok();
    const alertUi = await reveal.getInterface();
    const phrase = extractPhrase(alertUi.content);
    await (alertUi as { ok(): Promise<void> }).ok();
    await reveal;

    const kp = deriveKeypairs(phrase, 0);
    const secrets = [phrase, toHex(kp.spendPrivate), toHex(kp.viewPrivate), mnemonicToSeedHex(phrase)];
    const assertNoSecret = (blob: string): void => {
      for (const s of secrets) expect(blob.includes(s)).toBe(false);
    };

    // getAddress gives us a real (public) address to save as a contact.
    const { address } = unwrap<{ address: string }>(await request({ method: 'botho_getAddress' }));
    assertNoSecret(JSON.stringify(address));

    // Balance read persists scan state; addContact persists the contacts book.
    assertNoSecret(
      JSON.stringify(unwrap(await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }))),
    );
    unwrap(await request({ method: 'botho_addContact', params: { label: 'Self', address } }));

    // Read everything back through the persisted-state-backed paths.
    assertNoSecret(
      JSON.stringify(unwrap(await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }))),
    );
    assertNoSecret(
      JSON.stringify(unwrap(await request({ method: 'botho_getHistory', params: { rpcUrl: node.url } }))),
    );
    assertNoSecret(JSON.stringify(unwrap(await request({ method: 'botho_listContacts' }))));
  });
});
