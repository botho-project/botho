/**
 * Network configuration presets for the Botho desktop wallet.
 *
 * Defines RPC and faucet endpoints for different network environments.
 */

export interface NetworkConfig {
  /** Network identifier */
  id: string
  /** Human-readable network name */
  name: string
  /** RPC endpoint host */
  rpcHost: string
  /** RPC endpoint port */
  rpcPort: number
  /** Faucet endpoint host (null if faucet not available) */
  faucetHost: string | null
  /** Faucet endpoint port (null if faucet not available) */
  faucetPort: number | null
  /** Whether this is a testnet */
  isTestnet: boolean
}

/**
 * Predefined network configurations.
 */
export const NETWORKS: Record<string, NetworkConfig> = {
  /**
   * Public testnet - connects to seed.botho.io with faucet support.
   */
  testnet: {
    id: 'testnet',
    name: 'Testnet',
    rpcHost: 'seed.botho.io',
    rpcPort: 17101,
    faucetHost: 'faucet.botho.io',
    faucetPort: 17101,
    isTestnet: true,
  },

  /**
   * Local development network - connects to localhost.
   */
  local: {
    id: 'local',
    name: 'Local',
    rpcHost: '127.0.0.1',
    rpcPort: 27200,
    faucetHost: '127.0.0.1',
    faucetPort: 27200,
    isTestnet: true,
  },
}

/**
 * Default network to use when none is specified.
 */
export const DEFAULT_NETWORK = 'testnet'

/**
 * Get network configuration by ID.
 * Returns undefined if network not found.
 */
export function getNetwork(id: string): NetworkConfig | undefined {
  return NETWORKS[id]
}

/**
 * Check if a network has faucet support.
 */
export function hasFaucetSupport(network: NetworkConfig): boolean {
  return network.faucetHost !== null && network.faucetPort !== null
}

/**
 * Create a custom network configuration.
 */
export function createCustomNetwork(
  rpcHost: string,
  rpcPort: number,
  options?: {
    faucetHost?: string
    faucetPort?: number
    isTestnet?: boolean
  }
): NetworkConfig {
  return {
    id: 'custom',
    name: 'Custom',
    rpcHost,
    rpcPort,
    faucetHost: options?.faucetHost ?? null,
    faucetPort: options?.faucetPort ?? null,
    isTestnet: options?.isTestnet ?? false,
  }
}
