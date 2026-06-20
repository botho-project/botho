/**
 * Wallet Store
 *
 * Global state management for the mobile wallet using Zustand.
 *
 * Every operation calls the real UniFFI rust-bridge via `NativeWallet`
 * (src/native/walletModule.ts). There is no mock data here: balance, history,
 * send, faucet, node status and session all flow through the merged bridge
 * (#447). The bridge holds the wallet keys in native session memory; the
 * mnemonic is passed once on unlock and never persisted in JS.
 */

import { create } from "zustand";
import type {
  WalletAddress,
  WalletBalance,
  TransactionEntry,
  FaucetResult,
  NodeStatusInfo,
} from "../types/wallet";
import { NativeWallet } from "../native/walletModule";
import { DEFAULT_NODE } from "../config/nodes";
import { saveNodeUrl, loadNodeUrl } from "../native/keychain";

/** Extract a user-facing message from a thrown bridge error. */
function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string") return error;
  return fallback;
}

/** Wallet state */
interface WalletState {
  // Session state
  isUnlocked: boolean;
  isLoading: boolean;
  error: string | null;

  // Wallet data
  address: WalletAddress | null;
  balance: WalletBalance | null;
  transactions: TransactionEntry[];

  // Session info
  expiresAt: Date | null;

  // Network
  nodeUrl: string;
  isConnected: boolean;
  nodeStatus: NodeStatusInfo | null;
}

/** Wallet actions */
interface WalletActions {
  // Network / node selection
  setNodeUrl: (url: string) => Promise<void>;
  refreshNodeStatus: () => Promise<void>;
  hydrateNodeUrl: () => Promise<void>;

  // Session management
  unlock: (mnemonic: string) => Promise<void>;
  lock: () => Promise<void>;
  checkSession: () => Promise<void>;

  // Wallet operations
  refreshBalance: () => Promise<void>;
  refreshTransactions: (limit?: number) => Promise<void>;
  send: (toAddress: string, amountPicocredits: bigint) => Promise<string>;
  requestFaucet: () => Promise<FaucetResult>;

  // Error handling
  clearError: () => void;
}

type WalletStore = WalletState & WalletActions;

/** Default node URL (first live testnet node). */
const DEFAULT_NODE_URL = DEFAULT_NODE.url;

/**
 * Wallet store instance
 *
 * Usage:
 * ```typescript
 * const { isUnlocked, unlock, balance } = useWalletStore();
 * ```
 */
