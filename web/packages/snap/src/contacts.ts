/**
 * Snap contacts / address book (Phase-2, issue #1093).
 *
 * A small, self-contained store of `(label, address)` pairs so a user does not
 * have to re-paste an opaque `botho://2/…` stealth address every time they send
 * to the same counterparty. It is the Snap analogue of the web wallet's
 * `@botho/core` `EncryptedAddressBook` (#476) — a DIFFERENT runtime and storage
 * surface (`snap_manageState` vs browser localStorage/`VaultKey`), so it reuses
 * the shared address-validation utility but not that localStorage implementation.
 *
 * Design (mirrors the pure-helper + thin-handler split of `state.ts`/`index.ts`):
 *
 *  - **Independent of scan/history.** Contacts carry no on-chain facts, need no
 *    rescan and no wasm. This module depends ONLY on `readState`/`writeState`/
 *    `STATE_VERSION` from `state.ts`.
 *
 *  - **Purely additive persistence, NO `STATE_VERSION` bump.** The book is stored
 *    under a new `contacts` sibling key alongside `scan` in the same encrypted
 *    `{ version, scan, contacts }` blob. `usableScanState()` only version-gates
 *    the `scan` namespace, and `incrementalScan()` spreads the persisted blob
 *    before writing `scan`, so a `contacts` key survives every balance/history
 *    write. {@link writeContacts} MUST honour that invariant symmetrically — it
 *    spreads the persisted blob (preserving `scan`) so writing contacts never
 *    clobbers the scan checkpoint. Bumping the version would wrongly discard the
 *    scan checkpoint and force a full genesis rescan, so the version stays `1`.
 *
 *  - **Validated + normalized.** Addresses are validated with the shared
 *    `isValidAddress` from `@botho/core` (no hand-rolled regex); labels are
 *    trimmed, rejected if empty, and length-capped so a runaway label can't bloat
 *    the encrypted blob. An invalid address / bad label throws the same
 *    `InvalidParamsError` the RPC param helpers already use.
 */

import { InvalidParamsError } from '@metamask/snaps-sdk';
import { isValidAddress } from '@botho/core';

import { readState, writeState, STATE_VERSION } from './state';

/**
 * Maximum stored contact-label length (characters, after trimming). Caps the
 * per-entry size so a pathological label can't inflate the encrypted state blob.
 */
export const MAX_LABEL_LENGTH = 64;

/**
 * A single saved contact. Deliberately LEAN — the Snap tracks only a validated
 * Botho address + a user label, NOT the richer `@botho/core` `Contact`
 * (`txCount`/`lastTxAt`/`notes`) the web wallet keeps. Every field is JSON-safe.
 */
export interface SnapContact {
  /** Stable id, derived deterministically from the normalized address. */
  id: string;
  /** User-supplied label: trimmed, non-empty, length-capped. */
  label: string;
  /** A validated `botho://2/…` (or `tbotho://2/…`) address. */
  address: string;
}

/** The persisted contacts collection (the `contacts` state namespace). */
export type ContactBook = SnapContact[];

/* ------------------------------------------------------------------ */
/* Pure helpers (unit-testable without the SES `snap` global)         */
/* ------------------------------------------------------------------ */

/**
 * Normalize a user-supplied label: trim surrounding whitespace, reject an
 * empty/whitespace-only value, and cap the length at {@link MAX_LABEL_LENGTH}.
 * Throws {@link InvalidParamsError} on an empty or over-length label.
 */
export function normalizeLabel(label: string): string {
  const trimmed = (label ?? '').trim();
  if (trimmed.length === 0) {
    throw new InvalidParamsError('Contact label must be a non-empty string.');
  }
  if (trimmed.length > MAX_LABEL_LENGTH) {
    throw new InvalidParamsError(
      `Contact label must be at most ${MAX_LABEL_LENGTH} characters.`,
    );
  }
  return trimmed;
}

/**
 * Derive a stable, deterministic id for a contact from its (already-normalized)
 * address via a 32-bit FNV-1a hash rendered as fixed-width hex. Deterministic so
 * {@link addContact} stays pure (no `crypto` global needed) and the same address
 * always maps to the same id — which pairs naturally with the address-dedupe.
 */
export function contactId(address: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < address.length; i++) {
    hash ^= address.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16).padStart(8, '0');
}

/**
 * Add a `(label, address)` pair to the book, returning a NEW array (never
 * mutates the input). Validates the address with the shared `isValidAddress`
 * (rejecting an invalid one with {@link InvalidParamsError}), normalizes the
 * label, and rejects a duplicate address (same normalized address already saved)
 * to keep the book clean.
 */
export function addContact(
  book: ContactBook,
  entry: { label: string; address: string },
): ContactBook {
  const address = (entry.address ?? '').trim();
  if (!isValidAddress(address)) {
    throw new InvalidParamsError(
      'Contact address is not a valid Botho address (expected botho://2/ or tbotho://2/).',
    );
  }
  const label = normalizeLabel(entry.label);
  if (book.some((c) => c.address === address)) {
    throw new InvalidParamsError('A contact with this address already exists.');
  }
  return [...book, { id: contactId(address), label, address }];
}

/**
 * Remove the contact with the given id, returning a NEW array (never mutates the
 * input). Removing a non-existent id is a no-op that returns an equivalent book.
 */
export function removeContact(book: ContactBook, id: string): ContactBook {
  return book.filter((c) => c.id !== id);
}

/* ------------------------------------------------------------------ */
/* State wrappers (enforce the scan-preserving invariant)             */
/* ------------------------------------------------------------------ */

/**
 * Read the persisted contact book, or `[]` if the Snap has never written one.
 * A silent read — no dialog, no network.
 */
export async function readContacts(): Promise<ContactBook> {
  const state = await readState();
  return state?.contacts ?? [];
}

/**
 * Persist the contact book under the `contacts` namespace. CRITICAL: spreads the
 * existing persisted blob first so the sibling `scan` checkpoint (and any other
 * namespace) is preserved — writing contacts must NEVER clobber scan/history
 * state. `STATE_VERSION` is written unchanged (no bump, no migration).
 */
export async function writeContacts(book: ContactBook): Promise<void> {
  const state = await readState();
  await writeState({ version: STATE_VERSION, ...(state ?? {}), contacts: book });
}
