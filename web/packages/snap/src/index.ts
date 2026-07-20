/**
 * Botho MetaMask Snap — Phase-1 MVP (issue #815).
 *
 * Manage a privacy-by-default Botho wallet from inside MetaMask. The Snap:
 *   1. Derives the Botho wallet from the MetaMask SRP (SIP-6 `snap_getEntropy`)
 *      through the node-identical SLIP-10 RootIdentity pipeline (see
 *      `derivation.ts`). Keys never leave the sandbox.
 *   2. Runs `bth-wasm-signer` (inlined wasm) for all Botho crypto — scan / build
 *      / CLSAG sign — inside the Snaps SES executor (`signer.ts`), proven by the
 *      Phase-0 spike (PR #1055).
 *   3. Talks to a USER-SELECTED Botho node ingress for UTXO/decoy fetch, spent
 *      checks and tx submit, carrying over the web wallet's node-trust /
 *      wrong-network guard (`node.ts`, cf. #811).
 *   4. Renders custom-UI dialogs for receive / balance / send confirmation and a
 *      recovery-phrase backup (`ui.ts`).
 *
 * MVP RPC methods (all params/results are JSON-safe; amounts are string-encoded
 * u64 picocredits):
 *   - botho_getAddress  — the wallet's stealth receive address (silent read)
 *   - botho_getBalance  — spendable balance via scan (silent read)
 *   - botho_getHistory  — receive history projected from persisted scan state (silent read, #1092)
 *   - botho_listContacts — saved address book (silent read, #1093)
 *   - botho_addContact  — save a validated (label, address) contact (#1093)
 *   - botho_removeContact — drop a saved contact by id (#1093)
 *   - botho_send        — build + sign + submit, behind a confirmation dialog
 *   - botho_previewClaimLink — scan a claim link (parse → derive → scan → alert), pure read (#1094)
 *   - botho_claimLink   — sweep a claim link into this wallet, behind a confirmation (#1094)
 *   - botho_showReceive — receive dialog (stealth address, copyable)
 *   - botho_showBalance — balance dialog
 *   - botho_showHistory — transaction-history dialog (#1092)
 *   - botho_showContacts — contacts dialog (#1093)
 *   - botho_showMnemonic — reveal the derived recovery phrase (confirmed backup)
 *
 * Live-testnet send validation is deferred to a follow-up (betanet is frozen at
 * height 202, #1051); tests exercise send against a MOCKED node RPC. See
 * README.md.
 */

import {
  DialogType,
  InvalidParamsError,
  MethodNotFoundError,
  UserRejectedRequestError,
  type Json,
  type OnRpcRequestHandler,
} from '@metamask/snaps-sdk';
import {
  buildSendTransaction,
  loadSigner,
  type RecipientAddress,
} from '@botho/wasm-signer';
import { parseAddress } from '@botho/core';

import { ensureSigner, wasm } from './signer';
import { deriveWallet, revealMnemonic, DERIVATION_DESCRIPTION } from './derivation';
import { connectAndGuard, getOutputsWithMeta, makeSendRpc, EXPECTED_NETWORK_ID } from './node';
import {
  incrementalScan,
  incrementalScanBalance,
  deriveHistory,
  spentTargetKeys,
  type HistoryEntry,
} from './state';
import {
  addContact,
  removeContact,
  readContacts,
  writeContacts,
} from './contacts';
import {
  receiveContent,
  balanceContent,
  historyContent,
  contactsContent,
  sendConfirmContent,
  claimPreviewContent,
  claimConfirmContent,
  mnemonicBackupContent,
} from './ui';
import { parseClaimLink, scanClaimLink, buildSweep } from './claim';
import { resolveLocale, t } from './i18n';

declare const snap: {
  request(args: { method: string; params?: unknown }): Promise<unknown>;
};

const toHex = (b: Uint8Array): string =>
  Array.from(b)
    .map((x) => x.toString(16).padStart(2, '0'))
    .join('');

