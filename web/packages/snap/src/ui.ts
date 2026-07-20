/**
 * Snap custom-UI dialog content for receive / balance / send confirmation /
 * mnemonic backup (issue #815, deliverable 4).
 *
 * The Snaps SDK JSX components (`Box`, `Heading`, `Text`, `Row`, `Copyable`,
 * `Divider`) are `SnapComponent` factories — this module calls them directly as
 * functions (the "createElement" style) so no JSX build/transform is needed in
 * the SES bundle; each call returns a plain JSX element object.
 *
 * i18n (issue #1095): every user-facing string is sourced from the SES-safe
 * message map via `t(key, locale, params?)` (see `src/i18n.ts`). Each content
 * builder takes the resolved `locale`; `src/index.ts` resolves it once per RPC
 * invocation from `snap_getPreferences.locale`.
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
import { t, confirmationsPhrase, type Locale } from './i18n';
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
export function receiveContent(address: string, locale: Locale): JSXElement {
  return Box({
    children: [
      Heading({ children: t('receive.heading', locale) }),
      Text({ children: t('receive.body', locale) }),
      Copyable({ value: address }),
    ],
  });
}

/** Balance dialog: the wallet's spendable balance and its ingress node. */
export function balanceContent(
  spendablePicocredits: bigint,
  rpcUrl: string,
  locale: Locale,
): JSXElement {
  return Box({
    children: [
      Heading({ children: t('balance.heading', locale) }),
      Row({
        label: t('balance.spendable', locale),
        children: Text({ children: formatBTHWithUnit(spendablePicocredits) }),
      }),
      Divider({}),
      Row({ label: t('common.node', locale), children: Text({ children: hostOf(rpcUrl) }) }),
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
export function sendConfirmContent(view: SendConfirmView, locale: Locale): JSXElement {
  const total = view.amountPicocredits + view.feePicocredits;
  return Box({
    children: [
      Heading({ children: t('send.heading', locale) }),
      Row({
        label: t('send.amount', locale),
        children: Text({ children: formatBTHWithUnit(view.amountPicocredits) }),
      }),
      Row({
        label: t('send.networkFee', locale),
        children: Text({ children: formatBTHWithUnit(view.feePicocredits) }),
      }),
      Row({ label: t('send.total', locale), children: Text({ children: formatBTHWithUnit(total) }) }),
      Divider({}),
      Text({ children: t('send.recipient', locale) }),
      Copyable({ value: view.recipientAddress }),
      Row({ label: t('common.node', locale), children: Text({ children: hostOf(view.rpcUrl) }) }),
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
export function historyContent(
  entries: HistoryEntry[],
  rpcUrl: string,
  locale: Locale,
): JSXElement {
  if (entries.length === 0) {
    return Box({
      children: [
        Heading({ children: t('history.heading', locale) }),
        Text({ children: t('history.empty', locale) }),
        Divider({}),
        Row({ label: t('common.node', locale), children: Text({ children: hostOf(rpcUrl) }) }),
      ],
    });
  }
  return Box({
    children: [
      Heading({ children: t('history.heading', locale) }),
      ...entries.flatMap((entry) => [
        Row({
          label:
            entry.direction === 'spent'
              ? t('history.spent', locale)
              : t('history.received', locale),
          children: Text({ children: formatBTHWithUnit(BigInt(entry.amountPicocredits)) }),
        }),
        Text({
          children: t('history.line', locale, {
            height: entry.blockHeight,
            confirmations: confirmationsPhrase(entry.confirmations, locale),
            hash: shortHash(entry.txHash),
          }),
        }),
        Divider({}),
      ]),
      Row({ label: t('common.node', locale), children: Text({ children: hostOf(rpcUrl) }) }),
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
export function contactsContent(book: ContactBook, locale: Locale): JSXElement {
  if (book.length === 0) {
    return Box({
      children: [
        Heading({ children: t('contacts.heading', locale) }),
        Text({ children: t('contacts.empty', locale) }),
      ],
    });
  }
  return Box({
    children: [
      Heading({ children: t('contacts.heading', locale) }),
      ...book.flatMap((contact) => [
        Row({ label: contact.label, children: Text({ children: shortenAddress(contact.address) }) }),
        Copyable({ value: contact.address }),
        Divider({}),
      ]),
    ],
  });
}

/** Amounts + context rendered in the claim-link preview / confirm dialogs. */
export interface ClaimView {
  /** Gross spendable picocredits the ephemeral link wallet holds. */
  grossPicocredits: bigint;
  /** Sweep fee (network minimum) charged from the funded output. */
  feePicocredits: bigint;
  /** Net picocredits the user receives after the sweep fee. */
  netPicocredits: bigint;
  /** Optional cosmetic hint carried in the link (never authoritative). */
  amountHint?: bigint;
  /** The ingress node the scan / sweep runs against. */
  rpcUrl: string;
}

/**
 * Shared body rows for the claim-link dialogs: the scanned claimable / fee / net
 * amounts, an optional cosmetic hint line, and the ingress node. The SCANNED
 * amount is always authoritative; the `amountHint` (if present) is shown only as
 * a secondary, pre-scan cosmetic and is explicitly labelled as non-authoritative
 * (per `@botho/core` `claim-link.ts`). The bearer secret (mnemonic) is NEVER
 * rendered.
 */
function claimBodyRows(view: ClaimView, locale: Locale): JSXElement[] {
  const empty = view.grossPicocredits === 0n;
  const rows: JSXElement[] = [
    empty
      ? Text({ children: t('claim.empty', locale) })
      : Text({ children: t('claim.body', locale) }),
    Row({
      label: t('claim.claimable', locale),
      children: Text({ children: formatBTHWithUnit(view.grossPicocredits) }),
    }),
    Row({
      label: t('claim.sweepFee', locale),
      children: Text({ children: formatBTHWithUnit(view.feePicocredits) }),
    }),
    Row({
      label: t('claim.youReceive', locale),
      children: Text({ children: formatBTHWithUnit(view.netPicocredits) }),
    }),
  ];
  if (view.amountHint !== undefined) {
    rows.push(
      Text({
        children: t('claim.hint', locale, { amount: formatBTHWithUnit(view.amountHint) }),
      }),
    );
  }
  rows.push(Divider({}));
  rows.push(Row({ label: t('common.node', locale), children: Text({ children: hostOf(view.rpcUrl) }) }));
  return rows;
}

/**
 * Claim-link PREVIEW dialog (alert): a read-only scan of what a claim link holds
 * (`botho_previewClaimLink`). Renders the scanned claimable / fee / net and the
 * ingress node; does not submit anything.
 */
export function claimPreviewContent(view: ClaimView, locale: Locale): JSXElement {
  return Box({
    children: [
      Heading({ children: t('claim.previewHeading', locale) }),
      ...claimBodyRows(view, locale),
    ],
  });
}

/**
 * Claim-link CONFIRM dialog (confirmation): the same figures as the preview, but
 * gated behind an explicit approve/reject before the sweep is built + submitted
 * (`botho_claimLink`). Mirrors the `botho_send` confirmation.
 */
export function claimConfirmContent(view: ClaimView, locale: Locale): JSXElement {
  return Box({
    children: [
      Heading({ children: t('claim.confirmHeading', locale) }),
      ...claimBodyRows(view, locale),
    ],
  });
}

/** Fields rendered in the payment-request (pull payment) preview dialog. */
export interface PaymentRequestView {
  /** The requester's PUBLIC address to pay — echoed in full for verification. */
  to: string;
  /** Requested amount in picocredits, if the link carried one (payer chooses if absent). */
  amountPicocredits?: bigint;
  /** Optional human-readable note attached to the request. */
  memo?: string;
}

/**
 * Payment-request PREVIEW dialog (alert): shows who a `/pay#…` link asks the user
 * to pay, the requested amount (or "any amount" when the link leaves it open),
 * and an optional memo (`botho_showPaymentRequest`). A request link carries only
 * the requester's PUBLIC address — nothing secret — so, unlike the claim dialogs,
 * every field is shown in the clear (the payer must verify the address). The
 * actual payment is a separate, param-driven `botho_send` prefilled from these
 * fields; this dialog does not submit anything.
 */
export function paymentRequestContent(view: PaymentRequestView, locale: Locale): JSXElement {
  const rows: JSXElement[] = [
    Heading({ children: t('request.heading', locale) }),
    Text({ children: t('request.body', locale) }),
    Row({
      label: t('request.amount', locale),
      children: Text({
        children:
          view.amountPicocredits !== undefined
            ? formatBTHWithUnit(view.amountPicocredits)
            : t('request.amountAny', locale),
      }),
    }),
  ];
  if (view.memo !== undefined) {
    rows.push(Row({ label: t('request.memo', locale), children: Text({ children: view.memo }) }));
  }
  rows.push(Divider({}));
  rows.push(Text({ children: t('request.payTo', locale) }));
  rows.push(Copyable({ value: view.to }));
  return Box({ children: rows });
}

/** Mnemonic-backup dialog: the derived 24-word Botho recovery phrase. */
export function mnemonicBackupContent(mnemonic: string, locale: Locale): JSXElement {
  return Box({
    children: [
      Heading({ children: t('mnemonic.heading', locale) }),
      Text({ children: t('mnemonic.body', locale) }),
      Copyable({ value: mnemonic, sensitive: true }),
    ],
  });
}
