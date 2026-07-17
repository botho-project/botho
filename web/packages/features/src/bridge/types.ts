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

// ─── Tier 1: integrated BTH → wBTH export (#1031) ───────────────────────────

/**
 * Destination chains the wallet can EXPORT to (open a mint order for). This is
 * a strict subset of {@link VenueChain}: Hyperliquid is discovery-only
 * (`coming-soon`) until its HIP-1 spot market lands (#877), so it is not an
 * export destination yet.
 */
export type DestinationChain = 'ethereum' | 'solana'

/**
 * Mint-order status. Strings are byte-identical to the Rust
 * `OrderStatus::Display` impl (`bridge/core/src/order.rs`) so the API can be
 * consumed verbatim. The wallet only ever opens MINT orders, so the burn-side
 * states (`burn_*`, `release_*`, `released`) are intentionally omitted.
 */
export type MintOrderStatus =
  | 'awaiting_deposit'
  | 'deposit_detected'
  | 'deposit_confirmed'
  | 'mint_pending'
  | 'completed'
  | 'expired'
  | 'failed'

/** Request body for opening a mint order. */
export interface CreateMintOrderRequest {
  /** Chain wBTH is minted on. */
  destChain: DestinationChain
  /** The user's OWN address on `destChain` — where wBTH lands. */
  destAddress: string
  /**
   * Gross BTH to lock, in picocredits, as a decimal STRING to preserve the full
   * u64 range across JSON without precision loss (mirrors the reserve-proof
   * number contract in `network/`).
   */
  amount: string
}

/**
 * A mint order as returned by the bridge order API. Amounts are picocredit
 * strings (u64-safe). Field names are camelCase to match the existing
 * `serde(rename_all = "camelCase")` convention on the bridge's HTTP responses.
 */
export interface MintOrder {
  /** Order UUID. */
  id: string
  /** Current state-machine status. */
  status: MintOrderStatus
  /** Chain wBTH is minted on. */
  destChain: DestinationChain
  /** The user's address on `destChain`. */
  destAddress: string
  /** Gross BTH to lock, picocredits (string). */
  amount: string
  /** Bridge fee, picocredits (string). */
  fee: string
  /** BTH reserve deposit address the wallet must send the deposit to. */
  depositAddress: string
  /**
   * Order memo the deposit must carry so the bridge watcher associates the
   * deposit with this order (the first 16 bytes are the order UUID; see
   * `BridgeOrder::generate_memo`). Hex-encoded.
   */
  memo: string
  /** Destination-chain mint tx hash, once wBTH is minted. */
  destTx?: string | null
  /** Unix seconds after which an unpaid order expires. */
  expiresAt?: number | null
  /** Failure reason when `status === 'failed'`. */
  failureReason?: string | null
}

/**
 * Everything the integrated {@link ExportPanel} needs from the host app,
 * injected by the `/trade` page so `@botho/features` keeps NO dependency on the
 * wallet context or the bridge client wiring (mirroring how the page injects
 * venue/reserve data into {@link BridgeView}). The wallet does the BTH side
 * ONLY — there is deliberately no counterparty-chain signing hook here.
 */
export interface ExportController {
  /**
   * The bridge order API client, or `null` when no endpoint is configured
   * (`VITE_BRIDGE_API_BASE` unset). `null` makes the panel render an explicit
   * "endpoint not wired yet" state instead of a broken form.
   */
  client: BridgeClientLike | null
  /** Active bridge network — drives testnet labeling. */
  network: BridgeNetwork
  /** Snapshot of the wallet state relevant to exporting. */
  wallet: ExportWalletState
  /**
   * Build + sign + submit the BTH deposit via the wallet's existing
   * wasm-signer send path (`@botho/wasm-signer`). Resolves to the deposit tx
   * hash. This is the ONLY signing the wallet does; wBTH is minted by the
   * bridge to the user's own counterparty-chain wallet.
   */
  submitDeposit(args: {
    depositAddress: string
    amount: bigint
    memo: string
  }): Promise<string>
  /** Navigate the user to the wallet (to create/open or unlock it). */
  requestWallet?: () => void
}

/** Wallet fields the export panel reads (no secrets, no signing keys). */
export interface ExportWalletState {
  /** Whether a wallet exists in this browser. */
  hasWallet: boolean
  /** Whether the wallet is locked (deposit signing needs it unlocked). */
  isLocked: boolean
  /** Spendable balance in picocredits, or `null` when unknown/locked. */
  spendableBalance: bigint | null
}

/**
 * Structural subset of `BridgeClient` (from `bridge-client.ts`). Declared here
 * so `types.ts` stays dependency-free; the concrete client satisfies it.
 */
export interface BridgeClientLike {
  createMintOrder(req: CreateMintOrderRequest): Promise<MintOrder>
  getOrderStatus(id: string): Promise<MintOrder>
}

// ─── Unwrap: wBTH → BTH return leg (#1032) ──────────────────────────────────

/**
 * Source chains the wallet can UNWRAP from (the chain the user burns wBTH on).
 * Mirrors {@link DestinationChain}: Hyperliquid is discovery-only
 * (`coming-soon`) until its HIP-1 spot market lands (#877), so it is not an
 * unwrap source yet.
 */
