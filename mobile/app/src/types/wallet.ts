/**
 * Mobile Wallet Types
 *
 * These types mirror the Rust FFI interface exported by the `botho-mobile`
 * crate (mobile/rust-bridge/src/lib.rs, proc-macro UniFFI). Field names use
 * the camelCase form UniFFI emits in the generated Swift/Kotlin bindings, so
 * the TS wrapper in src/native/walletModule.ts can pass values straight
 * through the native module.
 */

/** Public wallet address (safe to display) */
export interface WalletAddress {
  /** View public key (hex encoded) */
  viewPublicKey: string;
  /** Spend public key (hex encoded) */
  spendPublicKey: string;
  /** Display format (tbotho://1/<base58(view||spend)>) */
  display: string;
}

/** Balance information */
export interface WalletBalance {
  /** Balance in picocredits (smallest unit) */
  picocredits: bigint;
  /** Formatted balance (e.g., "1.234567 BTH") */
  formatted: string;
  /** Number of UTXOs */
  utxoCount: number;
  /** Last sync block height */
  syncHeight: number;
}

/** Transaction history entry */
export interface TransactionEntry {
  /** Transaction hash */
  txHash: string;
  /** Amount in picocredits (negative for sends) */
  amount: bigint;
  /** Block height */
  blockHeight: number;
  /** Timestamp (Unix seconds; 0 when unknown for chain-recovered outputs) */
  timestamp: number;
  /** Direction: "send" or "receive" */
  direction: "send" | "receive";
  /** Counterparty address (if known) */
  counterparty?: string;
}

/** Session status information */
export interface SessionStatus {
  /** Whether wallet is currently unlocked */
  isUnlocked: boolean;
  /** Public address if unlocked */
  address?: WalletAddress;
  /** Seconds until session expires */
  expiresInSeconds?: number;
}

/** Result of a faucet request (mirrors Rust `FaucetResult`) */
export interface FaucetResult {
  /** Whether the faucet dispensed coins */
  success: boolean;
  /** Transaction hash of the payout (empty if unsuccessful) */
  txHash: string;
  /** Amount dispensed in picocredits */
  amount: bigint;
  /** Human-readable amount (e.g. "10.000000 BTH") */
  amountFormatted: string;
  /** Optional message from the faucet (error / rate-limit info) */
  message: string;
}

/** Health/status of a node, for the node picker (mirrors `NodeStatusInfo`) */
export interface NodeStatusInfo {
  /** Node software version */
  version: string;
  /** Network name (e.g. "botho-testnet") */
  network: string;
  /** Current chain height */
  chainHeight: number;
  /** Sync status string (e.g. "synced") */
  syncStatus: string;
  /** Number of connected peers */
  peerCount: number;
}

/**
 * Verifiable node identity returned by the `node_getIdentity` RPC (#500, epic
 * #441 Phase P1). A thin client fetches this for a candidate node *before*
 * trusting it for RPC ingress, so the user can confirm which node they are
 * talking to and the app can reject network / protocol mismatches.
 *
 * Field names mirror the JSON keys emitted by the node's RPC handler
 * (`botho/src/rpc/mod.rs::handle_node_identity`).
 */
export interface NodeIdentity {
  /** libp2p peer ID derived from the node's persistent keypair (stable). */
  peerId: string;
  /** SCP node-id signing public key (hex), derived from the peer ID. */
  nodeId: string;
  /** Network the node belongs to (e.g. "botho-testnet" / "botho-mainnet"). */
  network: string;
  /** Wire-protocol version the node speaks (e.g. "2.0.0"). */
  protocolVersion: string;
  /** Minimum protocol version the node will accept from peers. */
  minProtocolVersion: string;
  /** Node software version (e.g. "0.2.0"). */
  nodeVersion: string;
  /** Build provenance (git commit short hash, or "unknown"). */
  gitCommit: string;
  /** DNS-seed discovery namespace for the node's network. */
  dnsSeedDomain: string;
  /** Current chain tip height the node reports. */
  chainHeight: number;
  /** Current chain tip hash (hex). */
  tipHash: string;
}

/** Wallet error types (matching Rust MobileWalletError) */
export type WalletErrorType =
  | "WalletLocked"
  | "InvalidMnemonic"
  | "InvalidPassword"
  | "SessionExpired"
  | "NetworkError"
  | "InvalidAddress"
  | "InsufficientFunds"
  | "InternalError";

/** Wallet error with type and message */
export interface WalletError {
  type: WalletErrorType;
  message: string;
}

/**
 * Native wallet module interface.
 *
 * This is the full surface exported by the merged rust-bridge (#447). Method
 * names + argument names match the UniFFI-generated Swift/Kotlin bindings.
 */
export interface NativeWalletModule {
  /** Set the node URL for RPC connections */
  setNodeUrl(url: string): Promise<void>;

  /** Generate a new wallet, returns mnemonic phrase (auto-unlocks) */
  generateWallet(): Promise<string>;

  /** Unlock wallet with mnemonic phrase */
  unlockWithMnemonic(mnemonic: string): Promise<WalletAddress>;

  /** Lock wallet and zeroize keys */
  lock(): Promise<boolean>;

  /** Get current session status */
  getSessionStatus(): Promise<SessionStatus>;

  /** Get wallet balance (requires unlock) */
  getBalance(): Promise<WalletBalance>;

  /** Get transaction history (requires unlock) */
  getTransactionHistory(
    limit: number,
    offset: number
  ): Promise<TransactionEntry[]>;

  /** Get wallet public address (requires unlock) */
  getAddress(): Promise<WalletAddress>;

  /** Send a transfer (requires unlock); returns the tx hash */
  sendTransaction(
    toAddress: string,
    amountPicocredits: bigint
  ): Promise<string>;

  /** Request testnet coins from the configured faucet node (requires unlock) */
  requestFaucet(): Promise<FaucetResult>;

  /** Get the configured node's status (height/sync/peers) */
  getNodeStatus(): Promise<NodeStatusInfo>;
}
