/**
 * Native Wallet Module (TypeScript wrapper around the UniFFI rust-bridge).
 *
 * The cryptographic core lives in `mobile/rust-bridge` (the `botho-mobile`
 * crate, merged in #447). UniFFI generates Swift/Kotlin bindings from that
 * crate (see `mobile/app/ios/Generated/` and `mobile/app/android/generated/`).
 *
 * UniFFI does NOT generate the React-Native <-> native glue: a small Expo
 * native module re-exposes the generated `MobileWallet` object to JS. This file
 * is the single typed entry point the rest of the app uses; it:
 *
 *   1. Resolves that native module (`BothoWallet`) if it has been linked into
 *      the build (a custom dev client / prebuild — NOT Expo Go).
 *   2. Adapts the FFI value shapes to the app's TS types — in particular it
 *      converts the 64-bit integer fields (which the native bridge marshals as
 *      decimal strings to avoid JS `number` precision loss) into `bigint`.
 *   3. Surfaces a clear, actionable error if the native module is missing, so
 *      UI development in Expo Go still runs and the failure mode is obvious.
 *
 * See `mobile/NATIVE_INTEGRATION.md` for the exact native-link / prebuild step
 * that wires this module on a device or simulator.
 */

import { requireOptionalNativeModule } from "expo-modules-core";
import type {
  WalletAddress,
  WalletBalance,
  TransactionEntry,
  SessionStatus,
  FaucetResult,
  NodeStatusInfo,
  NativeWalletModule,
} from "../types/wallet";

/**
 * Raw shape of the values returned by the native bridge.
 *
 * 64-bit integers cross the JS bridge as decimal strings (or, defensively, as
 * `number`/`bigint` depending on the native runtime) — never as raw JS numbers
 * for amounts, which would silently lose precision above 2^53. The adapters
 * below normalize these.
 */
type Numeric = string | number | bigint;

interface RawWalletBalance {
  picocredits: Numeric;
  formatted: string;
  utxoCount: Numeric;
  syncHeight: Numeric;
}

interface RawTransactionEntry {
  txHash: string;
  amount: Numeric;
  blockHeight: Numeric;
  timestamp: Numeric;
  direction: string;
  counterparty?: string | null;
}

interface RawSessionStatus {
  isUnlocked: boolean;
  address?: WalletAddress | null;
  expiresInSeconds?: Numeric | null;
}

interface RawFaucetResult {
  success: boolean;
  txHash: string;
  amount: Numeric;
  amountFormatted: string;
  message: string;
}

interface RawNodeStatusInfo {
  version: string;
  network: string;
  chainHeight: Numeric;
  syncStatus: string;
  peerCount: Numeric;
}

/**
 * The native module surface. A single long-lived `MobileWallet` instance lives
 * in native memory (it holds the in-session keys); the bridge exposes its
 * methods as module functions operating on that singleton.
 */
interface RawNativeWalletModule {
  setNodeUrl(url: string): Promise<void>;
  generateWallet(): Promise<string>;
  unlockWithMnemonic(mnemonic: string): Promise<WalletAddress>;
  lock(): Promise<boolean>;
  getSessionStatus(): Promise<RawSessionStatus>;
  getBalance(): Promise<RawWalletBalance>;
  getTransactionHistory(
    limit: number,
    offset: number
  ): Promise<RawTransactionEntry[]>;
  getAddress(): Promise<WalletAddress>;
  sendTransaction(toAddress: string, amountPicocredits: string): Promise<string>;
  requestFaucet(): Promise<RawFaucetResult>;
  getNodeStatus(): Promise<RawNodeStatusInfo>;
}

/** Native module name registered by the Expo native module (iOS + Android). */
const NATIVE_MODULE_NAME = "BothoWallet";

/**
 * Resolve the native module if present. `null` in Expo Go or any build where
 * the rust-bridge native module has not been linked.
 */
const native = requireOptionalNativeModule<RawNativeWalletModule>(
  NATIVE_MODULE_NAME
);

/** True when the native rust-bridge is linked into this build. */
export function isNativeWalletAvailable(): boolean {
  return native != null;
}

