import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  type ReactNode,
} from 'react'
import {
  type NetworkConfig,
  NETWORKS,
  DEFAULT_NETWORK_ID,
  createCustomNetwork,
  saveSelectedNetwork,
  loadSelectedNetwork,
  validateRpcEndpoint,
} from '../config/networks'

interface NetworkState {
  /** Currently selected network */
  network: NetworkConfig
  /** Whether we're validating a custom endpoint */
  isValidating: boolean
  /** Validation error message */
  validationError: string | null
  /** Whether the network has a faucet available */
  hasFaucet: boolean
}

interface NetworkContextValue extends NetworkState {
  /** Switch to a different network */
  switchNetwork: (networkId: string) => void
  /** Set a custom RPC endpoint */
  setCustomEndpoint: (endpoint: string) => Promise<boolean>
  /** Get all available networks */
  availableNetworks: NetworkConfig[]
}

const NetworkContext = createContext<NetworkContextValue | null>(null)

/**
 * Get initial network configuration
 */
function getInitialNetwork(): NetworkConfig {
  const { networkId, customEndpoint } = loadSelectedNetwork()

  if (networkId === 'custom' && customEndpoint) {
    return createCustomNetwork(customEndpoint)
  }

  return NETWORKS[networkId] || NETWORKS[DEFAULT_NETWORK_ID]
}

export function NetworkProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<NetworkState>(() => {
    const network = getInitialNetwork()
    return {
      network,
      isValidating: false,
      validationError: null,
      hasFaucet: !!network.faucetEndpoint,
    }
  })

  // Notify listeners when network changes
  useEffect(() => {
    // Dispatch custom event for wallet context to pick up
    window.dispatchEvent(new CustomEvent('network-changed', {
      detail: { network: state.network }
    }))
  }, [state.network])

  const switchNetwork = useCallback((networkId: string) => {
    const network = NETWORKS[networkId]
    if (!network) {
      console.error(`Unknown network: ${networkId}`)
      return
    }

    saveSelectedNetwork(networkId)
    setState({
      network,
      isValidating: false,
      validationError: null,
      hasFaucet: !!network.faucetEndpoint,
    })
  }, [])

  const setCustomEndpoint = useCallback(async (endpoint: string): Promise<boolean> => {
    setState(s => ({
      ...s,
      isValidating: true,
      validationError: null,
    }))

    try {
      // Validate the endpoint is reachable
      const isValid = await validateRpcEndpoint(endpoint)

      if (!isValid) {
        setState(s => ({
          ...s,
          isValidating: false,
          validationError: 'Could not connect to endpoint',
        }))
        return false
      }

      const network = createCustomNetwork(endpoint)
      saveSelectedNetwork('custom', endpoint)

      setState({
        network,
        isValidating: false,
        validationError: null,
        hasFaucet: false,
      })

      return true
    } catch (err) {
      setState(s => ({
        ...s,
        isValidating: false,
        validationError: err instanceof Error ? err.message : 'Validation failed',
      }))
      return false
    }
  }, [])

  return (
    <NetworkContext.Provider
      value={{
        ...state,
        switchNetwork,
        setCustomEndpoint,
        availableNetworks: Object.values(NETWORKS),
      }}
    >
      {children}
    </NetworkContext.Provider>
  )
}

export function useNetwork() {
  const context = useContext(NetworkContext)
  if (!context) {
    throw new Error('useNetwork must be used within a NetworkProvider')
  }
  return context
}
