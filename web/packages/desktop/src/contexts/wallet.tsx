import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useRef,
  useLayoutEffect,
  type ReactNode,
} from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { FeeEstimate } from '@botho/adapters'
import type { Balance, Transaction, Address } from '@botho/core'
import { useConnection } from './connection'
import { walletNetwork, validWalletAddress, addressCacheKey } from '../config/wallet-network'

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

/** Result of generating a new mnemonic (secure flow) */
interface GenerateMnemonicResult {
  success: boolean
  /** The 24 mnemonic words (display only, do NOT store) */
  words?: string[]
  /** 1-indexed word positions user must verify */
  verifyPositions?: number[]
  /** Seconds until this pending wallet expires */
  expiresInSecs?: number
  error?: string
}

interface WalletContextValue extends WalletState {
  refreshBalance: () => Promise<void>
  refreshTransactions: () => Promise<void>
  sendTransaction: (params: SendTxParams) => Promise<{ success: boolean; txHash?: string; error?: string }>
  estimateFee: (amount: bigint, privacyLevel: 'standard' | 'private') => Promise<FeeEstimate>
  setAddress: (address: Address) => void
  /** Unlock wallet from file using password (mnemonic stays in Rust) */
  unlockWallet: (password: string, path?: string) => Promise<{ success: boolean; address?: string; error?: string }>
  /**
   * Generate a new mnemonic in Rust (SECURE - mnemonic never sent from JS).
   * Returns words for display and positions to verify.
   */
  generateMnemonic: () => Promise<GenerateMnemonicResult>
  /**
   * Confirm new wallet creation using verification words (SECURE flow).
   * The mnemonic is NEVER sent from JS - uses cached mnemonic from generateMnemonic().
   */
  confirmNewWallet: (password: string, verifyWords: string[], path?: string) => Promise<{ success: boolean; path?: string; address?: string; error?: string }>
  /** Cancel pending wallet creation */
  cancelPendingWallet: () => Promise<void>
  /**
   * Import an existing wallet from a mnemonic phrase.
   * NOTE: Mnemonic must cross JS/Rust boundary (unavoidable for restore).
   */
  importWallet: (mnemonic: string, password: string, path?: string) => Promise<{ success: boolean; path?: string; address?: string; error?: string }>
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
  const { connectedNode, adapter, endpoint } = useConnection()
  const network = walletNetwork(connectedNode?.networkId)
  const generation = useRef(0)
  const mounted = useRef(false)
  const current = (token: number) => mounted.current && generation.current === token

  // A selected-node/network change invalidates pending IPC and read completions.
  useLayoutEffect(() => {
    mounted.current = true
    generation.current += 1
    const cache = network ? localStorage.getItem(addressCacheKey(network)) : null
    const address = cache && validWalletAddress(cache, network) ? cache : null
    if (network && cache && !address) localStorage.removeItem(addressCacheKey(network))
    // This key contains public display/watch data only, never wallet material.
    localStorage.removeItem('botho-wallet-address')
    setState(s => ({ ...s, address, balance: null, transactions: [], isLoading: false,
      isSending: false, error: null, isUnlocked: false, sessionExpiresIn: null }))
    return () => { mounted.current = false; generation.current += 1 }
  }, [adapter, endpoint, connectedNode?.id, network])

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

  const setAddress = useCallback((address: Address) => {
    if (!network || !validWalletAddress(address, network)) {
      throw new Error('A complete v2 address for the connected network is required')
    }
    setState(s => ({ ...s, address }))
    localStorage.setItem(addressCacheKey(network), address)
  }, [network])

