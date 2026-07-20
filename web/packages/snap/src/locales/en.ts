/**
 * English message map — the FALLBACK locale and the source of truth for the key
 * set (issue #1095). `es`/`zh` are typed against `keyof typeof en`, so a missing
 * translation is a compile error, and `test/i18n.snap.ts` asserts key parity at
 * runtime as well.
 *
 * Values here reproduce the exact English literals that previously lived inline
 * in `src/ui.ts` / `src/index.ts`, so English rendering is byte-for-byte
 * unchanged.
 *
 * `{placeholder}` tokens are substituted by the Intl-free `t()` in `../i18n`
 * (plain string replace — no `Intl`, matching the SES constraint documented in
 * `src/format.ts`).
 */
export const en = {
  // Receive dialog
  'receive.heading': 'Receive BTH',
  'receive.body':
    'Share this Botho stealth address to receive funds. A fresh one-time ' +
    'output is created on-chain for every payment, so your balance stays private.',

  // Balance dialog
  'balance.heading': 'Botho balance',
  'balance.spendable': 'Spendable',

  // Shared row labels
  'common.node': 'Node',

  // Send-confirmation dialog
  'send.heading': 'Confirm send',
  'send.amount': 'Amount',
  'send.networkFee': 'Network fee',
  'send.total': 'Total',
  'send.recipient': 'Recipient',

  // Transaction-history dialog
  'history.heading': 'Transaction history',
  'history.empty': 'No transactions yet. Payments you receive will appear here.',
  'history.spent': 'Spent',
  'history.received': 'Received',
  // Count-aware confirmation phrase (Intl-free plural: caller picks One vs Other).
  'history.confirmationsOne': '{count} confirmation',
  'history.confirmationsOther': '{count} confirmations',
  'history.line': 'Block {height} · {confirmations} · {hash}',

  // Contacts dialog
  'contacts.heading': 'Contacts',
  'contacts.empty': 'No saved contacts yet. Add one to reuse a Botho address without re-pasting it.',

  // Claim-link dialogs
  'claim.previewHeading': 'Claim link',
  'claim.confirmHeading': 'Confirm claim',
  'claim.body':
    'This claim link holds funds that will be swept into your wallet. The ' +
    'sweep fee is paid from the link.',
  'claim.empty': 'Nothing to claim — this link is empty, already claimed, or not yet confirmed.',
  'claim.claimable': 'Claimable',
  'claim.sweepFee': 'Sweep fee',
  'claim.youReceive': 'You receive',
  'claim.hint': 'Link hint: {amount} (cosmetic — the scanned amount above is authoritative)',

  // Mnemonic-backup dialog
  'mnemonic.heading': 'Botho recovery phrase',
  'mnemonic.body':
    'These 24 words are derived from your MetaMask Secret Recovery Phrase ' +
    'and are full spending authority for this Botho wallet. Write them down ' +
    'and keep them offline. Anyone who sees them can spend your funds.',
  'mnemonic.placeholder': '•••• •••• (revealed after you confirm) ••••',

  // User-facing rejection / decline errors surfaced to the dApp
  'error.rejectMnemonic': 'User declined to reveal the recovery phrase.',
  'error.rejectSend': 'User rejected the send.',
  'error.rejectClaim': 'User rejected the claim.',
} as const;
