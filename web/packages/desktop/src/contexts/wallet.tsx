import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  type ReactNode,
} from 'react'
import { LocalNodeAdapter } from '@botho/adapters'
import type { Balance, Transaction, Address } from '@botho/core'
import { useConnection } from './connection'

interface WalletState {
  address: Address | null
  balance: Balance | null
  transactions: Transaction[]
  isLoading: boolean
  isSending: boolean
  error: string | null
}

interface SendTxParams {
  recipient: Address
  amount: bigint
  privacyLevel: 'standard' | 'private'
  memo?: string
}

interface WalletContextValue extends WalletState {
  refreshBalance: () => Promise<void>
  refreshTransactions: () => Promise<void>
  sendTransaction: (params: SendTxParams) => Promise<{ success: boolean; txHash?: string; error?: string }>
  estimateFee: (amount: bigint, privacyLevel: 'standard' | 'private') => Promise<bigint>
  setAddress: (address: Address) => void
}

const WalletContext = createContext<WalletContextValue | null>(null)

export function WalletProvider({ children }: { children: ReactNode }) {
  const { connectedNode } = useConnection()
  const [adapter, setAdapter] = useState<LocalNodeAdapter | null>(null)

  const [state, setState] = useState<WalletState>({
    address: null,
    balance: null,
    transactions: [],
    isLoading: false,
    isSending: false,
    error: null,
  })

  // Create adapter when connected
  useEffect(() => {
    if (connectedNode) {
      const newAdapter = new LocalNodeAdapter({
        host: connectedNode.host,
        port: connectedNode.port,
      })
      newAdapter.connect().then(() => {
        setAdapter(newAdapter)
      }).catch(console.error)
    } else {
      setAdapter(null)
    }
  }, [connectedNode])

  const setAddress = useCallback((address: Address) => {
    setState(s => ({ ...s, address }))
    localStorage.setItem('botho-wallet-address', address)
  }, [])

  // Load saved address on mount
  useEffect(() => {
    const saved = localStorage.getItem('botho-wallet-address')
    if (saved) {
      setState(s => ({ ...s, address: saved }))
    }
  }, [])

  const refreshBalance = useCallback(async () => {
    if (!adapter || !state.address) return

    setState(s => ({ ...s, isLoading: true, error: null }))
    try {
      const balance = await adapter.getBalance([state.address])
      setState(s => ({ ...s, balance, isLoading: false }))
    } catch (err) {
      setState(s => ({
        ...s,
        isLoading: false,
        error: err instanceof Error ? err.message : 'Failed to fetch balance',
      }))
    }
  }, [adapter, state.address])

  const refreshTransactions = useCallback(async () => {
    if (!adapter || !state.address) return

    setState(s => ({ ...s, isLoading: true, error: null }))
    try {
      const transactions = await adapter.getTransactionHistory([state.address], { limit: 50 })
      setState(s => ({ ...s, transactions, isLoading: false }))
    } catch (err) {
      setState(s => ({
        ...s,
        isLoading: false,
        error: err instanceof Error ? err.message : 'Failed to fetch transactions',
      }))
    }
  }, [adapter, state.address])

  const estimateFee = useCallback(async (_amount: bigint, privacyLevel: 'standard' | 'private'): Promise<bigint> => {
    if (!adapter) return BigInt(0)

    // Estimate transaction size based on privacy level
    // Standard: ML-DSA signature (~3.4 KB per input)
    // Private: LION ring signature (~17.5 KB per input)
    const sizeBytes = privacyLevel === 'private' ? 22000 : 4000
    return adapter.estimateFee(sizeBytes)
  }, [adapter])

  const sendTransaction = useCallback(async (_params: SendTxParams) => {
    if (!adapter) {
      return { success: false, error: 'Not connected to node' }
    }

    setState(s => ({ ...s, isSending: true, error: null }))

    try {
      // In a real implementation, this would:
      // 1. Build the transaction locally using _params
      // 2. Sign it with the wallet's private key
      // 3. Submit via adapter.submitTransaction()

      // For now, return a mock success to demonstrate the UI
      await new Promise(resolve => setTimeout(resolve, 1500)) // Simulate network delay

      const mockTxHash = `tx_${Date.now().toString(16)}_${Math.random().toString(16).slice(2, 10)}`

      setState(s => ({ ...s, isSending: false }))

      // Refresh balance and transactions after send
      await refreshBalance()
      await refreshTransactions()

      return { success: true, txHash: mockTxHash }
    } catch (err) {
      const error = err instanceof Error ? err.message : 'Transaction failed'
      setState(s => ({ ...s, isSending: false, error }))
      return { success: false, error }
    }
  }, [adapter, refreshBalance, refreshTransactions])

  // Auto-refresh when address changes
  useEffect(() => {
    if (state.address && adapter) {
      refreshBalance()
      refreshTransactions()
    }
  }, [state.address, adapter, refreshBalance, refreshTransactions])

  // Subscribe to transaction updates
  useEffect(() => {
    if (!adapter || !state.address) return

    const unsubscribe = adapter.onTransaction([state.address], () => {
      refreshBalance()
      refreshTransactions()
    })

    return unsubscribe
  }, [adapter, state.address, refreshBalance, refreshTransactions])

  return (
    <WalletContext.Provider
      value={{
        ...state,
        refreshBalance,
        refreshTransactions,
        sendTransaction,
        estimateFee,
        setAddress,
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