export const useWalletStore = create<WalletStore>((set, get) => ({
  // Initial state
  isUnlocked: false,
  isLoading: false,
  error: null,
  address: null,
  balance: null,
  transactions: [],
  expiresAt: null,
  nodeUrl: DEFAULT_NODE_URL,
  isConnected: false,
  nodeStatus: null,

  // Set node URL: persist it, push it to the native bridge, and refresh health.
  setNodeUrl: async (url: string) => {
    set({ nodeUrl: url, error: null });
    try {
      await NativeWallet.setNodeUrl(url);
      await saveNodeUrl(url);
      // Best-effort health probe; failure here is non-fatal.
      await get().refreshNodeStatus();
    } catch (error) {
      set({ error: errorMessage(error, "Failed to set node") });
    }
  },

  // Load the persisted node URL (or default) and apply it to the bridge.
  hydrateNodeUrl: async () => {
    try {
      const stored = await loadNodeUrl();
      const url = stored ?? get().nodeUrl ?? DEFAULT_NODE_URL;
      set({ nodeUrl: url });
      await NativeWallet.setNodeUrl(url);
    } catch (error) {
      // Fall back to the default; surface but do not block startup.
      set({ error: errorMessage(error, "Failed to load node selection") });
    }
  },

  // Fetch node health (height / sync / peers) for the picker.
  refreshNodeStatus: async () => {
    try {
      const status = await NativeWallet.getNodeStatus();
      set({ nodeStatus: status, isConnected: true });
    } catch (error) {
      set({
        nodeStatus: null,
        isConnected: false,
        error: errorMessage(error, "Node unreachable"),
      });
    }
  },

  // Unlock wallet with mnemonic
  unlock: async (mnemonic: string) => {
    set({ isLoading: true, error: null });

    try {
      // Make sure the bridge points at the selected node before any RPC ops.
      await NativeWallet.setNodeUrl(get().nodeUrl);

      const address = await NativeWallet.unlockWithMnemonic(mnemonic);

      set({
        isUnlocked: true,
        address,
        // Bridge enforces a 15-minute session timeout; mirror it locally.
        expiresAt: new Date(Date.now() + 15 * 60 * 1000),
        isLoading: false,
      });

      // Auto-refresh balance + history after unlock.
      get().refreshBalance();
      get().refreshTransactions();
    } catch (error) {
      set({
        isLoading: false,
        error: errorMessage(error, "Failed to unlock"),
      });
      throw error;
    }
  },

  // Lock wallet
  lock: async () => {
    set({ isLoading: true });

    try {
      await NativeWallet.lock();
    } catch (error) {
      // Even if the native lock errors, clear local state.
      console.error("Failed to lock:", error);
    } finally {
      set({
        isUnlocked: false,
        address: null,
        balance: null,
        transactions: [],
        expiresAt: null,
        isLoading: false,
      });
    }
  },

  // Check session status against the native bridge (authoritative).
  checkSession: async () => {
    try {
      const status = await NativeWallet.getSessionStatus();

      if (!status.isUnlocked) {
        // Only mutate if we currently think we're unlocked, to avoid churn.
        if (get().isUnlocked) {
          set({
            isUnlocked: false,
            address: null,
            balance: null,
            transactions: [],
            expiresAt: null,
          });
        }
        return;
      }

      set({
        isUnlocked: true,
        address: status.address ?? get().address,
        expiresAt:
          status.expiresInSeconds != null
            ? new Date(Date.now() + status.expiresInSeconds * 1000)
            : get().expiresAt,
      });
    } catch (error) {
      console.error("Failed to check session:", error);
    }
  },

  // Refresh balance
  refreshBalance: async () => {
    if (!get().isUnlocked) return;

    set({ isLoading: true });

    try {
      const balance = await NativeWallet.getBalance();
      set({ balance, isLoading: false, isConnected: true });
    } catch (error) {
      set({
        isLoading: false,
        error: errorMessage(error, "Failed to get balance"),
      });
    }
  },

  // Refresh transactions
  refreshTransactions: async (limit = 20) => {
    if (!get().isUnlocked) return;

    set({ isLoading: true });

    try {
      const transactions = await NativeWallet.getTransactionHistory(limit, 0);
      set({ transactions, isLoading: false });
    } catch (error) {
      set({
        isLoading: false,
        error: errorMessage(error, "Failed to get transactions"),
      });
    }
  },

  // Send a transfer. Returns the tx hash on success; throws on failure so the
  // caller (send screen) can show inline status.
  send: async (toAddress: string, amountPicocredits: bigint) => {
    if (!get().isUnlocked) {
      throw new Error("Wallet is locked");
    }

    set({ isLoading: true, error: null });

    try {
      const txHash = await NativeWallet.sendTransaction(
        toAddress,
        amountPicocredits
      );
      set({ isLoading: false });
      // Refresh balance + history after a successful submit.
      get().refreshBalance();
      get().refreshTransactions();
      return txHash;
    } catch (error) {
      const message = errorMessage(error, "Failed to send transaction");
      set({ isLoading: false, error: message });
      throw error instanceof Error ? error : new Error(message);
    }
  },

  // Request testnet coins from the faucet for the current address.
  requestFaucet: async () => {
    if (!get().isUnlocked) {
      throw new Error("Wallet is locked");
    }

    set({ isLoading: true, error: null });

    try {
      const result = await NativeWallet.requestFaucet();
      set({ isLoading: false });
      if (!result.success && result.message) {
        set({ error: result.message });
      }
      return result;
    } catch (error) {
      const message = errorMessage(error, "Faucet request failed");
      set({ isLoading: false, error: message });
      throw error instanceof Error ? error : new Error(message);
    }
  },

  // Clear error
  clearError: () => {
    set({ error: null });
  },
}));