class NativeModuleUnavailableError extends Error {
  constructor() {
    super(
      `Native wallet module "${NATIVE_MODULE_NAME}" is not available. ` +
        "The rust-bridge must be compiled and linked into a custom dev client " +
        "(it is not present in Expo Go). See mobile/NATIVE_INTEGRATION.md."
    );
    this.name = "NativeModuleUnavailableError";
  }
}

function requireNative(): RawNativeWalletModule {
  if (native == null) {
    throw new NativeModuleUnavailableError();
  }
  return native;
}

/** Convert an FFI numeric (string | number | bigint) to `bigint`. */
function toBigInt(value: Numeric): bigint {
  if (typeof value === "bigint") return value;
  if (typeof value === "number") return BigInt(Math.trunc(value));
  return BigInt(value);
}

/** Convert an FFI numeric to a JS `number` (for small, in-range fields). */
function toNumber(value: Numeric): number {
  if (typeof value === "number") return value;
  return Number(value);
}

function adaptBalance(raw: RawWalletBalance): WalletBalance {
  return {
    picocredits: toBigInt(raw.picocredits),
    formatted: raw.formatted,
    utxoCount: toNumber(raw.utxoCount),
    syncHeight: toNumber(raw.syncHeight),
  };
}

function adaptTransaction(raw: RawTransactionEntry): TransactionEntry {
  const direction = raw.direction === "send" ? "send" : "receive";
  return {
    txHash: raw.txHash,
    amount: toBigInt(raw.amount),
    blockHeight: toNumber(raw.blockHeight),
    timestamp: toNumber(raw.timestamp),
    direction,
    counterparty: raw.counterparty ?? undefined,
  };
}

function adaptSession(raw: RawSessionStatus): SessionStatus {
  return {
    isUnlocked: raw.isUnlocked,
    address: raw.address ?? undefined,
    expiresInSeconds:
      raw.expiresInSeconds == null ? undefined : toNumber(raw.expiresInSeconds),
  };
}

function adaptFaucet(raw: RawFaucetResult): FaucetResult {
  return {
    success: raw.success,
    txHash: raw.txHash,
    amount: toBigInt(raw.amount),
    amountFormatted: raw.amountFormatted,
    message: raw.message,
  };
}

function adaptNodeStatus(raw: RawNodeStatusInfo): NodeStatusInfo {
  return {
    version: raw.version,
    network: raw.network,
    chainHeight: toNumber(raw.chainHeight),
    syncStatus: raw.syncStatus,
    peerCount: toNumber(raw.peerCount),
  };
}

/**
 * Typed, adapted view of the rust-bridge wallet. Every method calls the real
 * UniFFI surface; numeric marshalling is normalized to the app's TS types.
 */
export const NativeWallet: NativeWalletModule = {
  async setNodeUrl(url: string): Promise<void> {
    return requireNative().setNodeUrl(url);
  },

  async generateWallet(): Promise<string> {
    return requireNative().generateWallet();
  },

  async unlockWithMnemonic(mnemonic: string): Promise<WalletAddress> {
    return requireNative().unlockWithMnemonic(mnemonic);
  },

  async lock(): Promise<boolean> {
    return requireNative().lock();
  },

  async getSessionStatus(): Promise<SessionStatus> {
    return adaptSession(await requireNative().getSessionStatus());
  },

  async getBalance(): Promise<WalletBalance> {
    return adaptBalance(await requireNative().getBalance());
  },

  async getTransactionHistory(
    limit: number,
    offset: number
  ): Promise<TransactionEntry[]> {
    const raw = await requireNative().getTransactionHistory(limit, offset);
    return raw.map(adaptTransaction);
  },

  async getAddress(): Promise<WalletAddress> {
    return requireNative().getAddress();
  },

  async sendTransaction(
    toAddress: string,
    amountPicocredits: bigint
  ): Promise<string> {
    // Marshal the amount as a decimal string to preserve full u64 precision
    // across the JS bridge.
    return requireNative().sendTransaction(
      toAddress,
      amountPicocredits.toString()
    );
  },

  async requestFaucet(): Promise<FaucetResult> {
    return adaptFaucet(await requireNative().requestFaucet());
  },

  async getNodeStatus(): Promise<NodeStatusInfo> {
    return adaptNodeStatus(await requireNative().getNodeStatus());
  },
};
