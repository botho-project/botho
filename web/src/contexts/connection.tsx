import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  type ReactNode,
} from 'react'

export interface CadenceNode {
  id: string
  host: string
  port: number
  version?: string
  blockHeight?: number
  networkId?: string
  latency?: number
  status: 'online' | 'connecting' | 'error'
}

interface ConnectionState {
  isScanning: boolean
  discoveredNodes: CadenceNode[]
  connectedNode: CadenceNode | null
  error: string | null
}

interface ConnectionContextValue extends ConnectionState {
  scanForNodes: () => Promise<void>
  connectToNode: (node: CadenceNode) => Promise<void>
  disconnect: () => void
  addCustomNode: (host: string, port: number) => Promise<void>
}

const ConnectionContext = createContext<ConnectionContextValue | null>(null)

// Common ports where Cadence nodes might be running
const DEFAULT_PORTS = [8080, 8081, 8082, 8083, 8084, 3000, 3001, 9090, 9091]
const SCAN_TIMEOUT = 2000

async function probeNode(host: string, port: number): Promise<CadenceNode | null> {
  const controller = new AbortController()
  const timeoutId = setTimeout(() => controller.abort(), SCAN_TIMEOUT)

  try {
    const startTime = performance.now()
    const response = await fetch(`http://${host}:${port}/api/status`, {
      signal: controller.signal,
      headers: { Accept: 'application/json' },
    })

    if (!response.ok) {
      return null
    }

    const data = await response.json()
    const latency = Math.round(performance.now() - startTime)

    return {
      id: `${host}:${port}`,
      host,
      port,
      version: data.version || 'unknown',
      blockHeight: data.blockHeight || data.block_height,
      networkId: data.networkId || data.network_id || 'cadence-mainnet',
      latency,
      status: 'online',
    }
  } catch {
    return null
  } finally {
    clearTimeout(timeoutId)
  }
}

export function ConnectionProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ConnectionState>({
    isScanning: false,
    discoveredNodes: [],
    connectedNode: null,
    error: null,
  })

  const scanForNodes = useCallback(async () => {
    setState((s) => ({ ...s, isScanning: true, error: null }))

    const hosts = ['localhost', '127.0.0.1']
    const probePromises: Promise<CadenceNode | null>[] = []

    for (const host of hosts) {
      for (const port of DEFAULT_PORTS) {
        probePromises.push(probeNode(host, port))
      }
    }

    const results = await Promise.all(probePromises)
    const nodes = results.filter((n): n is CadenceNode => n !== null)

    // Deduplicate by port (localhost and 127.0.0.1 are the same)
    const uniqueNodes = nodes.reduce<CadenceNode[]>((acc, node) => {
      if (!acc.some((n) => n.port === node.port)) {
        acc.push(node)
      }
      return acc
    }, [])

    setState((s) => ({
      ...s,
      isScanning: false,
      discoveredNodes: uniqueNodes,
    }))
  }, [])

  const connectToNode = useCallback(async (node: CadenceNode) => {
    setState((s) => ({
      ...s,
      error: null,
      connectedNode: { ...node, status: 'connecting' },
    }))

    try {
      // Verify the node is still reachable
      const verified = await probeNode(node.host, node.port)
      if (!verified) {
        throw new Error('Node is no longer reachable')
      }

      setState((s) => ({
        ...s,
        connectedNode: verified,
      }))

      // Store last connected node
      localStorage.setItem('cadence-last-node', JSON.stringify(verified))
    } catch (err) {
      setState((s) => ({
        ...s,
        connectedNode: null,
        error: err instanceof Error ? err.message : 'Connection failed',
      }))
    }
  }, [])

  const disconnect = useCallback(() => {
    setState((s) => ({
      ...s,
      connectedNode: null,
    }))
    localStorage.removeItem('cadence-last-node')
  }, [])

  const addCustomNode = useCallback(async (host: string, port: number) => {
    setState((s) => ({ ...s, isScanning: true, error: null }))

    const node = await probeNode(host, port)

    if (node) {
      setState((s) => ({
        ...s,
        isScanning: false,
        discoveredNodes: [...s.discoveredNodes.filter((n) => n.id !== node.id), node],
      }))
    } else {
      setState((s) => ({
        ...s,
        isScanning: false,
        error: `Could not connect to ${host}:${port}`,
      }))
    }
  }, [])

  // On mount, try to reconnect to last node or scan
  useEffect(() => {
    const init = async () => {
      const lastNode = localStorage.getItem('cadence-last-node')
      if (lastNode) {
        try {
          const node = JSON.parse(lastNode) as CadenceNode
          const verified = await probeNode(node.host, node.port)
          if (verified) {
            setState((s) => ({ ...s, connectedNode: verified }))
            return
          }
        } catch {
          // Invalid stored data, ignore
        }
      }
      // No stored node or it's not reachable, scan for nodes
      await scanForNodes()
    }
    init()
  }, [scanForNodes])

  return (
    <ConnectionContext.Provider
      value={{
        ...state,
        scanForNodes,
        connectToNode,
        disconnect,
        addCustomNode,
      }}
    >
      {children}
    </ConnectionContext.Provider>
  )
}

export function useConnection() {
  const context = useContext(ConnectionContext)
  if (!context) {
    throw new Error('useConnection must be used within a ConnectionProvider')
  }
  return context
}
