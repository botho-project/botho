import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useRef,
  type ReactNode,
} from 'react'
import { invoke } from '@tauri-apps/api/core'
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
  isUnlocked: boolean
  /** Whether a wallet file exists at the default path */
  hasWalletFile: boolean
  /** The default wallet file path */
  walletFilePath: string | null
}

interface SendTxParams {
  recipient: Address
  amount: bigint
  privacyLevel: 'standard' | 'private'
  memo?: string
  customFee?: bigint
}

interface WalletContextValue extends WalletState {
  refreshBalance: () => Promise<void>
  refreshTransactions: () => Promise<void>
  sendTransaction: (params: SendTxParams) => Promise<{ success: boolean; txHash?: string; error?: string }>
  estimateFee: (amount: bigint, privacyLevel: 'standard' | 'private') => Promise<bigint>
  setAddress: (address: Address) => void
  unlockWallet: (mnemonic: string) => void
  unlockWalletFromFile: (password: string, path?: string) => Promise<{ success: boolean; error?: string }>
  saveWalletToFile: (mnemonic: string, password: string, path?: string) => Promise<{ success: boolean; path?: string; error?: string }>
  lockWallet: () => void
  checkWalletFile: () => Promise<void>
}

// Tauri command response types
interface SendTransactionResult {
  success: boolean
  txHash?: string
  error?: string
}

interface LoadWalletFileResult {
  success: boolean
  mnemonic?: string
  syncHeight: number
  error?: string
}

interface SaveWalletFileResult {
  success: boolean
  path?: string
  error?: string
}

interface WalletFileExistsResult {
  exists: boolean
  path: string
}

const WalletContext = createContext<WalletContextValue | null>(null)

export function WalletProvider({ children }: { children: ReactNode }) {
  const { connectedNode } = useConnection()
  const [adapter, setAdapter] = useState<LocalNodeAdapter | null>(null)

  // Store mnemonic in memory only (never persisted)
  const mnemonicRef = useRef<string | null>(null)

  const [state, setState] = useState<WalletState>({
    address: null,
    balance: null,
    transactions: [],
    isLoading: false,
    isSending: false,
    error: null,
    isUnlocked: false,
    hasWalletFile: false,
    walletFilePath: null,
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

  // Check for wallet file on mount
  const checkWalletFile = useCallback(async () => {
    try {
      const result = await invoke<WalletFileExistsResult>('wallet_file_exists', { path: null })
      setState(s => ({
        ...s,
        hasWalletFile: result.exists,
        walletFilePath: result.path,
      }))
    } catch {
      // Ignore errors - wallet file check is optional
    }
  }, [])

  useEffect(() => {
    checkWalletFile()
  }, [checkWalletFile])

  const unlockWallet = useCallback((mnemonic: string) => {
    mnemonicRef.current = mnemonic
    setState(s => ({ ...s, isUnlocked: true }))
  }, [])

  const unlockWalletFromFile = useCallback(async (password: string, path?: string) => {
    try {
      const result = await invoke<LoadWalletFileResult>('load_wallet_file', {
        params: { password, path: path || null }
      })

      if (result.success && result.mnemonic) {
        mnemonicRef.current = result.mnemonic
        setState(s => ({ ...s, isUnlocked: true }))
        return { success: true }
      } else {
        return { success: false, error: result.error || 'Failed to load wallet' }
      }
    } catch (err) {
      return { success: false, error: err instanceof Error ? err.message : 'Failed to load wallet' }
    }
  }, [])

  const saveWalletToFile = useCallback(async (mnemonic: string, password: string, path?: string) => {
    try {
      const result = await invoke<SaveWalletFileResult>('save_wallet_file', {
        params: { mnemonic, password, path: path || null }
      })

      if (result.success) {
        // Update state to reflect that wallet file now exists
        setState(s => ({
          ...s,
          hasWalletFile: true,
          walletFilePath: result.path || s.walletFilePath,
        }))
        return { success: true, path: result.path }
      } else {
        return { success: false, error: result.error || 'Failed to save wallet' }
      }
    } catch (err) {
      return { success: false, error: err instanceof Error ? err.message : 'Failed to save wallet' }
    }
  }, [])

  const lockWallet = useCallback(() => {
    mnemonicRef.current = null
    setState(s => ({ ...s, isUnlocked: false }))
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

  const sendTransaction = useCallback(async (params: SendTxParams) => {
    if (!connectedNode) {
      return { success: false, error: 'Not connected to node' }
    }

    if (!mnemonicRef.current) {
      return { success: false, error: 'Wallet is locked. Please unlock your wallet first.' }
    }

    setState(s => ({ ...s, isSending: true, error: null }))

    try {
      // Call the Tauri backend to build, sign, and submit the transaction
      const result = await invoke<SendTransactionResult>('send_transaction', {
        params: {
          mnemonic: mnemonicRef.current,
          recipient: params.recipient,
          amount: params.amount.toString(),
          privacyLevel: params.privacyLevel,
          memo: params.memo,
          customFee: params.customFee?.toString(),
          nodeHost: connectedNode.host,
          nodePort: connectedNode.port,
        }
      })

      setState(s => ({ ...s, isSending: false }))

      if (result.success) {
        // Refresh balance and transactions after successful send
        await refreshBalance()
        await refreshTransactions()
        return { success: true, txHash: result.txHash }
      } else {
        setState(s => ({ ...s, error: result.error || 'Transaction failed' }))
        return { success: false, error: result.error }
      }
    } catch (err) {
      const error = err instanceof Error ? err.message : 'Transaction failed'
      setState(s => ({ ...s, isSending: false, error }))
      return { success: false, error }
    }
  }, [connectedNode, refreshBalance, refreshTransactions])

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
        unlockWallet,
        unlockWalletFromFile,
        saveWalletToFile,
        lockWallet,
        checkWalletFile,
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
