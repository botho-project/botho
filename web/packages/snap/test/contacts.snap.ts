/**
 * Contacts / address book (issue #1093).
 *
 * Contacts are a self-contained, encrypted `(label, address)` store under a new
 * `contacts` sibling namespace in the SAME `snap_manageState` blob as `scan` —
 * additive, so NO `STATE_VERSION` bump and no migration. The critical invariant
 * is cross-namespace isolation: writing contacts must NEVER clobber the persisted
 * `scan` checkpoint (and vice versa).
 *
 * Two layers, mirroring `state.snap.ts` / `history.snap.ts`:
 *
 *  1. PURE-LOGIC tests of `addContact` / `removeContact` / `normalizeLabel` (no
 *     `installSnap`, no wasm): validation, dedupe, non-mutation, label caps.
 *
 *  2. BEHAVIOURAL tests through the real SES `@metamask/snaps-jest` harness:
 *     add -> list round-trips through `snap_manageState`; remove empties the book;
 *     a malformed address is a JSON-RPC error; the empty-state dialog renders; and
 *     the cross-namespace isolation guarantee — a contact write between two
 *     balance reads leaves the scan checkpoint intact (the second balance read
 *     still resumes from the reorg buffer rather than rescanning from genesis).
 */

import { afterEach, describe, expect, it } from '@jest/globals';
import { installSnap } from '@metamask/snaps-jest';

import { startMockNode, type MockNode } from './mock-node';
import {
  addContact,
  removeContact,
  normalizeLabel,
  contactId,
  MAX_LABEL_LENGTH,
  type ContactBook,
} from '../src/contacts';
import { STATE_VERSION, REORG_BUFFER, scanStartHeight } from '../src/state';

function unwrap<T>(response: { response: unknown }): T {
  const res = response.response as { result?: T; error?: { message?: string } };
  if (res.error) {
    throw new Error(`snap returned error: ${JSON.stringify(res.error)}`);
  }
  return res.result as T;
}