/** Decode a `botho://2/` recipient into the signer's `RecipientAddress`. */
function decodeRecipient(address: string): RecipientAddress {
  const parsed = parseAddress(address);
  return {
    spend_public_key: toHex(parsed.spendPublic),
    view_public_key: toHex(parsed.viewPublic),
    kem_public_key: toHex(parsed.kemPublic),
  };
}

/** Require a string RPC param, throwing a typed InvalidParamsError otherwise. */
function requireString(params: Record<string, unknown> | undefined, key: string): string {
  const v = params?.[key];
  if (typeof v !== 'string' || v.length === 0) {
    throw new InvalidParamsError(`Missing or invalid "${key}" (expected a non-empty string).`);
  }
  return v;
}

/** Parse a string-encoded positive u64 picocredit amount. */
function requireAmount(params: Record<string, unknown> | undefined, key: string): bigint {
  const raw = requireString(params, key);
  let value: bigint;
  try {
    value = BigInt(raw);
  } catch {
    throw new InvalidParamsError(`"${key}" must be an integer string of picocredits.`);
  }
  if (value <= 0n) {
    throw new InvalidParamsError(`"${key}" must be greater than 0.`);
  }
  return value;
}

/** Optional string-encoded fee; defaults to the network minimum fee. */
function optionalFee(params: Record<string, unknown> | undefined): bigint {
  const raw = params?.feePicocredits;
  if (raw === undefined || raw === null || raw === '') return wasm.minFee();
  if (typeof raw !== 'string') {
    throw new InvalidParamsError('"feePicocredits" must be an integer string.');
  }
  let fee: bigint;
  try {
    fee = BigInt(raw);
  } catch {
    throw new InvalidParamsError('"feePicocredits" must be an integer string.');
  }
  const min = wasm.minFee();
  if (fee < min) {
    throw new InvalidParamsError(`Fee ${fee} is below the network minimum ${min}.`);
  }
  return fee;
}

/**
 * Compute the wallet's spendable balance via the persisted-state, windowed
 * incremental scan (#1091). Resumes from the last-scanned checkpoint in
 * `snap_manageState` instead of re-scanning the whole chain from genesis; spent
 * status is recomputed live on every read. Shared by `botho_getBalance` and
 * `botho_showBalance`.
 */
async function scanBalance(rpcUrl: string): Promise<bigint> {
  const { call, status } = await connectAndGuard(rpcUrl);
  const wallet = await deriveWallet();
  const signer = await loadSigner();
  return incrementalScanBalance({
    signer,
    keys: wallet.keys,
    // Bind persisted scan state to the node's reported network so a later read
    // against a different network is invalidated (loopback dev nodes may not
    // report one — fall back to the expected id to keep the binding stable).
    network: status.network ?? EXPECTED_NETWORK_ID,
    tip: status.chainHeight,
    fetchWindow: getOutputsWithMeta(call),
    sendRpc: makeSendRpc(call),
  });
}

/**
 * Derive the wallet's transaction history via the SAME persisted-state,
 * incremental scan as {@link scanBalance} (#1091) — history is a pure projection
 * over the already-persisted owned set (no rescan, no persisted history record).
 * The receive facts come off persisted state; the spent annotation and the tip
 * (for confirmations/finality depth) are recomputed live, exactly as the balance
 * is. Shared by `botho_getHistory` and `botho_showHistory`.
 */
async function scanHistory(rpcUrl: string): Promise<HistoryEntry[]> {
  const { call, status } = await connectAndGuard(rpcUrl);
  const wallet = await deriveWallet();
  const signer = await loadSigner();
  const { ownedOutputs, spendable, tip } = await incrementalScan({
    signer,
    keys: wallet.keys,
    network: status.network ?? EXPECTED_NETWORK_ID,
    tip: status.chainHeight,
    fetchWindow: getOutputsWithMeta(call),
    sendRpc: makeSendRpc(call),
  });
  return deriveHistory(ownedOutputs, spentTargetKeys(ownedOutputs, spendable), tip);
}

