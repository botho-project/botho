/**
 * Payment-request (pull payment) ingress for the Botho Snap (phase 2, #1108).
 *
 * A Botho **payment-request link** (`/pay#…`) is the *pull* complement to the
 * claim-link *push* flow shipped in #1094 (`claim.ts`). Where a claim link is a
 * BEARER instrument (the fragment IS an ephemeral spending secret), a payment
 * request carries only the requester's PUBLIC address plus an optional amount and
 * memo. It asks the holder to PAY the requester; the payer keeps custody until
 * they approve a normal send.
 *
 * SECURITY — push vs pull (why this file is NOT a copy of `claim.ts`): a claim
 * link's mnemonic is a bearer secret, so `claim.ts` is meticulous never to
 * surface it. A request link is the opposite: `to`/`amount`/`memo` are the exact
 * fields the payer must SEE to verify who they are paying. So this module freely
 * echoes them in dialogs and results — there is deliberately NO redaction.
 *
 * Preview is a PURE parse (no node RPC), so it is trivially "not gated on
 * betanet" (#1051). The actual send reuses the existing param-driven `botho_send`
 * (the caller threads the previewed `{ to, amountPicocredits?, memo? }` straight
 * into it); there is no new payer RPC, and the live send inherits the existing
 * #1051 gap unchanged.
 */

import { InvalidParamsError } from '@metamask/snaps-sdk';
import { parsePaymentRequestFragment, isValidAddress } from '@botho/core';

/**
 * A parsed payment request: the requester's PUBLIC address plus the optional
 * requested amount (picocredits) and memo. Nothing here is secret.
 */
export interface ParsedPaymentRequest {
  /** The requester's PUBLIC Botho address (format-validated). */
  to: string;
  /**
   * The requested amount in picocredits, if the link carried one. Absent means
   * the payer chooses the amount (the caller supplies it to `botho_send`).
   */
  amountPicocredits?: bigint;
  /** Optional human-readable note attached to the request. */
  memo?: string;
}

/**
 * Parse a payment-request link (full URL, bare fragment, or leading-`#` fragment
 * — all accepted by `parsePaymentRequestFragment`) and VALIDATE the recipient
 * address format at the boundary.
 *
 * `parsePaymentRequestFragment` only checks that `to` is a non-empty string; a
 * pull payment acts on that address, so we additionally require it to be a valid
 * Botho address (`isValidAddress`, the same gate `botho_send`'s `decodeRecipient`
 * applies) BEFORE any dialog or send. Both a malformed fragment and a malformed
 * address fail with a typed {@link InvalidParamsError}, mirroring how `claim.ts`
 * wraps `parseClaimLinkFragment`.
 */
export function parsePaymentRequest(link: string): ParsedPaymentRequest {
  let req;
  try {
    req = parsePaymentRequestFragment(link);
  } catch (err) {
    throw new InvalidParamsError(
      `Invalid payment request: ${err instanceof Error ? err.message : 'unparseable fragment'}`,
    );
  }
  if (!isValidAddress(req.to)) {
    throw new InvalidParamsError(
      'Invalid payment request: the recipient is not a valid Botho address.',
    );
  }
  return { to: req.to, amountPicocredits: req.amount, memo: req.memo };
}
