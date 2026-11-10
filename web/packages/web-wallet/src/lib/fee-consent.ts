/**
 * User consent for a node-quoted fee above the approved one (#2001).
 *
 * Since protocol 10.0.0 the node prices every send (`estimateFee` with
 * `tx_hex`), and its floor can be far above the tx-less preview the send
 * modal shows. `buildSendTransaction` never signs above the fee the user
 * approved (`maxFee`); when the node needs more it throws a
 * `FeeAboveApprovedError` before signing, and the caller must ask again.
 * These helpers are that "ask again" for the web wallet: a browser
 * `window.confirm` showing the required fee, wrapped by
 * `buildWithFeeConsent` so nothing above the confirmed fee is submitted.
 */

import { formatBTH } from '@botho/core'

import i18n from './i18n'

/**
 * Ask the user to approve `requiredFee` (above `approvedFee`). Resolves true
 * only on an explicit approval; a declined prompt resolves false.
 */
export type FeeReconfirm = (requiredFee: bigint, approvedFee: bigint) => Promise<boolean>

function bth(picocredits: bigint): string {
  return formatBTH(picocredits, { minDecimals: 2, maxDecimals: 12 })
}

/** A browser confirm for a send whose fee rose to `requiredFee`. */
export const confirmSendFee: FeeReconfirm = async (requiredFee, approvedFee) =>
  globalThis.confirm?.(
    i18n.t('wallet:feeConsent.send', { required: bth(requiredFee), approved: bth(approvedFee) }),
  ) === true

/**
 * A browser confirm for a claim-link sweep of `gross` whose fee rose to
 * `requiredFee`: the recipient nets `gross - requiredFee`.
 */
export function confirmSweepFee(gross: bigint): FeeReconfirm {
  return async (requiredFee, approvedFee) =>
    globalThis.confirm?.(
      i18n.t('wallet:feeConsent.claim', {
        required: bth(requiredFee),
        approved: bth(approvedFee),
        net: bth(gross > requiredFee ? gross - requiredFee : 0n),
      }),
    ) === true
}

/** The error a declined fee re-confirmation throws (nothing was sent). */
export function feeRejectedError(): Error {
  return new Error(i18n.t('wallet:feeConsent.rejected'))
}
