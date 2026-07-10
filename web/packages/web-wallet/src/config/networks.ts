/**
 * Network configuration for the Botho Web Wallet
 *
 * The wallet talks to a single "ingress" node — the SCP node the user trusts to
 * relay their RPC. The live testnet exposes three such nodes; the user picks
 * WHICH one is their ingress (persisted in localStorage). All read/write RPC is
 * routed to the selected ingress. Faucet RPC (`faucet_request` /
 * `faucet_getStatus`) only lives on the faucet node, so it is pinned there
 * regardless of which ingress is selected.
 */

export interface NetworkConfig {
  id: string
  name: string
  rpcEndpoint: string
  faucetEndpoint?: string
  explorerUrl?: string
  networkId: string
  isTestnet: boolean
}

/**
 * One of the live SCP nodes the user can choose as their RPC ingress.
 */
export interface IngressNode {
  /** Stable id, persisted in localStorage. */
  id: string
  /** Human-readable label shown in the picker. */
  name: string
  /** Short role description (validator / faucet node). */
  role: string
  /** Absolute JSON-RPC endpoint (https://host/rpc). */
  rpcEndpoint: string
  /** Whether this node also serves the faucet RPC. */
  servesFaucet: boolean
}

/**
 * Get RPC endpoint from environment variable or use default
 */
function getEnvRpcEndpoint(): string | undefined {
  return import.meta.env.VITE_RPC_ENDPOINT as string | undefined
}

/**
 * Get faucet endpoint from environment variable or use default
 */
function getEnvFaucetEndpoint(): string | undefined {
  return import.meta.env.VITE_FAUCET_ENDPOINT as string | undefined
}

/**
 * The live testnet nodes the user can select as their trusted ingress.
 *
 * Each is a real node with TLS + CORS enabled for browser access: three US
 * validators (seed/seed2/faucet) and two regional relay seeds (eu/ap, #613).
 * Health checks surface reachability so users can see when a node is down
 * before selecting it.
 */
export const INGRESS_NODES: IngressNode[] = [
  {
    id: 'seed',
    name: 'US seed 1',
    role: 'Primary SCP validator — Oregon',
    rpcEndpoint: 'https://seed.botho.io/rpc',
    servesFaucet: false,
  },
  {
    id: 'seed2',
    name: 'US seed 2',
    role: 'Secondary SCP validator — Oregon',
    rpcEndpoint: 'https://seed2.botho.io/rpc',
    servesFaucet: false,
  },
  {
    id: 'faucet',
    name: 'US faucet 1',
    role: 'SCP validator + faucet — Oregon',
    rpcEndpoint: 'https://faucet.botho.io/rpc',
    servesFaucet: true,
  },
  // Regional relay seeds (#613), TLS-terminated on-box via nginx + Let's
  // Encrypt (#636) — the wallet is an HTTPS PWA, so plain-HTTP :17101
  // endpoints would be blocked as mixed content.
  {
    id: 'eu',
    name: 'EU seed 1',
    role: 'Regional relay seed — Frankfurt',
    rpcEndpoint: 'https://eu.seed.botho.io/rpc',
    servesFaucet: false,
  },
  {
    id: 'ap',
    name: 'AP seed 1',
    role: 'Regional relay seed — Singapore',
    rpcEndpoint: 'https://ap.seed.botho.io/rpc',
    servesFaucet: false,
  },
]

/** The endpoint that serves faucet RPC, pinned regardless of ingress choice. */
export const FAUCET_ENDPOINT =
  getEnvFaucetEndpoint() ||
  INGRESS_NODES.find((n) => n.servesFaucet)?.rpcEndpoint ||
  'https://faucet.botho.io/rpc'

/** Default ingress node id when none is persisted. */
export const DEFAULT_INGRESS_ID = 'seed'

/** Find an ingress node by id. */
export function getIngressNode(id: string): IngressNode | undefined {
  return INGRESS_NODES.find((n) => n.id === id)
}

/**
 * Build the effective NetworkConfig for a given ingress node.
 *
 * The selected ingress supplies the read/write RPC endpoint; the faucet endpoint
 * is always pinned to the faucet node. When `VITE_RPC_ENDPOINT` is set (e2e /
 * same-origin-proxy builds) it overrides the ingress endpoint so the hermetic
 * test harness keeps working.
 */
export function networkForIngress(ingress: IngressNode): NetworkConfig {
  return {
    id: `testnet:${ingress.id}`,
    name: 'Testnet',
    rpcEndpoint: getEnvRpcEndpoint() || ingress.rpcEndpoint,
    faucetEndpoint: FAUCET_ENDPOINT,
    explorerUrl: 'https://botho.io/explorer',
    networkId: 'botho-testnet',
    isTestnet: true,
  }
}

/**
 * Predefined network configurations.
 *
 * Kept for backwards compatibility (the explorer / wallet read `NETWORKS` and
 * `DEFAULT_NETWORK_ID`). `testnet` resolves to the default ingress node.
 */
export const NETWORKS: Record<string, NetworkConfig> = {
  testnet: networkForIngress(getIngressNode(DEFAULT_INGRESS_ID) || INGRESS_NODES[0]),
}

/**
 * Default network during beta
 */
export const DEFAULT_NETWORK_ID = 'testnet'

/**
 * Get network configuration by ID
 */
export function getNetwork(id: string): NetworkConfig | undefined {
  return NETWORKS[id]
}

/**
 * Get all available networks
 */
export function getNetworks(): NetworkConfig[] {
  return Object.values(NETWORKS)
}

