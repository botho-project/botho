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
 *   - botho_send        — build + sign + submit, behind a confirmation dialog
 *   - botho_showReceive — receive dialog (stealth address, copyable)
 *   - botho_showBalance — balance dialog
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
  spendableBalance,
  type RecipientAddress,
} from '@botho/wasm-signer';
import { parseAddress } from '@botho/core';

import { ensureSigner, wasm } from './signer';
import { deriveWallet, revealMnemonic, DERIVATION_DESCRIPTION } from './derivation';
import { connectAndGuard, makeSendRpc } from './node';
import {
  receiveContent,
  balanceContent,
  sendConfirmContent,
  mnemonicBackupContent,
} from './ui';

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
      const { call } = await connectAndGuard(rpcUrl);
      const wallet = await deriveWallet();
      const balance = await spendableBalance(wallet.keys, makeSendRpc(call));
      return { spendablePicocredits: balance.toString() } as Json;
    }

    /* ------------------------------------------------------------------ */
    /* Dialog-driven flows                                                */
    /* ------------------------------------------------------------------ */
    case 'botho_showReceive': {
      const wallet = await deriveWallet();
      await alert(receiveContent(wallet.address));
      return { address: wallet.address } as Json;
    }

    case 'botho_showBalance': {
      const rpcUrl = requireString(params, 'rpcUrl');
      const { call } = await connectAndGuard(rpcUrl);
      const wallet = await deriveWallet();
      const balance = await spendableBalance(wallet.keys, makeSendRpc(call));
      await alert(balanceContent(balance, rpcUrl));
      return { spendablePicocredits: balance.toString() } as Json;
    }

    case 'botho_showMnemonic': {
      // Full spending authority — always behind an explicit user confirmation.
      const proceed = await confirm(
        mnemonicBackupContent('•••• •••• (revealed after you confirm) ••••'),
      );
      if (!proceed) {
        throw new UserRejectedRequestError('User declined to reveal the recovery phrase.');
      }
      const mnemonic = await revealMnemonic();
      await alert(mnemonicBackupContent(mnemonic));
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
        sendConfirmContent({
          recipientAddress,
          amountPicocredits: amount,
          feePicocredits: fee,
          rpcUrl,
        }),
      );
      if (!approved) {
        throw new UserRejectedRequestError('User rejected the send.');
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

    default:
      throw new MethodNotFoundError({ method: request.method });
  }
};
