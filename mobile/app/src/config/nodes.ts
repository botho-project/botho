/**
 * Testnet node configuration.
 *
 * The three live testnet nodes for the demo. The user picks one trusted node
 * as the wallet's RPC ingress (the wallet is a thin client: it scans and
 * submits via this node). The faucet node hosts the `faucet_request` RPC, so
 * faucet requests should be pointed at it.
 */

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

/** Find a node option by its URL. */
export function findNodeByUrl(url: string): NodeOption | undefined {
  return TESTNET_NODES.find((n) => n.url === url);
}

/** The faucet node, if configured. */
export function faucetNode(): NodeOption | undefined {
  return TESTNET_NODES.find((n) => n.isFaucet);
}
