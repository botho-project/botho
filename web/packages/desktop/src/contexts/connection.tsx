import { createContext, useContext, useEffect, useState, useCallback, useRef, type ReactNode } from 'react'
import { LocalNodeAdapter, RemoteNodeAdapter } from '@botho/adapters'
import type { NodeAdapter } from '@botho/adapters'
import type { NodeInfo } from '@botho/core'
import { walletNetwork, type WalletNetwork } from '../config/wallet-network'

interface ConnectionState {
  isScanning: boolean
  discoveredNodes: NodeInfo[]
  connectedNode: NodeInfo | null
  /** Exact URL used by this adapter, including scheme and RPC path. */
  endpoint: string | null
  error: string | null
}
interface ConnectionContextValue extends ConnectionState {
  scanForNodes: () => Promise<void>
  connectToNode: (node: NodeInfo) => Promise<void>
  disconnect: () => void
  addCustomNode: (host: string, port: number, network: WalletNetwork) => Promise<void>
  adapter: NodeAdapter | null
}
const ConnectionContext = createContext<ConnectionContextValue | null>(null)
const SEED_NODES = ['https://seed.botho.io/rpc']

function explicitEndpoint(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Select an explicit RPC endpoint')
  const url = new URL(value)
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash) {
    throw new Error('RPC endpoint must be HTTP(S) without credentials or fragment')
  }
  return url.href
}

export function ConnectionProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ConnectionState>({ isScanning: false, discoveredNodes: [], connectedNode: null, endpoint: null, error: null })
  const adapterRef = useRef<NodeAdapter | null>(null)
  const generation = useRef(0)
  const mounted = useRef(false)
  const probes = useRef(new Set<NodeAdapter>())

  const scanForNodes = useCallback(async () => {
    const token = generation.current
    setState(s => ({ ...s, isScanning: true, error: null }))
    try {
      const local = new LocalNodeAdapter()
      const nodes = await local.scanForNodes()
      // Local scanning explicitly uses http://host:port/rpc. Store that exact route as ID.
      const found = nodes.map(node => ({ ...node, id: new URL('/rpc', `http://${node.host.includes(':') ? `[${node.host}]` : node.host}:${node.port}`).href }))
      if (!found.length) {
        for (const endpoint of SEED_NODES) {
          const probe = new RemoteNodeAdapter({ seedNodes: [endpoint], networkId: 'botho-testnet', useWebSocket: false })
          probes.current.add(probe)
          try {
            await probe.connect()
            const info = probe.getNodeInfo()
            if (info) found.push({ ...info, id: explicitEndpoint(endpoint) })
          } catch { /* An unavailable candidate is not a selected connection. */ }
          finally { probe.disconnect(); probes.current.delete(probe) }
        }
      }
      if (mounted.current && token === generation.current) setState(s => ({ ...s, isScanning: false, discoveredNodes: found }))
    } catch (error) {
      if (mounted.current && token === generation.current) setState(s => ({ ...s, isScanning: false, error: String(error) }))
    }
  }, [])

  const connectToNode = useCallback(async (node: NodeInfo) => {
    const token = ++generation.current
    adapterRef.current?.disconnect()
    adapterRef.current = null
    setState(s => ({ ...s, connectedNode: null, endpoint: null, error: null }))
    let candidate: RemoteNodeAdapter | null = null
    try {
      // Discovery/custom entry supplies an explicit URL. Never reconstruct a selected URL from a hostname.
      const endpoint = explicitEndpoint(node.id)
      const network = walletNetwork(node.networkId)
      if (!network) throw new Error('Select a recognized network before connecting')
      candidate = new RemoteNodeAdapter({ seedNodes: [endpoint], networkId: network })
      probes.current.add(candidate)
      await candidate.connect()
      if (!mounted.current || token !== generation.current) { candidate.disconnect(); return }
      const info = candidate.getNodeInfo()
      if (!info || info.networkId !== network) throw new Error('Node network does not match the selected network')
      const connectedNode = { ...info, id: endpoint }
      adapterRef.current = candidate
      setState(s => ({ ...s, connectedNode, endpoint }))
      localStorage.setItem('botho-last-node', JSON.stringify({ ...connectedNode, endpoint }))
    } catch (error) {
      candidate?.disconnect()
      if (mounted.current && token === generation.current) setState(s => ({ ...s, connectedNode: null, endpoint: null, error: String(error) }))
    } finally { if (candidate) probes.current.delete(candidate) }
  }, [])

  const disconnect = useCallback(() => {
    generation.current += 1
    adapterRef.current?.disconnect()
    adapterRef.current = null
    for (const candidate of probes.current) candidate.disconnect()
    setState(s => ({ ...s, connectedNode: null, endpoint: null, isScanning: false }))
    localStorage.removeItem('botho-last-node')
  }, [])

  const addCustomNode = useCallback(async (host: string, port: number, network: WalletNetwork) => {
    const token = generation.current
    setState(s => ({ ...s, isScanning: true, error: null }))
    let probe: RemoteNodeAdapter | null = null
    try {
      // A full URL explicitly selects HTTPS/path; bare host+port is the local HTTP entry contract.
      const endpoint = explicitEndpoint(host.includes('://') ? host : `http://${host.includes(':') ? `[${host}]` : host}:${port}/rpc`)
      if (!walletNetwork(network)) throw new Error('Select mainnet or testnet explicitly')
      probe = new RemoteNodeAdapter({ seedNodes: [endpoint], networkId: network, useWebSocket: false })
      probes.current.add(probe)
      await probe.connect()
      const info = probe.getNodeInfo()
      if (!info || info.networkId !== network) throw new Error('Node network does not match the selected network')
      if (mounted.current && token === generation.current) setState(s => ({ ...s, isScanning: false,
        discoveredNodes: [...s.discoveredNodes.filter(n => n.id !== endpoint), { ...info, id: endpoint }] }))
    } catch (error) {
      if (mounted.current && token === generation.current) setState(s => ({ ...s, isScanning: false, error: String(error) }))
    } finally { if (probe) { probe.disconnect(); probes.current.delete(probe) } }
  }, [])

  useEffect(() => {
    mounted.current = true
    let saved: NodeInfo | null = null
    try {
      const value = JSON.parse(localStorage.getItem('botho-last-node') ?? 'null')
      // Older host-only cache entries are not authoritative endpoint selections.
      if (value?.endpoint && value.id === explicitEndpoint(value.endpoint)) saved = value
      else localStorage.removeItem('botho-last-node')
    } catch { localStorage.removeItem('botho-last-node') }
    if (saved) void connectToNode(saved)
    else void scanForNodes()
    return () => {
      mounted.current = false
      generation.current += 1
      adapterRef.current?.disconnect()
      adapterRef.current = null
      for (const probe of probes.current) probe.disconnect()
    }
  }, [connectToNode, scanForNodes])

  return <ConnectionContext.Provider value={{ ...state, adapter: adapterRef.current,
    scanForNodes, connectToNode, disconnect, addCustomNode }}>{children}</ConnectionContext.Provider>
}
export function useConnection() {
  const context = useContext(ConnectionContext)
  if (!context) throw new Error('useConnection must be used within a ConnectionProvider')
  return context
}
