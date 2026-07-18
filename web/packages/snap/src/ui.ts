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

import { formatBTHWithUnit } from './format';

/** Host component of an endpoint URL (for compact display), or the raw string. */
function hostOf(rpcUrl: string): string {
  try {
    return new URL(rpcUrl).host;
  } catch {
    return rpcUrl;
  }
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
