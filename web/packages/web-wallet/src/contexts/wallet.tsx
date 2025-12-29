import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  type ReactNode,
} from 'react'
import { RemoteNodeAdapter } from '@botho/adapters'
import { AddressBook } from '@botho/core'
import type { Balance, Contact, NodeInfo, Transaction } from '@botho/core'

interface WalletState {
  // Connection
  isConnected: boolean
  isConnecting: boolean
  nodeInfo: NodeInfo | null
  connectionError: string | null

  // Wallet
  hasWallet: boolean
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
  createWallet: () => Promise<void>
  importWallet: (seedPhrase: string) => Promise<void>
  exportWallet: () => string | null

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

const adapter = new RemoteNodeAdapter({
  seedNodes: ['https://seed.botho.io', 'https://seed2.botho.io'],
  networkId: 'botho-mainnet',
})

const addressBook = new AddressBook()

export function WalletProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<WalletState>({
    isConnected: false,
    isConnecting: false,
    nodeInfo: null,
    connectionError: null,
    hasWallet: false,
    address: null,
    balance: null,
    transactions: [],
    contacts: [],
  })

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

  const connect = useCallback(async () => {
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
      const storedAddress = localStorage.getItem('botho-wallet-address')
      if (storedAddress) {
        setState(s => ({ ...s, hasWallet: true, address: storedAddress }))
        // Fetch balance and transactions
        const [balance, transactions] = await Promise.all([
          adapter.getBalance([storedAddress]),
          adapter.getTransactionHistory([storedAddress], { limit: 50 }),
        ])
        setState(s => ({ ...s, balance, transactions }))
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
    adapter.disconnect()
    setState(s => ({
      ...s,
      isConnected: false,
      nodeInfo: null,
    }))
  }, [])

  const createWallet = useCallback(async () => {
    // TODO: Implement actual wallet creation with crypto
    // For now, generate a placeholder address
    const mockAddress = 'bth1' + Array.from(crypto.getRandomValues(new Uint8Array(32)))
      .map(b => b.toString(16).padStart(2, '0'))
      .join('')
      .slice(0, 40)

    localStorage.setItem('botho-wallet-address', mockAddress)
    setState(s => ({
      ...s,
      hasWallet: true,
      address: mockAddress,
      balance: { available: 0n, pending: 0n, total: 0n },
      transactions: [],
    }))
  }, [])

  const importWallet = useCallback(async (seedPhrase: string) => {
    // TODO: Implement actual wallet import
    // Validate seed phrase and derive address
    if (seedPhrase.split(' ').length !== 24) {
      throw new Error('Invalid seed phrase. Expected 24 words.')
    }

    const mockAddress = 'bth1' + Array.from(crypto.getRandomValues(new Uint8Array(32)))
      .map(b => b.toString(16).padStart(2, '0'))
      .join('')
      .slice(0, 40)

    localStorage.setItem('botho-wallet-address', mockAddress)
    localStorage.setItem('botho-wallet-seed', seedPhrase) // TODO: Encrypt this!

    setState(s => ({
      ...s,
      hasWallet: true,
      address: mockAddress,
    }))

    // Fetch balance
    if (adapter.isConnected()) {
      const balance = await adapter.getBalance([mockAddress])
      const transactions = await adapter.getTransactionHistory([mockAddress], { limit: 50 })
      setState(s => ({ ...s, balance, transactions }))
    }
  }, [])

  const exportWallet = useCallback(() => {
    // TODO: Implement actual seed phrase export
    return localStorage.getItem('botho-wallet-seed')
  }, [])

  const send = useCallback(async (to: string, amount: bigint, memo?: string): Promise<string> => {
    // TODO: Implement actual transaction signing and submission
    console.log('Sending', { to, amount, memo })
    throw new Error('Transaction signing not yet implemented')
  }, [])

  const refreshBalance = useCallback(async () => {
    if (!state.address || !adapter.isConnected()) return
    const balance = await adapter.getBalance([state.address])
    setState(s => ({ ...s, balance }))
  }, [state.address])

  const refreshTransactions = useCallback(async () => {
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
        exportWallet,
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
