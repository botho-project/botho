/**
 * Bridge / trade feature types (#1030, epic #1029).
 *
 * Tier 0 is a DISCOVERY surface: it lists the venues where wrapped BTH (wBTH)
 * trades today and explains the BTH→wBTH export flow. It ships NO chain client
 * code, NO wallet-connect, and NO embedded swap — every venue is reached via an
 * external deep-link. Tiers 1 (integrated export) and Unwrap build on this
 * scaffold; the extension points are the `<ExportPanel>` slot in
 * `components/export-panel.tsx` and the `mainnet` key in `venues.ts`.
 */

/**
 * Which address set to surface. Testnet is the only populated set today; the
 * `mainnet` key exists so addresses can be swapped in later without touching
 * component code (the seeded testnet addresses are NOT valid on mainnet).
 */
export type BridgeNetwork = 'testnet' | 'mainnet'

/** The chain a venue lives on. Drives the icon/label, not any client code. */
export type VenueChain = 'ethereum' | 'solana' | 'hyperliquid'

/**
 * Venue availability:
 * - `live`        — the pool is seeded and tradeable today (has a trade link).
 * - `coming-soon` — deployed token but the market is pending (e.g. Hyperliquid
 *   HIP-1 spot, gated on #877). Rendered with a "coming soon" badge and NO
 *   trade deep-link.
 */
export type VenueStatus = 'live' | 'coming-soon'

/**
 * One tradeable (or soon-tradeable) wBTH venue.
 *
 * Addresses are display + deep-link inputs only; nothing here is fed to a chain
 * client. `tradeUrl` is the venue's swap UI (omitted while `coming-soon`);
 * `explorerUrl` is the canonical on-chain address page for the pool.
 */
export interface Venue {
  /** Stable id, e.g. `uniswap-sepolia`. */
  id: string
  /** Chain family (icon/label only). */
  chain: VenueChain
  /** Human chain + network, e.g. "Ethereum Sepolia". */
  chainLabel: string
  /** Venue/DEX name, e.g. "Uniswap v3". */
  venueName: string
  /** Traded pair, e.g. "wBTH / WETH". */
  pairLabel: string
  /** wBTH token/mint address on this chain. */
  tokenAddress: string
  /** Liquidity pool address (present for seeded pools). */
  poolAddress?: string
  /** External swap-UI deep-link. Absent while `coming-soon`. */
  tradeUrl?: string
  /** External block-explorer address page for the pool (or token). */
  explorerUrl?: string
  /** Availability. */
  status: VenueStatus
}

/** Venue sets keyed by network so mainnet addresses can be swapped in later. */
export type VenueDirectory = Record<BridgeNetwork, Venue[]>

/**
 * A caller-supplied translator (react-i18next's `t`). The bridge feature
 * components stay i18n-runtime-agnostic — the web-wallet page owns the
 * `bridge` namespace and passes `t` in — so `@botho/features` keeps no
 * dependency on react-i18next (matching the rest of the module).
 */
export type Translate = (key: string, options?: Record<string, unknown>) => string
