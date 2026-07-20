/**
 * Claim-link ingress for the Botho Snap (phase 2, #1094).
 *
 * A Botho **claim link** is a *bearer instrument*: the link fragment IS an
 * ephemeral 12-word BIP39 mnemonic that owns the funded output(s). Claiming is
 * 100% client-side and reuses the normal CLSAG send/scan path — NO new node RPC,
 * no consensus change. The flow mirrors the web wallet's
 * `web-wallet/src/lib/claim-link-ops.ts` (`scanEphemeral` / `sweepEphemeral`),
 * but the web reference is bound to `RemoteNodeAdapter`; the Snap has its own
 * `NodeCall` / `makeSendRpc` (`src/node.ts`), so this module is the thin
 * Snap-local glue over the shared `SendRpc` slice.
 *
 *   1. PARSE  the link fragment -> ephemeral mnemonic (+ non-authoritative
 *             amount hint) via `parseClaimLinkFragment` from `@botho/core`.
 *   2. DERIVE the ephemeral keys (`deriveKeypairs` + `mnemonicToSeedHex`).
 *   3. SCAN   the ephemeral wallet's spendable balance (`spendableBalance`, the
 *             same `chain_getOutputs` + `chain_areKeyImagesSpent` surface as
 *             `botho_getBalance`). gross/fee/net.
 *   4. SWEEP  build + submit a CLSAG send FROM the ephemeral keys TO the user's
 *             own address (`buildSendTransaction`), fee paid from the funded
 *             output so the user nets `gross - fee`.
 *
 * BEARER-SECRET HYGIENE (cf. #474/#475): the ephemeral mnemonic lives only
 * in-memory for the duration of the RPC call. It is NEVER persisted, NEVER put
 * in a dialog or a returned result, and NEVER included in an error message.
 */

import { InvalidParamsError } from '@metamask/snaps-sdk';
import { parseClaimLinkFragment, deriveKeypairs, parseAddress } from '@botho/core';
import {
  buildSendTransaction,
  spendableBalance,
  deriveKemPublicKey,
  mnemonicToSeedHex,
  type SignerKeys,
  type SendRpc,
} from '@botho/wasm-signer';

import { wasm } from './signer';

const toHex = (b: Uint8Array): string =>
  Array.from(b)
    .map((x) => x.toString(16).padStart(2, '0'))
    .join('');

/** The parsed contents of a claim link: the ephemeral secret + optional hint. */
export interface ParsedClaimLink {
  /**
   * The ephemeral 12-word BIP39 mnemonic that owns the funded output(s). Bearer
   * secret — never surface it (dialog / result / error / log).
   */
  mnemonic: string;
  /**
   * Optional, non-authoritative cosmetic amount hint (picocredits) carried in
   * the link. NEVER trust it for the claimed value — the on-chain scan is always
   * authoritative (see `@botho/core` `claim-link.ts`).
   */
  amountHint?: bigint;
}

/** The result of scanning an ephemeral claim-link wallet. */
export interface ClaimScan {
  /** Gross spendable picocredits owned by the ephemeral wallet. */
  grossPicocredits: bigint;
  /** Sweep fee (network minimum) that will be charged. */
  feePicocredits: bigint;
  /** Net picocredits the user receives after the sweep fee (clamped at 0). */
  netPicocredits: bigint;
}

/**
 * Parse a claim link (full URL, bare fragment, or leading-`#` fragment — all
 * accepted by `parseClaimLinkFragment`) into its ephemeral secret. Throws a
 * typed {@link InvalidParamsError} on a malformed / unsupported-version / empty
 * fragment so a bad link fails BEFORE any network round-trip and BEFORE any
 * dialog. The underlying parser fails before the mnemonic is reconstructed, so
 * its message never carries the bearer secret.
 */
export function parseClaimLink(link: string): ParsedClaimLink {
  let secret;
  try {
    secret = parseClaimLinkFragment(link);
  } catch (err) {
    throw new InvalidParamsError(
      `Invalid claim link: ${err instanceof Error ? err.message : 'unparseable fragment'}.`,
    );
  }
  return { mnemonic: secret.mnemonic, amountHint: secret.amountHint };
}

/** Derive the ephemeral wallet's signing keys from its mnemonic. */
function ephemeralKeys(mnemonic: string): SignerKeys {
  const kp = deriveKeypairs(mnemonic, 0);
  return {
    spendPrivateKey: toHex(kp.spendPrivate),
    viewPrivateKey: toHex(kp.viewPrivate),
    seed: mnemonicToSeedHex(mnemonic),
  };
}

/**
 * Scan an ephemeral claim-link wallet for its spendable balance and compute
 * gross/fee/net. `gross === 0n` means the link is empty, already claimed (the
 * output's key image is spent), or the funding tx has not confirmed yet — all
 * three yield an empty spendable set, distinguished by UX, not here. Pure read:
 * reuses the same RPC surface as `botho_getBalance` (no `tx_submit`).
 */
export async function scanClaimLink(mnemonic: string, rpc: SendRpc): Promise<ClaimScan> {
  const grossPicocredits = await spendableBalance(ephemeralKeys(mnemonic), rpc);
  const feePicocredits = wasm.minFee();
  const netPicocredits =
    grossPicocredits > feePicocredits ? grossPicocredits - feePicocredits : 0n;
  return { grossPicocredits, feePicocredits, netPicocredits };
}

/**
 * Build a CLSAG sweep transaction FROM the ephemeral claim-link keys TO
 * `destinationAddress` (the user's own wallet), paying the sweep fee out of the
 * funded output so the user nets `gross - fee`. Re-scans at sweep time to get
 * the live gross/fee (mirrors the web wallet's `sweepEphemeral`). Throws — before
 * building or submitting anything — if there is nothing spendable (empty /
 * already claimed / unconfirmed) or the funded amount cannot cover the fee.
 * Returns the unsigned-then-signed tx hex ready for `tx_submit`.
 */
export async function buildSweep(
  mnemonic: string,
  destinationAddress: string,
  rpc: SendRpc,
): Promise<{ txHex: string; scan: ClaimScan }> {
  const scan = await scanClaimLink(mnemonic, rpc);
  if (scan.grossPicocredits === 0n) {
    throw new Error(
      'Nothing to claim — this link is empty, already claimed, or not yet confirmed.',
    );
  }
  if (scan.netPicocredits <= 0n) {
    throw new Error('The claimed amount does not cover the sweep fee.');
  }

  const destKeys = parseAddress(destinationAddress);
  const senderKemPublicKey = await deriveKemPublicKey(mnemonic);

  const { txHex } = await buildSendTransaction({
    keys: ephemeralKeys(mnemonic),
    recipient: {
      spend_public_key: toHex(destKeys.spendPublic),
      view_public_key: toHex(destKeys.viewPublic),
      kem_public_key: toHex(destKeys.kemPublic),
    },
    senderKemPublicKey,
    amount: scan.netPicocredits,
    fee: scan.feePicocredits,
    rpc,
  });

  return { txHex, scan };
}
