/**
 * Payment-request links — the *pull* complement to claimable send-links (#460).
 *
 * The implementation was PROMOTED into `@botho/core` (`wallet/payment-request.ts`)
 * so the Botho MetaMask Snap can import it too (#1108). That move also swapped the
 * base64url step to an SES-safe codec (`@scure/base` `base64urlnopad` + a pure-JS
 * UTF-8 codec) with byte-identical output, so existing `/pay#…` links keep
 * round-tripping. This module is now a thin re-export to keep the web wallet's
 * consumers (`RequestModal.tsx`, `pay.tsx`) importing from the same local path.
 *
 * See `@botho/core` `wallet/payment-request.ts` for the full documentation and
 * link-format details.
 */

export {
  buildPaymentRequestFragment,
  buildPaymentRequestLink,
  parsePaymentRequestFragment,
  type PaymentRequest,
} from '@botho/core'