  // Check for wallet file on mount
  const checkWalletFile = useCallback(async () => {
    const token = generation.current
    try {
      const result = await invoke<WalletFileExistsResult>('wallet_file_exists', { path: null })
      if (!current(token)) return
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
    if (!network) return
    const token = generation.current
    try {
      const result = await invoke<SessionStatusResult>('get_session_status', { network })
      if (!current(token)) return
      if (result.address && !validWalletAddress(result.address, network)) throw new Error('Native wallet returned an incompatible address')
      setState(s => ({
        ...s,
        isUnlocked: result.isUnlocked,
        address: result.address || s.address,
        sessionExpiresIn: result.expiresInSecs ?? null,
      }))
    } catch {
      // Ignore errors - session check is optional
    }
  }, [network, adapter, connectedNode?.id])

  // Unlock wallet from file (mnemonic stays in Rust)
  const unlockWallet = useCallback(async (password: string, path?: string) => {
    if (!network) return { success: false, error: 'Connect to a recognized network first' }
    const token = generation.current
    try {
      const result = await invoke<UnlockWalletResult>('unlock_wallet', {
        params: { network, password, path: path || null }
      })

      if (!current(token)) return { success: false, error: 'Network changed while wallet operation completed' }
      if (result.success && (!result.address || !validWalletAddress(result.address, network))) {
        return { success: false, error: 'Native wallet returned an incompatible address' }
      }
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
  }, [network, adapter, connectedNode?.id])

  // Generate mnemonic in Rust (SECURE - never accept mnemonic from JS for new wallets)
  const generateMnemonic = useCallback(async (): Promise<GenerateMnemonicResult> => {
    try {
      const result = await invoke<GenerateMnemonicResult>('generate_mnemonic')
      return result
    } catch (err) {
      return { success: false, error: err instanceof Error ? err.message : 'Failed to generate mnemonic' }
    }
  }, [])

  // Confirm new wallet using verification words (SECURE flow)
  const confirmNewWallet = useCallback(async (password: string, verifyWords: string[], path?: string) => {
    if (!network) return { success: false, error: 'Connect to a recognized network first' }
    const token = generation.current
    try {
      const result = await invoke<CreateWalletResult>('confirm_new_wallet', {
        params: { network, password, verifyWords, path: path || null }
      })

      if (!current(token)) return { success: false, error: 'Network changed while wallet operation completed' }
      if (result.success && (!result.address || !validWalletAddress(result.address, network))) {
        return { success: false, error: 'Native wallet returned an incompatible address' }
      }
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
  }, [network, adapter, connectedNode?.id])

  // Cancel pending wallet creation
  const cancelPendingWallet = useCallback(async () => {
    try {
      await invoke<boolean>('cancel_pending_wallet')
    } catch {
      // Ignore errors
    }
  }, [])

  // Import existing wallet from mnemonic (mnemonic crosses JS/Rust boundary - unavoidable for restore)
  const importWallet = useCallback(async (mnemonic: string, password: string, path?: string) => {
    if (!network) return { success: false, error: 'Connect to a recognized network first' }
    const token = generation.current
    try {
      const result = await invoke<CreateWalletResult>('import_wallet', {
        params: { network, mnemonic, password, path: path || null }
      })

      if (!current(token)) return { success: false, error: 'Network changed while wallet operation completed' }
      if (result.success && (!result.address || !validWalletAddress(result.address, network))) {
        return { success: false, error: 'Native wallet returned an incompatible address' }
      }
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
        return { success: false, error: result.error || 'Failed to import wallet' }
      }
    } catch (err) {
      return { success: false, error: err instanceof Error ? err.message : 'Failed to import wallet' }
    }
  }, [network, adapter, connectedNode?.id])

  // Lock wallet (keys are zeroized in Rust)
  const lockWallet = useCallback(async () => {
    generation.current += 1
    const token = generation.current
    try {
      await invoke<boolean>('lock_wallet')
    } catch {
      // Ignore errors
    }
    if (current(token)) setState(s => ({ ...s, isUnlocked: false, sessionExpiresIn: null }))
  }, [])

  const refreshBalance = useCallback(async () => {
    if (!adapter || !network || !validWalletAddress(state.address ?? '', network) || !state.address) return
    const token = generation.current

    setState(s => ({ ...s, isLoading: true, error: null }))
    try {
      let balance: Balance
      if (state.isUnlocked) {
        if (!endpoint) return
        const result = await invoke<{ success: boolean; balance: string; error?: string }>('get_balance', {
          params: { network, endpoint },
        })
        if (!result.success) throw new Error(result.error ?? 'Native balance sync failed')
        const amount = BigInt(result.balance)
        balance = { available: amount, pending: 0n, total: amount }
      } else {
        balance = await adapter.getBalance([state.address])
      }
      if (!current(token)) return
      setState(s => ({ ...s, balance, isLoading: false }))
    } catch (err) {
      if (!current(token)) return
      setState(s => ({
        ...s,
        isLoading: false,
        error: err instanceof Error ? err.message : 'Failed to fetch balance',
      }))
    }
  }, [adapter, endpoint, state.address, state.isUnlocked, network])

  const refreshTransactions = useCallback(async () => {
    if (!adapter || !network || !validWalletAddress(state.address ?? '', network) || !state.address) return
    const token = generation.current

    setState(s => ({ ...s, isLoading: true, error: null }))
    try {
      const transactions = await adapter.getTransactionHistory([state.address], { limit: 50 })
      if (!current(token)) return
      setState(s => ({ ...s, transactions, isLoading: false }))
    } catch (err) {
      if (!current(token)) return
      setState(s => ({
        ...s,
        isLoading: false,
        error: err instanceof Error ? err.message : 'Failed to fetch transactions',
      }))
    }
  }, [adapter, endpoint, state.address, state.isUnlocked, network])

  const estimateFee = useCallback(async (_amount: bigint, privacyLevel: 'standard' | 'private'): Promise<FeeEstimate> => {
    if (!adapter) return { fee: BigInt(0), clusterFactorDisplay: '1.00x' }

    // Estimate transaction size based on privacy level
    // Standard: minting transaction (PoW-bound attribution, no signature — ADR 0006)
    // Private: CLSAG ring signature (~700 bytes per input, ring=20)
    const sizeBytes = privacyLevel === 'private' ? 4000 : 2000
    // NOTE (#634): no cluster wealth is passed here, so this UI estimate uses the
    // node's 1.00x base rate. Unlike the web wallet, the desktop app keeps the
    // wallet keys inside Rust (never in JS), so this context cannot scan owned
    // outputs to derive the cluster's target keys. The authoritative,
    // cluster-aware fee is computed in the Rust `send_transaction` path at send
    // time; `LocalNodeAdapter.getClusterWealth` is available for a future
    // Rust-side (or key-bearing) caller to supply it.
    return adapter.estimateFee(sizeBytes)
  }, [adapter])

  const sendTransaction = useCallback(async (params: SendTxParams) => {
    if (!connectedNode || !network || !endpoint) {
      return { success: false, error: 'Not connected to a recognized network' }
    }

    const token = generation.current
    // SECURITY: No mnemonic is passed - Rust uses the cached session
    setState(s => ({ ...s, isSending: true, error: null }))

    try {
      // Call the Tauri backend to build, sign, and submit the transaction
      // Keys are retrieved from the session in Rust - never exposed to JS
      const result = await invoke<SendTransactionResult>('send_transaction', {
        params: {
          network,
          recipient: params.recipient,
          amount: params.amount.toString(),
          privacyLevel: params.privacyLevel,
          memo: params.memo,
          customFee: params.customFee?.toString(),
          endpoint,
        }
      })

      if (!current(token)) return { success: result.success, txHash: result.txHash, error: result.error }
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
      if (current(token)) setState(s => ({ ...s, isSending: false, error }))
      return { success: false, error }
    }
  }, [connectedNode, endpoint, network, refreshBalance, refreshTransactions])

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
        generateMnemonic,
        confirmNewWallet,
        cancelPendingWallet,
        importWallet,
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
