import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useRef,
  type ReactNode,
} from 'react'
import { RemoteNodeAdapter, type WsConnectionStatus } from '@botho/adapters'
import { AddressBook, saveWallet, loadWallet, getWalletInfo, deriveAddress, isValidMnemonic, clearWallet } from '@botho/core'
import type { Balance, Contact, NodeInfo, Transaction } from '@botho/core'
import { type NetworkConfig, loadSelectedNetwork, NETWORKS, DEFAULT_NETWORK_ID, createCustomNetwork } from '../config/networks'

interface WalletState {
  // Connection
  isConnected: boolean
  isConnecting: boolean
  nodeInfo: NodeInfo | null
  connectionError: string | null

  // WebSocket status
  wsStatus: WsConnectionStatus

  // Wallet
  hasWallet: boolean
  isEncrypted: boolean
  isLocked: boolean
  address: string | null
  balance: Balance | null
  transactions: Transaction[]

  // Address book
  contacts: Contact[]
}

interface WalletContextValue extends WalletState {
  // Connection
  connect: () => Promise<void>
  disconnect: () => void

  // Wallet
  createWallet: (mnemonic: string, password?: string) => Promise<void>
  importWallet: (seedPhrase: string, password?: string) => Promise<void>
  unlockWallet: (password: string) => Promise<void>
  exportWallet: (password?: string) => Promise<string | null>
  resetWallet: () => void

  // Transactions
  send: (to: string, amount: bigint, memo?: string) => Promise<string>
  refreshBalance: () => Promise<void>
  refreshTransactions: () => Promise<void>

  // Address book
  addContact: (name: string, address: string, notes?: string) => Promise<Contact>
  updateContact: (id: string, updates: Partial<Pick<Contact, 'name' | 'address' | 'notes'>>) => Promise<Contact>
  deleteContact: (id: string) => Promise<void>
  getContactName: (address: string) => string
}

const WalletContext = createContext<WalletContextValue | null>(null)

const addressBook = new AddressBook()

/** Polling interval when WebSocket is disconnected (30 seconds) */
const FALLBACK_POLL_INTERVAL = 30000

/**
 * Create adapter from network configuration
 */
function createAdapterFromNetwork(network: NetworkConfig): RemoteNodeAdapter {
  return new RemoteNodeAdapter({
    seedNodes: [network.rpcEndpoint],
    networkId: network.networkId,
  })
}

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