/**
 * Create a custom network configuration
 */
export function createCustomNetwork(rpcEndpoint: string, name?: string): NetworkConfig {
  return {
    id: 'custom',
    name: name || 'Custom',
    rpcEndpoint,
    faucetEndpoint: FAUCET_ENDPOINT,
    networkId: 'botho-custom',
    isTestnet: false,
  }
}

/** Health snapshot for an ingress node, from `node_getStatus`. */
export interface NodeHealth {
  /** 'online' if node_getStatus succeeded, 'offline' on error/timeout. */
  status: 'online' | 'offline' | 'checking'
  /** Current chain height (when online). */
  chainHeight?: number
  /** Whether the node reports itself synced. */
  synced?: boolean
  /** Sync progress percentage (0-100). */
  syncProgress?: number
}

/**
 * Query a node's health via `node_getStatus`. Never throws — returns an
 * 'offline' snapshot on any network/timeout/RPC error.
 */
export async function fetchNodeHealth(endpoint: string): Promise<NodeHealth> {
  try {
    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), 5000)

    const response = await fetch(endpoint, {
      method: 'POST',
      signal: controller.signal,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method: 'node_getStatus',
        params: {},
        id: 1,
      }),
    })

    clearTimeout(timeoutId)
    if (!response.ok) return { status: 'offline' }

    const json = (await response.json()) as {
      result?: { chainHeight?: number; synced?: boolean; syncProgress?: number }
      error?: unknown
    }
    if (json.error || !json.result) return { status: 'offline' }

    return {
      status: 'online',
      chainHeight: json.result.chainHeight,
      synced: json.result.synced,
      syncProgress: json.result.syncProgress,
    }
  } catch {
    return { status: 'offline' }
  }
}

/**
 * Validate that an RPC endpoint is reachable
 */
export async function validateRpcEndpoint(endpoint: string): Promise<boolean> {
  const health = await fetchNodeHealth(endpoint)
  return health.status === 'online'
}

/**
 * Storage key for persisted ingress / network selection
 */
const INGRESS_STORAGE_KEY = 'botho_selected_ingress'
const NETWORK_STORAGE_KEY = 'botho_selected_network'
const CUSTOM_ENDPOINT_KEY = 'botho_custom_endpoint'
/**
 * Records the host of a custom node the user accepted *from a deep link* (#587).
 * Persisted so the "connected to custom node <host> (from a link)" banner
 * survives a reload, reminding the user they are off the default seeds. Cleared
 * whenever the user reverts or picks a built-in ingress.
 */
const CUSTOM_NODE_FROM_LINK_KEY = 'botho_custom_node_from_link'

/**
 * Save selected ingress node to localStorage.
 */
export function saveSelectedIngress(ingressId: string): void {
  try {
    localStorage.setItem(INGRESS_STORAGE_KEY, ingressId)
    // Keep the legacy network key consistent so older reads stay valid.
    localStorage.setItem(NETWORK_STORAGE_KEY, DEFAULT_NETWORK_ID)
    localStorage.removeItem(CUSTOM_ENDPOINT_KEY)
    // Choosing a built-in ingress means we are no longer on a link-supplied node.
    localStorage.removeItem(CUSTOM_NODE_FROM_LINK_KEY)
  } catch {
    // localStorage may not be available
  }
}

/**
 * Persist that the active custom node was accepted from a deep link (#587),
 * keyed by host. Drives the "from a link" banner.
 */
export function saveCustomNodeFromLink(host: string): void {
  try {
    localStorage.setItem(CUSTOM_NODE_FROM_LINK_KEY, host)
  } catch {
    // localStorage may not be available
  }
}

/**
 * Load the host of the link-supplied custom node, or `null` when the active node
 * was not accepted from a link (or storage is unavailable).
 */
export function loadCustomNodeFromLink(): string | null {
  try {
    return localStorage.getItem(CUSTOM_NODE_FROM_LINK_KEY)
  } catch {
    return null
  }
}

/** Forget the link-supplied custom node marker (on revert / manual switch). */
export function clearCustomNodeFromLink(): void {
  try {
    localStorage.removeItem(CUSTOM_NODE_FROM_LINK_KEY)
  } catch {
    // localStorage may not be available
  }
}

/**
 * Load the selected ingress node id from localStorage.
 */
export function loadSelectedIngress(): string {
  try {
    return localStorage.getItem(INGRESS_STORAGE_KEY) || DEFAULT_INGRESS_ID
  } catch {
    return DEFAULT_INGRESS_ID
  }
}

/**
 * Save selected network to localStorage (legacy custom-endpoint path).
 */
export function saveSelectedNetwork(networkId: string, customEndpoint?: string): void {
  try {
    localStorage.setItem(NETWORK_STORAGE_KEY, networkId)
    if (customEndpoint) {
      localStorage.setItem(CUSTOM_ENDPOINT_KEY, customEndpoint)
    } else {
      localStorage.removeItem(CUSTOM_ENDPOINT_KEY)
    }
  } catch {
    // localStorage may not be available
  }
}

/**
 * Load selected network from localStorage (legacy custom-endpoint path).
 */
export function loadSelectedNetwork(): { networkId: string; customEndpoint?: string } {
  try {
    const networkId = localStorage.getItem(NETWORK_STORAGE_KEY) || DEFAULT_NETWORK_ID
    const customEndpoint = localStorage.getItem(CUSTOM_ENDPOINT_KEY) || undefined
    return { networkId, customEndpoint }
  } catch {
    return { networkId: DEFAULT_NETWORK_ID }
  }
}
