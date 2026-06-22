/**
 * Node configuration + user-managed node list.
 *
 * The wallet is a thin client: it scans and submits transactions through a
 * single trusted node (its RPC ingress). Historically the user could only pick
 * one of three hardcoded testnet nodes; epic #441 P3 replaces that with a
 * user-managed list of trusted nodes:
 *
 *   - The 3 live testnet nodes are *seeded* into the list as defaults (so the
 *     app works out of the box), but they are no longer the only options.
 *   - The user can add any node by RPC URL after verifying its identity
 *     (`node_getIdentity`, #500) and explicitly confirming trust.
 *   - The list is persisted (secure store) and the active node drives
 *     `setNodeUrl` into the native bridge.
 *
 * `ManagedNode` is the persisted shape; `NodeOption` remains the static config
 * shape for the seed entries (kept for backwards-compatibility with callers
 * such as the faucet flow).
 */

import type { NodeIdentity } from "../types/wallet";

export interface NodeOption {
  /** Stable id for persistence + selection. */
  id: string;
  /** Human-readable label shown in the picker. */
  label: string;
  /** RPC base URL passed to `setNodeUrl`. */
  url: string;
  /** Short description of the node's role. */
  description: string;
  /** Whether this node serves the faucet RPC. */
  isFaucet: boolean;
}

/**
 * A user-managed trusted node entry (persisted).
 *
 * Seed entries (`source: "seed"`) ship with the app; `source: "user"` entries
 * are added by the user after identity verification. `verifiedIdentity` records
 * the identity the user confirmed at add-time so the picker can display it and
 * later flag drift (e.g. the node now reports a different network / peer ID).
 */
export interface ManagedNode {
  /** Stable id for persistence + selection. */
  id: string;
  /** Human-readable label shown in the picker. */
  label: string;
  /** RPC base URL passed to `setNodeUrl`. */
  url: string;
  /** Short description of the node's role. */
  description: string;
  /** Whether this node serves the faucet RPC. */
  isFaucet: boolean;
  /** Where this entry came from. Seed entries cannot be removed. */
  source: "seed" | "user";
  /** Identity confirmed by the user when the node was added (if verified). */
  verifiedIdentity?: NodeIdentity;
}

/**
 * The 3 live testnet nodes. `seed2` has no confirmed DNS name on the testnet,
 * so it is addressed by its public IP.
 */
export const TESTNET_NODES: NodeOption[] = [
  {
    id: "seed",
    label: "Seed 1",
    url: "https://seed.botho.io",
    description: "Primary SCP seed node",
    isFaucet: false,
  },
  {
    id: "seed2",
    label: "Seed 2",
    url: "https://34.220.36.204",
    description: "Secondary SCP seed node (IP, no DNS yet)",
    isFaucet: false,
  },
  {
    id: "faucet",
    label: "Faucet",
    url: "https://faucet.botho.io",
    description: "Faucet node — required for testnet coin requests",
    isFaucet: true,
  },
];

/** Default node selection (used until the user picks one). */
export const DEFAULT_NODE = TESTNET_NODES[0];

/**
 * Seed node list used to initialize a user's managed list on first run (or when
 * no persisted list exists). These remain selectable but are no longer the only
 * options — the user can add their own nodes alongside them.
 */
export function seedNodes(): ManagedNode[] {
  return TESTNET_NODES.map((n) => ({ ...n, source: "seed" as const }));
}

/** Find a seed node option by its URL. */
export function findNodeByUrl(url: string): NodeOption | undefined {
  return TESTNET_NODES.find((n) => n.url === url);
}

/** Find a managed node by URL within a list. */
export function findManagedByUrl(
  nodes: ManagedNode[],
  url: string
): ManagedNode | undefined {
  return nodes.find((n) => n.url === url);
}

/** The faucet seed node, if configured. */
export function faucetNode(): NodeOption | undefined {
  return TESTNET_NODES.find((n) => n.isFaucet);
}

/**
 * Derive a stable id for a user-added node from its URL. Reusing the URL keeps
 * ids deterministic and prevents accidental duplicates in the persisted list.
 */
export function nodeIdForUrl(url: string): string {
  return `user:${url}`;
}

/**
 * Derive a short, human-friendly label from a node URL (its hostname), used as
 * the default label when the user adds a node without naming it.
 */
export function labelFromUrl(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}
