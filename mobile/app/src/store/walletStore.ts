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
import {
  DEFAULT_NODE,
  seedNodes,
  nodeIdForUrl,
  labelFromUrl,
  type ManagedNode,
} from "../config/nodes";
import {
  saveNodeUrl,
  loadNodeUrl,
  saveNodeList,
  loadNodeList,
} from "../native/keychain";
import { fetchNodeIdentity, normalizeNodeUrl } from "../native/nodeIdentity";
import type { NodeIdentity } from "../types/wallet";
import { compareNetwork, isProtocolCompatible } from "../config/network";

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

  // User-managed list of trusted nodes (seeded with the testnet defaults).
  nodes: ManagedNode[];
}

/** Result of verifying a candidate node before adding it. */
export interface VerifyNodeResult {
  /** Normalized URL that was probed. */
  url: string;
  /** The identity the node reported. */
  identity: NodeIdentity;
  /** Whether the node's network matches the wallet's expected network. */
  networkMatches: boolean;
  /** Whether the node's protocol is compatible with this client. */
  protocolCompatible: boolean;
}

/** Wallet actions */
interface WalletActions {
  // Network / node selection
  setNodeUrl: (url: string) => Promise<void>;
  refreshNodeStatus: () => Promise<void>;
  hydrateNodeUrl: () => Promise<void>;

  // User-managed trusted node list
  hydrateNodes: () => Promise<void>;
  verifyNode: (url: string) => Promise<VerifyNodeResult>;
  addVerifiedNode: (
    result: VerifyNodeResult,
    label?: string
  ) => Promise<ManagedNode>;
  removeNode: (id: string) => Promise<void>;

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
  // Seeded with the testnet defaults; replaced by the persisted list on hydrate.
  nodes: seedNodes(),

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

  // Load the persisted user-managed node list (or seed defaults on first run).
  hydrateNodes: async () => {
    try {
      const stored = await loadNodeList();
      if (stored && stored.length > 0) {
        set({ nodes: stored });
      } else {
        // First run: seed with the testnet defaults and persist them so the
        // user can edit the list from a stable starting point.
        const seeds = seedNodes();
        set({ nodes: seeds });
        await saveNodeList(seeds);
      }
    } catch (error) {
      // Non-fatal: keep the in-memory seed list.
      set({ error: errorMessage(error, "Failed to load node list") });
    }
  },

  // Verify a candidate node's identity (node_getIdentity) before trusting it.
  // Throws NodeIdentityError on unreachable / non-Botho / older nodes so the UI
  // can show an inline error; on success returns the identity plus the
  // network/protocol compatibility flags for the confirmation step.
  verifyNode: async (url: string): Promise<VerifyNodeResult> => {
    const normalized = normalizeNodeUrl(url);
    const identity = await fetchNodeIdentity(normalized);
    return {
      url: normalized,
      identity,
      networkMatches: compareNetwork(identity.network) === "match",
      protocolCompatible: isProtocolCompatible(
        identity.protocolVersion,
        identity.minProtocolVersion
      ),
    };
  },

  // Add a node the user has verified and chosen to trust. Persists the updated
  // list. If a node with the same URL already exists it is updated in place
  // (re-verification refreshes the stored identity) rather than duplicated.
  addVerifiedNode: async (
    result: VerifyNodeResult,
    label?: string
  ): Promise<ManagedNode> => {
    const entry: ManagedNode = {
      id: nodeIdForUrl(result.url),
      label: label?.trim() || labelFromUrl(result.url),
      url: result.url,
      description: "User-added trusted node",
      isFaucet: false,
      source: "user",
      verifiedIdentity: result.identity,
    };

    const existing = get().nodes;
    const idx = existing.findIndex((n) => n.url === result.url);
    const next =
      idx >= 0
        ? existing.map((n, i) =>
            i === idx ? { ...n, ...entry, source: n.source } : n
          )
        : [...existing, entry];

    set({ nodes: next });
    try {
      await saveNodeList(next);
    } catch (error) {
      set({ error: errorMessage(error, "Failed to save node") });
    }
    return entry;
  },

  // Remove a user-added node. Seed nodes cannot be removed (they are the
  // app's known-good defaults). If the removed node is the active one, fall
  // back to the first remaining node.
  removeNode: async (id: string): Promise<void> => {
    const existing = get().nodes;
    const target = existing.find((n) => n.id === id);
    if (!target || target.source === "seed") return;

    const next = existing.filter((n) => n.id !== id);
    set({ nodes: next });

    // If we removed the active node, switch to a remaining one.
    if (target.url === get().nodeUrl) {
      const fallback = next[0];
      if (fallback) {
        await get().setNodeUrl(fallback.url);
      }
    }

    try {
      await saveNodeList(next);
    } catch (error) {
      set({ error: errorMessage(error, "Failed to remove node") });
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
