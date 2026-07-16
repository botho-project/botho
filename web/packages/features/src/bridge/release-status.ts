/**
 * Release-order state-machine helpers (Unwrap, #1032).
 *
 * The status strings mirror the Rust `OrderStatus::Display` impl exactly
 * (`bridge/core/src/order.rs`) for the burn side: `burn_detected →
 * burn_confirmed → release_pending → released`, with `expired` / `failed` as
 * off-path terminal states. Keeping the strings identical means the client can
 * consume the release API verbatim once #1036 exposes it. This is the burn-side
 * twin of `order-status.ts` (the mint side).
 */
import type { ReleaseOrderStatus, SourceChain } from './types'

/**
 * The happy-path progression, in order. The tracking UI renders these as a
 * stepper and marks each as done/active/pending relative to the current status.
 * `expired`/`failed` are intentionally excluded — they are terminal off-ramps
 * rendered separately.
 */
export const RELEASE_PROGRESSION: readonly ReleaseOrderStatus[] = [
  'burn_detected',
  'burn_confirmed',
  'release_pending',
  'released',
] as const

/** Terminal states — no further polling is useful once reached. */
export function isTerminalReleaseStatus(status: ReleaseOrderStatus): boolean {
  return status === 'released' || status === 'expired' || status === 'failed'
}

/**
 * Index of `status` within {@link RELEASE_PROGRESSION}, or `-1` for the off-path
 * terminal states (`expired`/`failed`). Used to decide which stepper rows are
 * complete.
 */
export function releaseProgressionIndex(status: ReleaseOrderStatus): number {
  return RELEASE_PROGRESSION.indexOf(status)
}

/**
 * Canonical block-explorer transaction URL for the wBTH BURN tx on the source
 * chain. Hosts mirror `venues.ts` (Sepolia etherscan / Solana devnet explorer).
 * Testnet-only today; returns `null` for chains without a known host so the
 * caller can omit the link rather than build a broken URL.
 */
export function sourceTxUrl(chain: SourceChain, tx: string): string | null {
  switch (chain) {
    case 'ethereum':
      return `https://sepolia.etherscan.io/tx/${tx}`
    case 'solana':
      return `https://explorer.solana.com/tx/${tx}?cluster=devnet`
    default:
      return null
  }
}