/** Ask the user to approve/reject a confirmation dialog. */
async function confirm(content: unknown): Promise<boolean> {
  return (await snap.request({
    method: 'snap_dialog',
    params: { type: DialogType.Confirmation, content },
  })) as boolean;
}

/** Show an informational alert dialog (single acknowledge button). */
async function alert(content: unknown): Promise<void> {
  await snap.request({
    method: 'snap_dialog',
    params: { type: DialogType.Alert, content },
  });
}

export const onRpcRequest: OnRpcRequestHandler = async ({ request }) => {
  ensureSigner();
  const params = (request.params ?? undefined) as Record<string, unknown> | undefined;

  // Resolve the dialog locale once per invocation from the MetaMask user's
  // preference (`snap_getPreferences.locale`, no extra manifest permission),
  // narrowed to the Snap's supported set and defaulting to `en` (#1095). Threaded
  // into every content builder and the user-facing rejection messages below.
  // Silent-read methods simply don't use it.
  const locale = await resolveLocale();

  switch (request.method) {
    /* ------------------------------------------------------------------ */
    /* Silent reads                                                       */
    /* ------------------------------------------------------------------ */
    case 'botho_getAddress': {
      const wallet = await deriveWallet();
      return { address: wallet.address, derivation: DERIVATION_DESCRIPTION } as Json;
    }

    case 'botho_getBalance': {
      const rpcUrl = requireString(params, 'rpcUrl');
      const balance = await scanBalance(rpcUrl);
      return { spendablePicocredits: balance.toString() } as Json;
    }

    case 'botho_getHistory': {
      const rpcUrl = requireString(params, 'rpcUrl');
      const entries = await scanHistory(rpcUrl);
      return { entries } as unknown as Json;
    }

    case 'botho_listContacts': {
      // Silent read of the saved address book.
      const contacts = await readContacts();
      return { contacts } as unknown as Json;
    }

    /* ------------------------------------------------------------------ */
    /* Contacts: validated read-modify-write over the `contacts` namespace */
    /* (spreads the persisted blob so `scan` is never clobbered — #1093).  */
    /* ------------------------------------------------------------------ */
    case 'botho_addContact': {
      const label = requireString(params, 'label');
      const address = requireString(params, 'address');
      // `addContact` validates the address via `isValidAddress` and rejects an
      // invalid address / empty / over-length label with InvalidParamsError.
      const book = addContact(await readContacts(), { label, address });
      await writeContacts(book);
      const contact = book[book.length - 1];
      return { contact, contacts: book } as unknown as Json;
    }

    case 'botho_removeContact': {
      const id = requireString(params, 'id');
      const book = removeContact(await readContacts(), id);
      await writeContacts(book);
      return { contacts: book } as unknown as Json;
    }

    /* ------------------------------------------------------------------ */
    /* Dialog-driven flows                                                */
    /* ------------------------------------------------------------------ */
    case 'botho_showReceive': {
      const wallet = await deriveWallet();
      await alert(receiveContent(wallet.address, locale));
      return { address: wallet.address } as Json;
    }

    case 'botho_showBalance': {
      const rpcUrl = requireString(params, 'rpcUrl');
      const balance = await scanBalance(rpcUrl);
      await alert(balanceContent(balance, rpcUrl, locale));
      return { spendablePicocredits: balance.toString() } as Json;
    }

    case 'botho_showHistory': {
      const rpcUrl = requireString(params, 'rpcUrl');
      const entries = await scanHistory(rpcUrl);
      await alert(historyContent(entries, rpcUrl, locale));
      return { entries, count: entries.length } as unknown as Json;
    }

    case 'botho_showContacts': {
      const contacts = await readContacts();
      await alert(contactsContent(contacts, locale));
      return { contacts, count: contacts.length } as unknown as Json;
    }

    case 'botho_showMnemonic': {
      // Full spending authority — always behind an explicit user confirmation.
      const proceed = await confirm(
        mnemonicBackupContent(t('mnemonic.placeholder', locale), locale),
      );
      if (!proceed) {
        throw new UserRejectedRequestError(t('error.rejectMnemonic', locale));
      }
      const mnemonic = await revealMnemonic();
      await alert(mnemonicBackupContent(mnemonic, locale));
      return { revealed: true } as Json;
    }

    /* ------------------------------------------------------------------ */
    /* Send: confirm -> build + sign -> submit                            */
    /* ------------------------------------------------------------------ */
    case 'botho_send': {
      const rpcUrl = requireString(params, 'rpcUrl');
      const recipientAddress = requireString(params, 'recipientAddress');
      const amount = requireAmount(params, 'amountPicocredits');
      const fee = optionalFee(params);

      // Validate the recipient address up front so a typo fails before the
      // network round-trip and before we show a confirmation for a bad send.
      const recipient = decodeRecipient(recipientAddress);

      // Node-trust / wrong-network guard before anything else touches funds.
      const { call } = await connectAndGuard(rpcUrl);

      const approved = await confirm(
        sendConfirmContent(
          {
            recipientAddress,
            amountPicocredits: amount,
            feePicocredits: fee,
            rpcUrl,
          },
          locale,
        ),
      );
      if (!approved) {
        throw new UserRejectedRequestError(t('error.rejectSend', locale));
      }

      const wallet = await deriveWallet();
      const { txHex } = await buildSendTransaction({
        keys: wallet.keys,
        recipient,
        senderKemPublicKey: wallet.kemPublicKey,
        amount,
        fee,
        rpc: makeSendRpc(call),
      });

      const { txHash } = await call<{ txHash: string }>('tx_submit', { tx_hex: txHex });
      return { txHash, txBytes: txHex.length / 2 } as Json;
    }

    /* ------------------------------------------------------------------ */
    /* Claim-link ingress (#1094): parse -> derive -> scan -> [sweep]      */
    /* A claim link is a bearer instrument (an ephemeral mnemonic); the    */
    /* bearer secret lives only in-memory for this call — never persisted, */
    /* never surfaced in a dialog / result / error.                        */
    /* ------------------------------------------------------------------ */
    case 'botho_previewClaimLink': {
      const rpcUrl = requireString(params, 'rpcUrl');
      const link = requireString(params, 'link');

      // Parse BEFORE any network round-trip so a malformed link fails without
      // a call or a dialog (typed InvalidParamsError).
      const { mnemonic, amountHint } = parseClaimLink(link);

      // Node-trust / wrong-network guard before we scan.
      const { call } = await connectAndGuard(rpcUrl);
      const scan = await scanClaimLink(mnemonic, makeSendRpc(call));

      await alert(claimPreviewContent({ ...scan, amountHint, rpcUrl }, locale));
      return {
        grossPicocredits: scan.grossPicocredits.toString(),
        feePicocredits: scan.feePicocredits.toString(),
        netPicocredits: scan.netPicocredits.toString(),
      } as Json;
    }

    case 'botho_claimLink': {
      const rpcUrl = requireString(params, 'rpcUrl');
      const link = requireString(params, 'link');

      const { mnemonic, amountHint } = parseClaimLink(link);

      // Wrong-network guard fires first, before anything touches funds.
      const { call } = await connectAndGuard(rpcUrl);
      const rpc = makeSendRpc(call);

      // Scan so the confirmation shows the authoritative claimable/net amount.
      const scan = await scanClaimLink(mnemonic, rpc);

      const approved = await confirm(claimConfirmContent({ ...scan, amountHint, rpcUrl }, locale));
      if (!approved) {
        throw new UserRejectedRequestError(t('error.rejectClaim', locale));
      }

      // Sweep into the user's OWN derived address. `buildSweep` re-scans and
      // surfaces "nothing to claim" (never submitting) when the link is empty.
      const wallet = await deriveWallet();
      const { txHex, scan: sweptScan } = await buildSweep(mnemonic, wallet.address, rpc);

      const { txHash } = await call<{ txHash: string }>('tx_submit', { tx_hex: txHex });
      return { txHash, netPicocredits: sweptScan.netPicocredits.toString() } as Json;
    }

    default:
      throw new MethodNotFoundError({ method: request.method });
  }
};
