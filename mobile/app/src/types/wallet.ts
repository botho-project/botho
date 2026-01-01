/**
 * Mobile Wallet Types
 *
 * These types mirror the Rust FFI interface defined in botho-mobile.
 * Keep in sync with mobile/rust-bridge/src/botho_mobile.udl
 */

/** Public wallet address (safe to display) */
export interface WalletAddress {
  /** View public key (hex encoded) */
  viewPublicKey: string;
  /** Spend public key (hex encoded) */
  spendPublicKey: string;
  /** Display format (cad:...) */
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
  /** Timestamp (Unix milliseconds) */
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

/** Native wallet module interface */
export interface NativeWalletModule {
  /** Set the node URL for RPC connections */
  setNodeUrl(url: string): Promise<void>;

  /** Generate a new wallet, returns mnemonic phrase */
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
}
