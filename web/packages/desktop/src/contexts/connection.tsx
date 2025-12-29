import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useRef,
  type ReactNode,
} from 'react'
import { LocalNodeAdapter } from '@botho/adapters'
import type { NodeInfo } from '@botho/core'

interface ConnectionState {
  isScanning: boolean
  discoveredNodes: NodeInfo[]
  connectedNode: NodeInfo | null
  error: string | null
}

interface ConnectionContextValue extends ConnectionState {
  scanForNodes: () => Promise<void>
  connectToNode: (node: NodeInfo) => Promise<void>
  disconnect: () => void
  addCustomNode: (host: string, port: number) => Promise<void>
  /** The connected adapter for making API calls */
  adapter: LocalNodeAdapter | null
}

const ConnectionContext = createContext<ConnectionContextValue | null>(null)

const scanAdapter = new LocalNodeAdapter()

export function ConnectionProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ConnectionState>({
    isScanning: false,
    discoveredNodes: [],
    connectedNode: null,
    error: null,
  })
  const adapterRef = useRef<LocalNodeAdapter | null>(null)

  const scanForNodes = useCallback(async () => {
    setState((s) => ({ ...s, isScanning: true, error: null }))

    try {
      const nodes = await scanAdapter.scanForNodes()
      setState((s) => ({
        ...s,
        isScanning: false,
        discoveredNodes: nodes,
      }))
    } catch (err) {
      setState((s) => ({
        ...s,
        isScanning: false,
        error: err instanceof Error ? err.message : 'Scan failed',
      }))
    }
  }, [])

  const connectToNode = useCallback(async (node: NodeInfo) => {
    setState((s) => ({
      ...s,
      error: null,
      connectedNode: { ...node, status: 'connecting' },
    }))

    try {
      // Disconnect existing adapter if any
      if (adapterRef.current) {
        adapterRef.current.disconnect()
      }

      // Create a new adapter for this specific node
      const nodeAdapter = new LocalNodeAdapter({
        host: node.host,
        port: node.port,
      })
      await nodeAdapter.connect()

      adapterRef.current = nodeAdapter
      const connectedNode = nodeAdapter.getNodeInfo()
      setState((s) => ({
        ...s,
        connectedNode,
      }))

      // Store last connected node
      localStorage.setItem('botho-last-node', JSON.stringify(connectedNode))
    } catch (err) {
      adapterRef.current = null
      setState((s) => ({
        ...s,
        connectedNode: null,
        error: err instanceof Error ? err.message : 'Connection failed',
      }))
    }
  }, [])

  const disconnect = useCallback(() => {
    if (adapterRef.current) {
      adapterRef.current.disconnect()
      adapterRef.current = null
    }
    setState((s) => ({
      ...s,
      connectedNode: null,
    }))
    localStorage.removeItem('botho-last-node')
  }, [])

  const addCustomNode = useCallback(async (host: string, port: number) => {
    setState((s) => ({ ...s, isScanning: true, error: null }))

    try {
      const customAdapter = new LocalNodeAdapter({ host, port })
      await customAdapter.connect()
      const node = customAdapter.getNodeInfo()

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
    } catch (err) {
      setState((s) => ({
        ...s,
        isScanning: false,
        error: err instanceof Error ? err.message : 'Connection failed',
      }))
    }
  }, [])

  // On mount, try to reconnect to last node or scan
  useEffect(() => {
    const init = async () => {
      const lastNode = localStorage.getItem('botho-last-node')
      if (lastNode) {
        try {
          const node = JSON.parse(lastNode) as NodeInfo
          const nodeAdapter = new LocalNodeAdapter({
            host: node.host,
            port: node.port,
          })
          await nodeAdapter.connect()
          const connectedNode = nodeAdapter.getNodeInfo()
          if (connectedNode) {
            adapterRef.current = nodeAdapter
            setState((s) => ({ ...s, connectedNode }))
            return
          }
        } catch {
          // Invalid stored data or node not reachable, ignore
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
        adapter: adapterRef.current,
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
