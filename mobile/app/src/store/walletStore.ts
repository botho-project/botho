/**
 * Wallet Store
 *
 * Global state management for the mobile wallet using Zustand.
 * Integrates with the native Rust module for actual wallet operations.
 */

import { create } from "zustand";
import type {
  WalletAddress,
  WalletBalance,
  TransactionEntry,
  SessionStatus,
} from "../types/wallet";

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
}

/** Wallet actions */
interface WalletActions {
  // Initialization
  setNodeUrl: (url: string) => void;

  // Session management
  unlock: (mnemonic: string) => Promise<void>;
  lock: () => Promise<void>;
  checkSession: () => Promise<void>;

  // Wallet operations
  refreshBalance: () => Promise<void>;
  refreshTransactions: (limit?: number) => Promise<void>;

  // Error handling
  clearError: () => void;
}

type WalletStore = WalletState & WalletActions;

/** Default node URL (testnet) */
const DEFAULT_NODE_URL = "https://testnet.botho.network:8443";

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

  // Set node URL
  setNodeUrl: (url: string) => {
    set({ nodeUrl: url });
    // TODO: Call native module to update URL
  },

  // Unlock wallet with mnemonic
  unlock: async (mnemonic: string) => {
    set({ isLoading: true, error: null });

    try {
      // TODO: Call native module
      // const address = await NativeWallet.unlockWithMnemonic(mnemonic);

      // Simulate for now
      await new Promise((resolve) => setTimeout(resolve, 500));

      const mockAddress: WalletAddress = {
        viewPublicKey: "0".repeat(64),
        spendPublicKey: "1".repeat(64),
        display: "cad:0000...1111",
      };

      set({
        isUnlocked: true,
        address: mockAddress,
        expiresAt: new Date(Date.now() + 15 * 60 * 1000), // 15 min
        isLoading: false,
      });

      // Auto-refresh balance after unlock
      get().refreshBalance();
    } catch (error) {
      set({
        isLoading: false,
        error: error instanceof Error ? error.message : "Failed to unlock",
      });
    }
  },

  // Lock wallet
  lock: async () => {
    set({ isLoading: true });

    try {
      // TODO: Call native module
      // await NativeWallet.lock();

      set({
        isUnlocked: false,
        address: null,
        balance: null,
        transactions: [],
        expiresAt: null,
        isLoading: false,
      });
    } catch (error) {
      set({
        isLoading: false,
        error: error instanceof Error ? error.message : "Failed to lock",
      });
    }
  },

  // Check session status
  checkSession: async () => {
    try {
      // TODO: Call native module
      // const status = await NativeWallet.getSessionStatus();

      const { expiresAt } = get();

      if (expiresAt && new Date() > expiresAt) {
        // Session expired
        set({
          isUnlocked: false,
          address: null,
          balance: null,
          transactions: [],
          expiresAt: null,
        });
      }
    } catch (error) {
      console.error("Failed to check session:", error);
    }
  },

  // Refresh balance
  refreshBalance: async () => {
    const { isUnlocked } = get();
    if (!isUnlocked) return;

    set({ isLoading: true });

    try {
      // TODO: Call native module
      // const balance = await NativeWallet.getBalance();

      // Simulate for now
      await new Promise((resolve) => setTimeout(resolve, 300));

      const mockBalance: WalletBalance = {
        picocredits: BigInt(0),
        formatted: "0.000000 BTH",
        utxoCount: 0,
        syncHeight: 0,
      };

      set({ balance: mockBalance, isLoading: false });
    } catch (error) {
      set({
        isLoading: false,
        error: error instanceof Error ? error.message : "Failed to get balance",
      });
    }
  },

  // Refresh transactions
  refreshTransactions: async (limit = 20) => {
    const { isUnlocked } = get();
    if (!isUnlocked) return;

    set({ isLoading: true });

    try {
      // TODO: Call native module
      // const txs = await NativeWallet.getTransactionHistory(limit, 0);

      // Simulate for now
      await new Promise((resolve) => setTimeout(resolve, 300));

      set({ transactions: [], isLoading: false });
    } catch (error) {
      set({
        isLoading: false,
        error:
          error instanceof Error ? error.message : "Failed to get transactions",
      });
    }
  },

  // Clear error
  clearError: () => {
    set({ error: null });
  },
}));