export type SourceChain = 'ethereum' | 'solana'

/**
 * Release-order status. The burn-side strings are byte-identical to the Rust
 * `OrderStatus::Display` impl (`bridge/core/src/order.rs`) —
 * `burn_detected → burn_confirmed → release_pending → released`, with
 * `expired` / `failed` as off-path terminal states.
 *
 * `awaiting_burn` is a public-API-only pre-state (#1036): a release order is a
 * NON-CUSTODIAL tracking intent (the burn happens in the user's own
 * counterparty wallet and is self-describing), so between registration and the
 * watcher detecting the on-chain burn there is not yet any `BridgeOrder` to
 * report — the API returns `awaiting_burn` until it can correlate one by
 * `(bthAddress, amount)`. It has no Rust `OrderStatus` counterpart by design.
 */
export type ReleaseOrderStatus =
  | 'awaiting_burn'
  | 'burn_detected'
  | 'burn_confirmed'
  | 'release_pending'
  | 'released'
  | 'expired'
  | 'failed'

/**
 * Request body for opening a release order. The wallet registers its INTENT to
 * unwrap — the source chain, the Botho address the released BTH should land at
 * (the wallet's own receive address, ADR 0004), and the wBTH amount it will
 * burn. The user then executes the `bridgeBurn(amount, bthAddress)` call in
 * THEIR OWN counterparty wallet; the bridge correlates that burn to this order
 * by the `bthAddress` + `amount` and releases the BTH. The Botho wallet never
 * signs on the counterparty chain.
 */
export interface CreateReleaseOrderRequest {
  /** Chain the user burns wBTH on. */
  sourceChain: SourceChain
  /** The Botho address released BTH is sent to (the wallet's receive address). */
  bthAddress: string
  /**
   * Gross wBTH to burn, in picocredits, as a decimal STRING to preserve the
   * full u64 range across JSON without precision loss (mirrors
   * {@link CreateMintOrderRequest}).
   */
  amount: string
}

/**
 * A release order as returned by the bridge order API (#1036 fast-follow).
 * Amounts are picocredit strings (u64-safe); field names are camelCase to match
 * the bridge's `serde(rename_all = "camelCase")` HTTP responses.
 */
export interface ReleaseOrder {
  /** Order UUID. */
  id: string
  /** Current release state-machine status. */
  status: ReleaseOrderStatus
  /** Chain the user burns wBTH on. */
  sourceChain: SourceChain
  /** The Botho address released BTH is sent to. */
  bthAddress: string
  /** Gross wBTH to burn, picocredits (string). */
  amount: string
  /** Bridge fee, picocredits (string). */
  fee: string
  /** wBTH token/mint address the user must burn on `sourceChain`. */
  tokenAddress: string
  /** Source-chain burn tx hash, once the burn is detected. */
  sourceTx?: string | null
  /** Botho release tx hash, once the BTH is released. */
  destTx?: string | null
  /** Unix seconds after which an unburned order expires. */
  expiresAt?: number | null
  /** Failure reason when `status === 'failed'`. */
  failureReason?: string | null
}

/**
 * Structural subset of `BridgeClient` for the release side. Declared here so
 * `types.ts` stays dependency-free; the concrete client satisfies it. Kept
 * separate from {@link BridgeClientLike} so the mint-only `ExportController`
 * mocks stay valid (they need not implement release methods).
 */
export interface ReleaseClientLike {
  createReleaseOrder(req: CreateReleaseOrderRequest): Promise<ReleaseOrder>
  getReleaseOrderStatus(id: string): Promise<ReleaseOrder>
}

/**
 * Everything the {@link UnwrapPanel} needs from the host app, injected by the
 * `/trade` page so `@botho/features` keeps NO dependency on the wallet context
 * or the bridge client wiring (mirroring {@link ExportController}).
 *
 * There is deliberately NO signing hook here: the wallet does not sign on the
 * counterparty chain, and the released BTH simply arrives at `releaseAddress`
 * (which the wallet already scans for owned outputs). The wallet's whole job is
 * destination + guidance + tracking + receipt.
 */
export interface UnwrapController {
  /**
   * The release-order API client, or `null` when no endpoint is configured
   * (`VITE_BRIDGE_API_BASE` unset). `null` keeps the destination + burn
   * guidance visible (they need no backend — the bridge watches the chain and
   * releases regardless) but disables live order tracking, mirroring how the
   * export panel degrades when unconfigured.
   */
  client: ReleaseClientLike | null
  /** Active bridge network — drives testnet labeling. */
  network: BridgeNetwork
  /** Snapshot of the wallet state relevant to unwrapping. */
  wallet: UnwrapWalletState
  /** Navigate the user to the wallet (to create/open or unlock it). */
  requestWallet?: () => void
}

/** Wallet fields the unwrap panel reads (no secrets, no signing keys). */
export interface UnwrapWalletState {
  /** Whether a wallet exists in this browser. */
  hasWallet: boolean
  /** Whether the wallet is locked. */
  isLocked: boolean
  /**
   * The wallet's Botho receive address for the released BTH — a stealth address
   * (ADR 0004), reused from the wallet context (the same value the Receive
   * modal shows). `null` when no wallet exists or it is locked and the address
   * is unavailable.
   */
  releaseAddress: string | null
}
