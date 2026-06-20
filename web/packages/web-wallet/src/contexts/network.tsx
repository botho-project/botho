import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  useRef,
  type ReactNode,
} from 'react'
import {
  type NetworkConfig,
  type IngressNode,
  type NodeHealth,
  NETWORKS,
  INGRESS_NODES,
  DEFAULT_NETWORK_ID,
  DEFAULT_INGRESS_ID,
  createCustomNetwork,
  networkForIngress,
  getIngressNode,
  saveSelectedNetwork,
  saveSelectedIngress,
  loadSelectedIngress,
  loadSelectedNetwork,
  validateRpcEndpoint,
  fetchNodeHealth,
} from '../config/networks'

interface NetworkState {
  /** Currently selected network (derived from the selected ingress node). */
  network: NetworkConfig
  /** Id of the selected ingress node (or 'custom'). */
  ingressId: string
  /** Whether we're validating a custom endpoint */
  isValidating: boolean
  /** Validation error message */
  validationError: string | null
  /** Whether the network has a faucet available */
  hasFaucet: boolean
  /** Per-ingress-node health snapshots, keyed by ingress id. */
  nodeHealth: Record<string, NodeHealth>
}

interface NetworkContextValue extends NetworkState {
  /** Pick which SCP node is the trusted RPC ingress. */
  selectIngress: (ingressId: string) => void
  /** Switch to a different network (legacy; maps testnet -> default ingress). */
  switchNetwork: (networkId: string) => void
  /** Set a custom RPC endpoint */
  setCustomEndpoint: (endpoint: string) => Promise<boolean>
  /** Get all available networks */
  availableNetworks: NetworkConfig[]
  /** The selectable ingress nodes. */
  ingressNodes: IngressNode[]
  /** Re-run the health checks for every ingress node. */
  refreshHealth: () => void
}

const NetworkContext = createContext<NetworkContextValue | null>(null)

/** Health is re-polled on this cadence (ms). */
const HEALTH_POLL_INTERVAL = 20_000

/**
 * Get initial network configuration from the persisted ingress selection
 * (falling back to the legacy custom-endpoint path).
 */
function getInitialNetwork(): { network: NetworkConfig; ingressId: string } {
  const { networkId, customEndpoint } = loadSelectedNetwork()
  if (networkId === 'custom' && customEndpoint) {
    return { network: createCustomNetwork(customEndpoint), ingressId: 'custom' }
  }

  const ingressId = loadSelectedIngress()
  const ingress = getIngressNode(ingressId)
  if (ingress) {
    return { network: networkForIngress(ingress), ingressId: ingress.id }
  }

  return { network: NETWORKS[DEFAULT_NETWORK_ID], ingressId: DEFAULT_INGRESS_ID }
}

export function NetworkProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<NetworkState>(() => {
    const { network, ingressId } = getInitialNetwork()
    const nodeHealth: Record<string, NodeHealth> = {}
    for (const n of INGRESS_NODES) nodeHealth[n.id] = { status: 'checking' }
    return {
      network,
      ingressId,
      isValidating: false,
      validationError: null,
      hasFaucet: !!network.faucetEndpoint,
      nodeHealth,
    }
  })

  // Notify listeners when network changes
  useEffect(() => {
    // Dispatch custom event for wallet context to pick up
    window.dispatchEvent(new CustomEvent('network-changed', {
      detail: { network: state.network }
    }))
  }, [state.network])

  // Poll each ingress node's health via node_getStatus.
  const pollTimerRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const runHealthChecks = useCallback(() => {
    for (const node of INGRESS_NODES) {
      fetchNodeHealth(node.rpcEndpoint).then((health) => {
        setState((s) => ({
          ...s,
          nodeHealth: { ...s.nodeHealth, [node.id]: health },
        }))
      })
    }
  }, [])

  useEffect(() => {
    runHealthChecks()
    pollTimerRef.current = setInterval(runHealthChecks, HEALTH_POLL_INTERVAL)
    return () => {
      if (pollTimerRef.current) clearInterval(pollTimerRef.current)
    }
  }, [runHealthChecks])

  const selectIngress = useCallback((ingressId: string) => {
    const ingress = getIngressNode(ingressId)
    if (!ingress) {
      console.error(`Unknown ingress node: ${ingressId}`)
      return
    }
    saveSelectedIngress(ingressId)
    const network = networkForIngress(ingress)
    setState((s) => ({
      ...s,
      network,
      ingressId,
      isValidating: false,
      validationError: null,
      hasFaucet: !!network.faucetEndpoint,
    }))
  }, [])

  const switchNetwork = useCallback((networkId: string) => {
    // Legacy entry point: any non-custom network maps to the default ingress.
    if (networkId === 'custom') return
    selectIngress(DEFAULT_INGRESS_ID)
  }, [selectIngress])

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

      setState(s => ({
        ...s,
        network,
        ingressId: 'custom',
        isValidating: false,
        validationError: null,
        hasFaucet: !!network.faucetEndpoint,
      }))

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
        selectIngress,
        switchNetwork,
        setCustomEndpoint,
        availableNetworks: Object.values(NETWORKS),
        ingressNodes: INGRESS_NODES,
        refreshHealth: runHealthChecks,
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
