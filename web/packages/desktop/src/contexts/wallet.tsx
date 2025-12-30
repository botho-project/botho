import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
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
  /** Seconds until session expires (if unlocked) */
  sessionExpiresIn: number | null
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
  /** Unlock wallet from file using password (mnemonic stays in Rust) */
  unlockWallet: (password: string, path?: string) => Promise<{ success: boolean; address?: string; error?: string }>
  /** Create new wallet file and unlock it */
  createWallet: (mnemonic: string, password: string, path?: string) => Promise<{ success: boolean; path?: string; address?: string; error?: string }>
  /** Lock wallet and zeroize keys in Rust */
  lockWallet: () => Promise<void>
  checkWalletFile: () => Promise<void>
  /** Check session status */
  checkSessionStatus: () => Promise<void>
}

// Tauri command response types
interface SendTransactionResult {
  success: boolean
  txHash?: string
  error?: string
}

interface UnlockWalletResult {
  success: boolean
  address?: string
  hasTimeout: boolean
  timeoutMins: number
  error?: string
}

interface CreateWalletResult {
  success: boolean
  path?: string
  address?: string
  error?: string
}

interface SessionStatusResult {
  isUnlocked: boolean
  address?: string
  expiresInSecs?: number
}

interface WalletFileExistsResult {
  exists: boolean
  path: string
}

const WalletContext = createContext<WalletContextValue | null>(null)

export function WalletProvider({ children }: { children: ReactNode }) {
  const { connectedNode } = useConnection()
  const [adapter, setAdapter] = useState<LocalNodeAdapter | null>(null)

  // SECURITY: Mnemonic is NEVER stored in JavaScript.
  // All key material stays in Rust memory and is accessed via session.

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
    sessionExpiresIn: null,
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

  // Check session status from Rust
  const checkSessionStatus = useCallback(async () => {
    try {
      const result = await invoke<SessionStatusResult>('get_session_status')
      setState(s => ({
        ...s,
        isUnlocked: result.isUnlocked,
        address: result.address || s.address,
        sessionExpiresIn: result.expiresInSecs ?? null,
      }))
    } catch {
      // Ignore errors - session check is optional
    }
  }, [])

  // Unlock wallet from file (mnemonic stays in Rust)
  const unlockWallet = useCallback(async (password: string, path?: string) => {
    try {
      const result = await invoke<UnlockWalletResult>('unlock_wallet', {
        params: { password, path: path || null }
      })

      if (result.success) {
        setState(s => ({
          ...s,
          isUnlocked: true,
          address: result.address || s.address,
        }))
        return { success: true, address: result.address }
      } else {
        return { success: false, error: result.error || 'Failed to unlock wallet' }
      }
    } catch (err) {
      return { success: false, error: err instanceof Error ? err.message : 'Failed to unlock wallet' }
    }
  }, [])

  // Create new wallet file and unlock it
  const createWallet = useCallback(async (mnemonic: string, password: string, path?: string) => {
    try {
      const result = await invoke<CreateWalletResult>('create_wallet', {
        params: { mnemonic, password, path: path || null }
      })

      if (result.success) {
        // Update state to reflect that wallet file now exists and is unlocked
        setState(s => ({
          ...s,
          isUnlocked: true,
          hasWalletFile: true,
          walletFilePath: result.path || s.walletFilePath,
          address: result.address || s.address,
        }))
        return { success: true, path: result.path, address: result.address }
      } else {
        return { success: false, error: result.error || 'Failed to create wallet' }
      }
    } catch (err) {
      return { success: false, error: err instanceof Error ? err.message : 'Failed to create wallet' }
    }
  }, [])

  // Lock wallet (keys are zeroized in Rust)
  const lockWallet = useCallback(async () => {
    try {
      await invoke<boolean>('lock_wallet')
    } catch {
      // Ignore errors
    }
    setState(s => ({ ...s, isUnlocked: false, sessionExpiresIn: null }))
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

    // SECURITY: No mnemonic is passed - Rust uses the cached session
    setState(s => ({ ...s, isSending: true, error: null }))

    try {
      // Call the Tauri backend to build, sign, and submit the transaction
      // Keys are retrieved from the session in Rust - never exposed to JS
      const result = await invoke<SendTransactionResult>('send_transaction', {
        params: {
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

  // Check session status on mount and periodically
  useEffect(() => {
    checkSessionStatus()
    // Check every minute for session expiry
    const interval = setInterval(checkSessionStatus, 60000)
    return () => clearInterval(interval)
  }, [checkSessionStatus])

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
        createWallet,
        lockWallet,
        checkWalletFile,
        checkSessionStatus,
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
