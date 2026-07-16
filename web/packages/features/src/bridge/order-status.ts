/**
 * Mint-order state-machine helpers (#1031).
 *
 * The status strings mirror the Rust `OrderStatus::Display` impl exactly
 * (`bridge/core/src/order.rs`): `awaiting_deposit → deposit_detected →
 * deposit_confirmed → mint_pending → completed`, with `expired` / `failed` as
 * off-path terminal states. Keeping the strings identical means the client can
 * consume the API verbatim once it is exposed.
 */
import type { DestinationChain, MintOrderStatus } from './types'

/**
 * The happy-path progression, in order. The tracking UI renders these as a
 * stepper and marks each as done/active/pending relative to the current status.
 * `expired`/`failed` are intentionally excluded — they are terminal off-ramps
 * rendered separately.
 */
export const MINT_PROGRESSION: readonly MintOrderStatus[] = [
  'awaiting_deposit',
  'deposit_detected',
  'deposit_confirmed',
  'mint_pending',
  'completed',
] as const

/** Terminal states — no further polling is useful once reached. */
export function isTerminalStatus(status: MintOrderStatus): boolean {
  return status === 'completed' || status === 'expired' || status === 'failed'
}

/**
 * Index of `status` within {@link MINT_PROGRESSION}, or `-1` for the off-path
 * terminal states (`expired`/`failed`). Used to decide which stepper rows are
 * complete.
 */
export function progressionIndex(status: MintOrderStatus): number {
  return MINT_PROGRESSION.indexOf(status)
}

/**
 * Canonical block-explorer transaction URL for a minted wBTH tx on the
 * destination chain. Hosts mirror `venues.ts` (Sepolia etherscan / Solana
 * devnet explorer). Testnet-only today; returns `null` for chains without a
 * known host so the caller can omit the link rather than build a broken URL.
 */
export function destTxUrl(
  chain: DestinationChain,
  tx: string,
): string | null {
  switch (chain) {
    case 'ethereum':
      return `https://sepolia.etherscan.io/tx/${tx}`
    case 'solana':
      return `https://explorer.solana.com/tx/${tx}?cluster=devnet`
    default:
      return null
  }
}
