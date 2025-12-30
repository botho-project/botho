import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useRef,
  type ReactNode,
} from 'react'
import { LocalNodeAdapter, RemoteNodeAdapter } from '@botho/adapters'
import type { NodeAdapter } from '@botho/adapters'
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
  adapter: NodeAdapter | null
}

const ConnectionContext = createContext<ConnectionContextValue | null>(null)

const localScanAdapter = new LocalNodeAdapter()

// Remote seed nodes to try when no local nodes are found
const SEED_NODES = ['https://seed.botho.io']

export function ConnectionProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ConnectionState>({
    isScanning: false,
    discoveredNodes: [],
    connectedNode: null,
    error: null,
  })
  const adapterRef = useRef<NodeAdapter | null>(null)

  const scanForNodes = useCallback(async () => {
    setState((s) => ({ ...s, isScanning: true, error: null }))

    try {
      // First, scan for local nodes
      const localNodes = await localScanAdapter.scanForNodes()

      // If no local nodes, try remote seed nodes
      if (localNodes.length === 0) {
        const remoteNodes: NodeInfo[] = []

        for (const seedUrl of SEED_NODES) {
          try {
            const remoteAdapter = new RemoteNodeAdapter({ seedNodes: [seedUrl] })
            await remoteAdapter.connect()
            const nodeInfo = remoteAdapter.getNodeInfo()
            if (nodeInfo) {
              // Mark as remote node
              remoteNodes.push({
                ...nodeInfo,
                id: seedUrl,
                host: new URL(seedUrl).hostname,
                port: 443,
              })
            }
            remoteAdapter.disconnect()
          } catch {
            // Seed node not reachable, continue
          }
        }

        setState((s) => ({
          ...s,
          isScanning: false,
          discoveredNodes: remoteNodes,
        }))
      } else {
        setState((s) => ({
          ...s,
          isScanning: false,
          discoveredNodes: localNodes,
        }))
      }
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

      let nodeAdapter: NodeAdapter

      // Check if this is a remote node (port 443 or hostname is a seed)
      const isRemote = node.port === 443 || SEED_NODES.some((s) => s.includes(node.host))

      if (isRemote) {
        // Use RemoteNodeAdapter for seed nodes
        const seedUrl = `https://${node.host}`
        nodeAdapter = new RemoteNodeAdapter({ seedNodes: [seedUrl] })
      } else {
        // Use LocalNodeAdapter for local nodes
        nodeAdapter = new LocalNodeAdapter({
          host: node.host,
          port: node.port,
        })
      }

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
      // Determine if custom node is remote (port 443) or local
      const isRemote = port === 443

      let nodeAdapter: NodeAdapter
      if (isRemote) {
        nodeAdapter = new RemoteNodeAdapter({ seedNodes: [`https://${host}`] })
      } else {
        nodeAdapter = new LocalNodeAdapter({ host, port })
      }

      await nodeAdapter.connect()
      const node = nodeAdapter.getNodeInfo()

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
          const isRemote = node.port === 443 || SEED_NODES.some((s) => s.includes(node.host))

          let nodeAdapter: NodeAdapter
          if (isRemote) {
            nodeAdapter = new RemoteNodeAdapter({ seedNodes: [`https://${node.host}`] })
          } else {
            nodeAdapter = new LocalNodeAdapter({
              host: node.host,
              port: node.port,
            })
          }

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
