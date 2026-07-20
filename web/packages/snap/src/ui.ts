/**
 * Snap custom-UI dialog content for receive / balance / send confirmation /
 * mnemonic backup (issue #815, deliverable 4).
 *
 * The Snaps SDK JSX components (`Box`, `Heading`, `Text`, `Row`, `Copyable`,
 * `Divider`) are `SnapComponent` factories — this module calls them directly as
 * functions (the "createElement" style) so no JSX build/transform is needed in
 * the SES bundle; each call returns a plain JSX element object.
 */

import {
  Box,
  Heading,
  Text,
  Row,
  Copyable,
  Divider,
  type JSXElement,
} from '@metamask/snaps-sdk/jsx';

import { shortenAddress } from '@botho/core';

import { formatBTHWithUnit } from './format';
import type { HistoryEntry } from './state';
import type { ContactBook } from './contacts';

/** Host component of an endpoint URL (for compact display), or the raw string. */
function hostOf(rpcUrl: string): string {
  try {
    return new URL(rpcUrl).host;
  } catch {
    return rpcUrl;
  }
}

/** Abbreviate a long hex tx hash for compact dialog display (head…tail). */
function shortHash(txHash: string): string {
  return txHash.length > 20 ? `${txHash.slice(0, 10)}…${txHash.slice(-10)}` : txHash;
}

/** Receive dialog: the wallet's stealth receive address. */
export function receiveContent(address: string): JSXElement {
  return Box({
    children: [
      Heading({ children: 'Receive BTH' }),
      Text({
        children:
          'Share this Botho stealth address to receive funds. A fresh one-time ' +
          'output is created on-chain for every payment, so your balance stays private.',
      }),
      Copyable({ value: address }),
    ],
  });
}

/** Balance dialog: the wallet's spendable balance and its ingress node. */
export function balanceContent(spendablePicocredits: bigint, rpcUrl: string): JSXElement {
  return Box({
    children: [
      Heading({ children: 'Botho balance' }),
      Row({ label: 'Spendable', children: Text({ children: formatBTHWithUnit(spendablePicocredits) }) }),
      Divider({}),
      Row({ label: 'Node', children: Text({ children: hostOf(rpcUrl) }) }),
    ],
  });
}

/** Parameters rendered in the send-confirmation dialog. */
export interface SendConfirmView {
  recipientAddress: string;
  amountPicocredits: bigint;
  feePicocredits: bigint;
  rpcUrl: string;
}

/** Send confirmation dialog: recipient, amount, fee, total, ingress node. */
export function sendConfirmContent(view: SendConfirmView): JSXElement {
  const total = view.amountPicocredits + view.feePicocredits;
  return Box({
    children: [
      Heading({ children: 'Confirm send' }),
      Row({ label: 'Amount', children: Text({ children: formatBTHWithUnit(view.amountPicocredits) }) }),
      Row({ label: 'Network fee', children: Text({ children: formatBTHWithUnit(view.feePicocredits) }) }),
      Row({ label: 'Total', children: Text({ children: formatBTHWithUnit(total) }) }),
      Divider({}),
      Text({ children: 'Recipient' }),
      Copyable({ value: view.recipientAddress }),
      Row({ label: 'Node', children: Text({ children: hostOf(view.rpcUrl) }) }),
    ],
  });
}

/**
 * Transaction-history dialog: the wallet's receive history, newest first, each
 * entry annotated with its live spent/received direction and its finality depth
 * (`confirmations`). A low confirmation count flags a shallow, reorg-prone
 * receive near the tip. Renders an explicit empty-state when there is no history.
 *
 * History is a PURE projection over the persisted scan state (#1091) plus a live
 * spent-check — no rescan, no persisted history record (see `src/state.ts`).
 */
export function historyContent(entries: HistoryEntry[], rpcUrl: string): JSXElement {
  if (entries.length === 0) {
    return Box({
      children: [
        Heading({ children: 'Transaction history' }),
        Text({ children: 'No transactions yet. Payments you receive will appear here.' }),
        Divider({}),
        Row({ label: 'Node', children: Text({ children: hostOf(rpcUrl) }) }),
      ],
    });
  }
  return Box({
    children: [
      Heading({ children: 'Transaction history' }),
      ...entries.flatMap((entry) => [
        Row({
          label: entry.direction === 'spent' ? 'Spent' : 'Received',
          children: Text({ children: formatBTHWithUnit(BigInt(entry.amountPicocredits)) }),
        }),
        Text({
          children:
            `Block ${entry.blockHeight} · ${entry.confirmations} ` +
            `confirmation${entry.confirmations === 1 ? '' : 's'} · ${shortHash(entry.txHash)}`,
        }),
        Divider({}),
      ]),
      Row({ label: 'Node', children: Text({ children: hostOf(rpcUrl) }) }),
    ],
  });
}

/**
 * Contacts dialog: the saved address book, each entry rendering its label, a
 * compact shortened address line, and a `Copyable` of the full address for reuse
 * in a send. Renders an explicit empty-state when no contacts are saved. Add /
 * remove are driven by the `botho_addContact` / `botho_removeContact` RPC methods
 * (dApp-driven); this dialog is view-only (#1093).
 */
export function contactsContent(book: ContactBook): JSXElement {
  if (book.length === 0) {
    return Box({
      children: [
        Heading({ children: 'Contacts' }),
        Text({ children: 'No saved contacts yet. Add one to reuse a Botho address without re-pasting it.' }),
      ],
    });
  }
  return Box({
    children: [
      Heading({ children: 'Contacts' }),
      ...book.flatMap((contact) => [
        Row({ label: contact.label, children: Text({ children: shortenAddress(contact.address) }) }),
        Copyable({ value: contact.address }),
        Divider({}),
      ]),
    ],
  });
}

/** Mnemonic-backup dialog: the derived 24-word Botho recovery phrase. */
export function mnemonicBackupContent(mnemonic: string): JSXElement {
  return Box({
    children: [
      Heading({ children: 'Botho recovery phrase' }),
      Text({
        children:
          'These 24 words are derived from your MetaMask Secret Recovery Phrase ' +
          'and are full spending authority for this Botho wallet. Write them down ' +
          'and keep them offline. Anyone who sees them can spend your funds.',
      }),
      Copyable({ value: mnemonic, sensitive: true }),
    ],
  });
}
