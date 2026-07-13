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
  validateRpcEndpointForNetwork,
  fetchNodeHealth,
  saveCustomNodeFromLink,
  loadCustomNodeFromLink,
  clearCustomNodeFromLink,
} from '../config/networks'
import {
  parseRpcDeepLink,
  rpcLinkHost,
  classifyRpcHost,
  type RpcHostTrust,
} from '../lib/custom-rpc-link'

/**
 * A `?rpc=` deep link that has been parsed and validated but NOT yet applied —
 * it is awaiting an explicit user trust decision (#587). Surfacing it as
 * "pending" instead of applying it is the whole point of the guardrail: a link
 * must never silently switch the active node.
 */
export interface PendingRpcLink {
  /** The validated https RPC endpoint the link wants to use. */
  rpcUrl: string
  /** Bare host shown to the user ("point your wallet at <host>"). */
  host: string
  /** Whether the host is a known Botho-operated host or an unknown third party. */
  trust: RpcHostTrust
}

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
  /**
   * A parsed-but-not-yet-applied `?rpc=` deep link awaiting the user's trust
   * decision, or `null` when there is none (#587).
   */
  pendingRpcLink: PendingRpcLink | null
  /**
   * Host of the custom node the user is currently connected to *because they
   * accepted a deep link*, or `null` when on a built-in / manually-set node.
   * Drives the persistent "from a link" banner.
   */
  customNodeFromLink: string | null
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
  /**
   * Begin polling node health. Returns an unsubscribe fn; polling stops once
   * the last subscriber unsubscribes. Call from components that show health
   * (the NetworkSelector) so the landing page makes no node calls.
   */
  startHealthPolling: () => () => void
  /**
   * Apply the pending `?rpc=` deep link as the custom ingress (#587). Validates
   * reachability first; on success persists the "from a link" marker so the
   * banner appears. No-op when there is no pending link. Resolves to whether the
   * node was applied.
   */
  acceptPendingRpcLink: () => Promise<boolean>
  /** Dismiss the pending deep link WITHOUT switching nodes — the prior node is left intact (#587). */
  declinePendingRpcLink: () => void
  /** Revert a link-supplied custom node back to the default ingress with one tap (#587). */
  revertCustomNode: () => void
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
    // Only honour a persisted "from a link" marker if we are actually still on a
    // custom node; otherwise a stale marker would mislabel a built-in ingress.
    const customNodeFromLink = ingressId === 'custom' ? loadCustomNodeFromLink() : null
    return {
      network,
      ingressId,
      isValidating: false,
      validationError: null,
      hasFaucet: !!network.faucetEndpoint,
      nodeHealth,
      pendingRpcLink: null,
      customNodeFromLink,
    }
  })

  // Notify listeners when network changes
  useEffect(() => {
    // Dispatch custom event for wallet context to pick up
    window.dispatchEvent(new CustomEvent('network-changed', {
      detail: { network: state.network }
    }))
  }, [state.network])

  // Mirror the pending deep link into a ref so `acceptPendingRpcLink` always sees
  // the latest value without re-creating the (stable) setCustomEndpoint closure.
  const pendingRpcLinkRef = useRef<PendingRpcLink | null>(null)
  useEffect(() => {
    pendingRpcLinkRef.current = state.pendingRpcLink
  }, [state.pendingRpcLink])

  // Poll each ingress node's health via node_getStatus.
  //
  // Polling is started on demand (see `startHealthPolling`) rather than on
  // mount: the landing page does not render the node picker and must not reach
  // out to the SCP nodes (an unreachable node would otherwise emit a console
  // network error on every page). The NetworkSelector starts polling while it
  // is mounted (wallet/explorer pages) and stops on unmount.
  const pollTimerRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const pollSubscribers = useRef(0)

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

  const startHealthPolling = useCallback((): (() => void) => {
    pollSubscribers.current += 1
    if (pollSubscribers.current === 1) {
      runHealthChecks()
      pollTimerRef.current = setInterval(runHealthChecks, HEALTH_POLL_INTERVAL)
    }
    return () => {
      pollSubscribers.current = Math.max(0, pollSubscribers.current - 1)
      if (pollSubscribers.current === 0 && pollTimerRef.current) {
        clearInterval(pollTimerRef.current)
        pollTimerRef.current = null
      }
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
      // Switching to a built-in node clears any "from a link" status.
      customNodeFromLink: null,
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
      // Validate the endpoint: https shape, reachability, and network match.
      // Shared by the manual picker path and the accepted `?rpc=` deep link so
      // both enforce the same guardrails before persisting (#587).
      const validation = await validateRpcEndpointForNetwork(endpoint)

      if (!validation.ok) {
        setState(s => ({
          ...s,
          isValidating: false,
          validationError: validation.error,
        }))
        return false
      }

      const network = createCustomNetwork(endpoint)
      saveSelectedNetwork('custom', endpoint)
      // A directly-set custom endpoint is NOT a link-supplied node; drop any
      // stale "from a link" marker. acceptPendingRpcLink re-sets it afterward
      // for the deep-link path.
      clearCustomNodeFromLink()

      setState(s => ({
        ...s,
        network,
        ingressId: 'custom',
        isValidating: false,
        validationError: null,
        hasFaucet: !!network.faucetEndpoint,
        customNodeFromLink: null,
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

  // Custom-RPC deep link (P6.3, #458 §3 step 5) behind a trust gate (#587).
  //
  // SECURITY: a `?rpc=` link is attacker-controllable and must NEVER silently
  // switch the active node. So instead of applying it, we surface the parsed
  // link as a PENDING trust prompt (`pendingRpcLink`); the user must explicitly
  // accept it (`acceptPendingRpcLink`) before it touches the active node, and a
  // decline leaves the prior node intact. The param is stripped from the URL
  // immediately so a refresh/back/share doesn't silently re-arm the prompt with
  // a stale link.
  const deepLinkSeen = useRef(false)
  useEffect(() => {
    if (deepLinkSeen.current) return
    if (typeof window === 'undefined') return
    const parsed = parseRpcDeepLink(window.location.search)
    if (parsed.ok !== true) return
    deepLinkSeen.current = true
    const host = rpcLinkHost(parsed.rpcUrl)
    try {
      const url = new URL(window.location.href)
      url.searchParams.delete('rpc')
      window.history.replaceState(null, '', url.toString())
    } catch {
      // history API unavailable (non-browser test env) — ignore.
    }
    if (!host) return
    // Stash the parsed link for the trust gate; do NOT apply it here.
    setState((s) => ({
      ...s,
      pendingRpcLink: { rpcUrl: parsed.rpcUrl, host, trust: classifyRpcHost(parsed.rpcUrl) },
    }))
  }, [])

  const acceptPendingRpcLink = useCallback(async (): Promise<boolean> => {
    const pending = pendingRpcLinkRef.current
    if (!pending) return false
    const ok = await setCustomEndpoint(pending.rpcUrl)
    if (ok) {
      // Only mark "from a link" once the node actually validated and committed.
      saveCustomNodeFromLink(pending.host)
      setState((s) => ({ ...s, pendingRpcLink: null, customNodeFromLink: pending.host }))
    } else {
      // Validation failed (unreachable node): clear the prompt but stay on the
      // prior node. The validationError set by setCustomEndpoint is surfaced.
      setState((s) => ({ ...s, pendingRpcLink: null }))
    }
    return ok
  }, [setCustomEndpoint])

  const declinePendingRpcLink = useCallback(() => {
    // Decline = keep the current node. We touch nothing but the prompt itself.
    setState((s) => ({ ...s, pendingRpcLink: null }))
  }, [])

  const revertCustomNode = useCallback(() => {
    clearCustomNodeFromLink()
    selectIngress(DEFAULT_INGRESS_ID)
  }, [selectIngress])

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
        startHealthPolling,
        acceptPendingRpcLink,
        declinePendingRpcLink,
        revertCustomNode,
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