export function WalletProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<WalletState>({
    isConnected: false,
    isConnecting: false,
    nodeInfo: null,
    connectionError: null,
    wsStatus: 'disconnected',
    hasWallet: false,
    isEncrypted: false,
    isLocked: false,
    address: null,
    balance: null,
    transactions: [],
    contacts: [],
  })

  // Store adapter in ref so we can recreate it when network changes
  const adapterRef = useRef<RemoteNodeAdapter>(createAdapterFromNetwork(getInitialNetwork()))

  // Store mnemonic in memory after unlock (cleared on page refresh)
  const mnemonicRef = useRef<string | null>(null)

  // Load address book on mount
  useEffect(() => {
    addressBook.load().then(() => {
      setState(s => ({ ...s, contacts: addressBook.getAll() }))
    })
  }, [])

  // Auto-connect on mount
  useEffect(() => {
    connect()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Listen for network changes from NetworkContext
  useEffect(() => {
    const handleNetworkChange = async (event: Event) => {
      const customEvent = event as CustomEvent<{ network: NetworkConfig }>
      const newNetwork = customEvent.detail.network

      // Disconnect from current network
      adapterRef.current.disconnect()

      // Create new adapter for new network
      adapterRef.current = createAdapterFromNetwork(newNetwork)

      // Reset connection state
      setState(s => ({
        ...s,
        isConnected: false,
        isConnecting: false,
        nodeInfo: null,
        connectionError: null,
        wsStatus: 'disconnected',
        balance: null,
        transactions: [],
      }))

      // Reconnect
      await connect()
    }

    window.addEventListener('network-changed', handleNetworkChange)
    return () => window.removeEventListener('network-changed', handleNetworkChange)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Subscribe to WebSocket status changes
  useEffect(() => {
    const adapter = adapterRef.current
    const unsubscribe = adapter.onWsStatusChange((wsStatus) => {
      setState(s => ({ ...s, wsStatus }))
    })
    // Initialize with current status
    setState(s => ({ ...s, wsStatus: adapter.getWsStatus() }))
    return unsubscribe
  }, [])

  // Subscribe to real-time block updates for balance refresh
  useEffect(() => {
    if (!state.isConnected || !state.address || state.isLocked) return

    const adapter = adapterRef.current
    const unsubscribe = adapter.onNewBlock(async () => {
      // Refresh balance and transactions when new block arrives
      try {
        const [balance, transactions] = await Promise.all([
          adapter.getBalance([state.address!]),
          adapter.getTransactionHistory([state.address!], { limit: 50 }),
        ])
        setState(s => ({ ...s, balance, transactions }))
      } catch {
        // Ignore refresh errors - will retry on next block
      }
    })

    return unsubscribe
  }, [state.isConnected, state.address, state.isLocked])

  // Fallback polling when WebSocket is disconnected
  useEffect(() => {
    // Only poll if connected to node but WebSocket is down
    if (!state.isConnected || !state.address || state.isLocked) return
    if (state.wsStatus === 'connected') return // Use WebSocket instead

    const adapter = adapterRef.current
    const pollInterval = setInterval(async () => {
      try {
        const [balance, transactions] = await Promise.all([
          adapter.getBalance([state.address!]),
          adapter.getTransactionHistory([state.address!], { limit: 50 }),
        ])
        setState(s => ({ ...s, balance, transactions }))
      } catch {
        // Ignore polling errors
      }
    }, FALLBACK_POLL_INTERVAL)

    return () => clearInterval(pollInterval)
  }, [state.isConnected, state.address, state.isLocked, state.wsStatus])

  const connect = useCallback(async () => {
    const adapter = adapterRef.current
    setState(s => ({ ...s, isConnecting: true, connectionError: null }))

    try {
      await adapter.connect()
      setState(s => ({
        ...s,
        isConnected: true,
        isConnecting: false,
        nodeInfo: adapter.getNodeInfo(),
      }))

      // Check for stored wallet
      const walletInfo = getWalletInfo()
      if (walletInfo.exists) {
        setState(s => ({
          ...s,
          hasWallet: true,
          isEncrypted: walletInfo.isEncrypted,
          isLocked: walletInfo.isEncrypted, // Encrypted wallets start locked
          address: walletInfo.address,
        }))

        // If not encrypted, load balance immediately
        if (!walletInfo.isEncrypted && walletInfo.address) {
          const [balance, transactions] = await Promise.all([
            adapter.getBalance([walletInfo.address]),
            adapter.getTransactionHistory([walletInfo.address], { limit: 50 }),
          ])
          setState(s => ({ ...s, balance, transactions }))
        }
      }
    } catch (err) {
      setState(s => ({
        ...s,
        isConnecting: false,
        connectionError: err instanceof Error ? err.message : 'Connection failed',
      }))
    }
  }, [])

  const disconnect = useCallback(() => {
    adapterRef.current.disconnect()
    setState(s => ({
      ...s,
      isConnected: false,
      nodeInfo: null,
    }))
  }, [])

  const createWallet = useCallback(async (mnemonic: string, password?: string) => {
    if (!isValidMnemonic(mnemonic)) {
      throw new Error('Invalid mnemonic provided')
    }

    // Save wallet (mnemonic + derived address) to localStorage
    await saveWallet(mnemonic, password)
    const address = deriveAddress(mnemonic)

    // Store mnemonic in memory
    mnemonicRef.current = mnemonic

    setState(s => ({
      ...s,
      hasWallet: true,
      isEncrypted: !!password,
      isLocked: false,
      address,
      balance: { available: 0n, pending: 0n, total: 0n },
      transactions: [],
    }))
  }, [])

  const importWallet = useCallback(async (seedPhrase: string, password?: string) => {
    // Normalize input: trim, lowercase, collapse whitespace
    const normalized = seedPhrase.trim().toLowerCase().replace(/\s+/g, ' ')

    // Validate mnemonic (supports 12 or 24 words)
    const wordCount = normalized.split(' ').length
    if (wordCount !== 12 && wordCount !== 24) {
      throw new Error('Invalid recovery phrase. Expected 12 or 24 words.')
    }

    if (!isValidMnemonic(normalized)) {
      throw new Error('Invalid recovery phrase. Please check your words and try again.')
    }

    // Save wallet using proper derivation
    await saveWallet(normalized, password)
    const address = deriveAddress(normalized)

    // Store mnemonic in memory
    mnemonicRef.current = normalized

    setState(s => ({
      ...s,
      hasWallet: true,
      isEncrypted: !!password,
      isLocked: false,
      address,
    }))

    // Fetch balance
    const adapter = adapterRef.current
    if (adapter.isConnected()) {
      const balance = await adapter.getBalance([address])
      const transactions = await adapter.getTransactionHistory([address], { limit: 50 })
      setState(s => ({ ...s, balance, transactions }))
    }
  }, [])

  const unlockWallet = useCallback(async (password: string) => {
    const stored = await loadWallet(password)
    if (!stored) {
      throw new Error('No wallet found')
    }

    // Store mnemonic in memory
    mnemonicRef.current = stored.mnemonic

    setState(s => ({ ...s, isLocked: false }))

    // Fetch balance now that we're unlocked
    const adapter = adapterRef.current
    if (adapter.isConnected() && stored.address) {
      const [balance, transactions] = await Promise.all([
        adapter.getBalance([stored.address]),
        adapter.getTransactionHistory([stored.address], { limit: 50 }),
      ])
      setState(s => ({ ...s, balance, transactions }))
    }
  }, [])

  const exportWallet = useCallback(async (password?: string) => {
    // If we have mnemonic in memory, use it
    if (mnemonicRef.current) {
      return mnemonicRef.current
    }

    // Otherwise try to load from storage
    const stored = await loadWallet(password)
    return stored?.mnemonic ?? null
  }, [])

  const resetWallet = useCallback(() => {
    // Clear stored wallet from localStorage
    clearWallet()
    // Clear mnemonic from memory
    mnemonicRef.current = null
    // Reset state to initial
    setState(s => ({
      ...s,
      hasWallet: false,
      isEncrypted: false,
      isLocked: false,
      address: null,
      balance: null,
      transactions: [],
    }))
  }, [])

  const send = useCallback(async (to: string, amount: bigint, memo?: string): Promise<string> => {
    // TODO: Implement actual transaction signing and submission
    console.log('Sending', { to, amount, memo })
    throw new Error('Transaction signing not yet implemented')
  }, [])

  const refreshBalance = useCallback(async () => {
    const adapter = adapterRef.current
    if (!state.address || !adapter.isConnected()) return
    const balance = await adapter.getBalance([state.address])
    setState(s => ({ ...s, balance }))
  }, [state.address])

  const refreshTransactions = useCallback(async () => {
    const adapter = adapterRef.current
    if (!state.address || !adapter.isConnected()) return
    const transactions = await adapter.getTransactionHistory([state.address], { limit: 50 })
    setState(s => ({ ...s, transactions }))
  }, [state.address])

  // Address book methods
  const addContact = useCallback(async (name: string, address: string, notes?: string) => {
    const contact = await addressBook.add(name, address, notes)
    setState(s => ({ ...s, contacts: addressBook.getAll() }))
    return contact
  }, [])

  const updateContact = useCallback(async (id: string, updates: Partial<Pick<Contact, 'name' | 'address' | 'notes'>>) => {
    const contact = await addressBook.update(id, updates)
    setState(s => ({ ...s, contacts: addressBook.getAll() }))
    return contact
  }, [])

  const deleteContact = useCallback(async (id: string) => {
    await addressBook.delete(id)
    setState(s => ({ ...s, contacts: addressBook.getAll() }))
  }, [])

  const getContactName = useCallback((address: string) => {
    return addressBook.getDisplayName(address)
  }, [])

  return (
    <WalletContext.Provider
      value={{
        ...state,
        connect,
        disconnect,
        createWallet,
        importWallet,
        unlockWallet,
        exportWallet,
        resetWallet,
        send,
        refreshBalance,
        refreshTransactions,
        addContact,
        updateContact,
        deleteContact,
        getContactName,
      }}
    >
      {children}
    </WalletContext.Provider>
  )
}

export function useWallet() {
  const context = useContext(WalletContext)
  if (!context) {
    throw new Error('useWallet must be used within a WalletProvider')
  }
  return context
}

/**
 * Get the adapter for use with explorer/blockchain queries
 * Returns a function that retrieves the current adapter instance
 * Note: This returns a new adapter each time if network has changed
 */
export function useAdapter() {
  const adapterRef = useRef<RemoteNodeAdapter>(createAdapterFromNetwork(getInitialNetwork()))
  return adapterRef.current
}