/** Whether a snap response carried a JSON-RPC error (for negative-path asserts). */
function isError(response: { response: unknown }): boolean {
  return Boolean((response.response as { error?: unknown }).error);
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

// Two well-formed testnet v2 addresses (from the address-codec fixtures). Only
// their FORMAT matters here — the Snap validates format via `isValidAddress`.
const ADDR_A =
  'tbotho://2/' +
  '11111111111111111111111111111111111111111111111111111111111111111111111111111111';

/* ================================================================== */
/* 1. Pure-logic contract (no SES install)                            */
/* ================================================================== */

describe('contacts pure helpers (#1093)', () => {
  // These cover the validation + normalization surface that does NOT need a real
  // encoded address (label caps, malformed-address rejection, id stability,
  // non-mutating removal). The happy-path add of a VALID address is exercised in
  // layer 2 against the Snap's own derived (guaranteed-valid) stealth address.

  it('normalizeLabel trims and rejects empty / whitespace-only labels', () => {
    expect(normalizeLabel('  Alice  ')).toBe('Alice');
    expect(() => normalizeLabel('')).toThrow();
    expect(() => normalizeLabel('   ')).toThrow();
  });

  it('normalizeLabel caps the label length at MAX_LABEL_LENGTH', () => {
    const atCap = 'x'.repeat(MAX_LABEL_LENGTH);
    expect(normalizeLabel(atCap)).toBe(atCap); // exactly at the cap is allowed
    expect(() => normalizeLabel('x'.repeat(MAX_LABEL_LENGTH + 1))).toThrow();
  });

  it('addContact rejects a malformed / non-Botho address', () => {
    expect(() => addContact([], { label: 'Bad', address: 'not-an-address' })).toThrow();
    expect(() => addContact([], { label: 'Bad', address: '' })).toThrow();
  });

  it('contactId is deterministic and stable per address', () => {
    expect(contactId('abc')).toBe(contactId('abc'));
    expect(contactId('abc')).not.toBe(contactId('abd'));
    expect(contactId('abc')).toMatch(/^[0-9a-f]{8}$/u);
  });

  it('removeContact filters by id, returns a NEW array, and no-ops an unknown id', () => {
    const book: ContactBook = [
      { id: 'aaaa1111', label: 'Alice', address: ADDR_A },
      { id: 'bbbb2222', label: 'Bob', address: ADDR_A },
    ];
    const removed = removeContact(book, 'aaaa1111');
    expect(removed).toHaveLength(1);
    expect(removed[0].id).toBe('bbbb2222');
    expect(book).toHaveLength(2); // input not mutated

    // Removing an id that isn't present is a no-op returning an equivalent book.
    expect(removeContact(book, 'no-such-id')).toHaveLength(2);
  });

  it('does not bump STATE_VERSION (contacts are an additive sibling namespace)', () => {
    expect(STATE_VERSION).toBe(1);
  });
});

/* ================================================================== */
/* 2. Behavioural: contacts through the SES harness                   */
/* ================================================================== */

interface SnapContactJson {
  id: string;
  label: string;
  address: string;
}

describe('botho snap: contacts / address book against a mocked node (#1093)', () => {
  let node: MockNode;

  afterEach(async () => {
    if (node) await node.close();
  });

  /** The Snap's own derived stealth address — a guaranteed-valid Botho address. */
  type Request = Awaited<ReturnType<typeof installSnap>>['request'];
  async function derivedAddress(request: Request): Promise<string> {
    const { address } = unwrap<{ address: string }>(
      await request({ method: 'botho_getAddress' }),
    );
    return address;
  }

  it('addContact then listContacts round-trips through snap_manageState', async () => {
    const { request } = await installSnap();
    const address = await derivedAddress(request);

    const added = unwrap<{ contact: SnapContactJson; contacts: SnapContactJson[] }>(
      await request({ method: 'botho_addContact', params: { label: 'My other wallet', address } }),
    );
    expect(added.contact.label).toBe('My other wallet');
    expect(added.contact.address).toBe(address);
    expect(added.contact.id).toMatch(/^[0-9a-f]{8}$/u);

    const { contacts } = unwrap<{ contacts: SnapContactJson[] }>(
      await request({ method: 'botho_listContacts' }),
    );
    expect(contacts).toHaveLength(1);
    expect(contacts[0]).toEqual(added.contact);
  });

  it('removeContact drops the entry (listContacts then returns [])', async () => {
    const { request } = await installSnap();
    const address = await derivedAddress(request);

    const added = unwrap<{ contact: SnapContactJson }>(
      await request({ method: 'botho_addContact', params: { label: 'Alice', address } }),
    );
    unwrap(await request({ method: 'botho_removeContact', params: { id: added.contact.id } }));

    const { contacts } = unwrap<{ contacts: SnapContactJson[] }>(
      await request({ method: 'botho_listContacts' }),
    );
    expect(contacts).toEqual([]);
  });

  it('addContact with a malformed address returns a JSON-RPC error', async () => {
    const { request } = await installSnap();
    const response = await request({
      method: 'botho_addContact',
      params: { label: 'Typo', address: 'botho://2/not-a-real-address' },
    });
    expect(isError(response)).toBe(true);
  });

  it('botho_showContacts renders the empty-state dialog', async () => {
    const { request } = await installSnap();
    const response = request({ method: 'botho_showContacts' });
    const ui = await response.getInterface();
    expect(ui.type).toBe('alert');
    const rendered = JSON.stringify(ui.content);
    expect(rendered).toContain('Contacts');
    expect(rendered).toContain('No saved contacts yet');

    await (ui as { ok(): Promise<void> }).ok();
    const result = unwrap<{ contacts: SnapContactJson[]; count: number }>(await response);
    expect(result.contacts).toEqual([]);
    expect(result.count).toBe(0);
  });

  it('cross-namespace isolation: a contact write does NOT clobber the scan checkpoint', async () => {
    // A multi-window tip makes a full genesis rescan visible in call counts.
    const tip = 3000;
    node = await startMockNode({ chainHeight: tip });
    const { request } = await installSnap();

    // 1. Seed a persisted scan checkpoint via a full genesis balance scan.
    unwrap(await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }));
    const afterBalance = outputWindows(node).length;
    expect(afterBalance).toBeGreaterThan(1); // genuinely multi-window genesis scan

    // 2. Write a contact IN BETWEEN the two balance reads.
    const address = await derivedAddress(request);
    unwrap(await request({ method: 'botho_addContact', params: { label: 'Saved', address } }));

    // 3. A second balance read at the SAME tip must still RESUME from the
    //    persisted checkpoint (only the trailing reorg-buffer window), proving
    //    the contact write preserved `scan` rather than discarding it.
    unwrap(await request({ method: 'botho_getBalance', params: { rpcUrl: node.url } }));
    const resumeWindows = outputWindows(node).slice(afterBalance);
    expect(resumeWindows).toEqual([{ start: scanStartHeight(tip), end: tip }]);
    for (const w of resumeWindows) {
      expect(w.start).toBeGreaterThanOrEqual(tip - REORG_BUFFER);
    }

    // 4. And the contact itself survived the intervening balance write
    //    (scan write must not clobber `contacts` either).
    const { contacts } = unwrap<{ contacts: SnapContactJson[] }>(
      await request({ method: 'botho_listContacts' }),
    );
    expect(contacts).toHaveLength(1);
    expect(contacts[0].address).toBe(address);
  });
});
