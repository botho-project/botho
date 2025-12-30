import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useRef,
  type ReactNode,
} from 'react'
import { RemoteNodeAdapter } from '@botho/adapters'
import { AddressBook, saveWallet, loadWallet, getWalletInfo, deriveAddress, isValidMnemonic, clearWallet } from '@botho/core'
import type { Balance, Contact, NodeInfo, Transaction } from '@botho/core'

interface WalletState {
  // Connection
  isConnected: boolean
  isConnecting: boolean
  nodeInfo: NodeInfo | null
  connectionError: string | null

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
    isEncrypted: false,
    isLocked: false,
    address: null,
    balance: null,
    transactions: [],
    contacts: [],
  })

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
    adapter.disconnect()
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
 * Returns the RemoteNodeAdapter instance
 */
export function useAdapter() {
  return adapter
}
