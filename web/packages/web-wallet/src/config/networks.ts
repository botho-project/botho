/**
 * Network configuration for the Botho Web Wallet
 * Supports testnet, mainnet, and custom RPC endpoints
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
 * Predefined network configurations
 */
export const NETWORKS: Record<string, NetworkConfig> = {
  testnet: {
    id: 'testnet',
    name: 'Testnet',
    rpcEndpoint: getEnvRpcEndpoint() || 'https://seed.botho.io/rpc',
    faucetEndpoint: getEnvFaucetEndpoint() || 'https://faucet.botho.io/rpc',
    explorerUrl: 'https://explorer.testnet.botho.io',
    networkId: 'botho-testnet',
    isTestnet: true,
  },
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
    networkId: 'botho-custom',
    isTestnet: false,
  }
}

/**
 * Validate that an RPC endpoint is reachable
 */
export async function validateRpcEndpoint(endpoint: string): Promise<boolean> {
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
    return response.ok
  } catch {
    return false
  }
}

/**
 * Storage key for persisted network selection
 */
const NETWORK_STORAGE_KEY = 'botho_selected_network'
const CUSTOM_ENDPOINT_KEY = 'botho_custom_endpoint'

/**
 * Save selected network to localStorage
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
 * Load selected network from localStorage
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
