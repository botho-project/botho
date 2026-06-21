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
import { AddressBook, ClaimLinkStore, saveWallet, loadWallet, loadWalletWithKey, getWalletInfo, deriveAddress, deriveKeypairs, parseAddress, isValidMnemonic, clearWallet, createClaimLinkMnemonic, buildClaimLink, VaultKey } from '@botho/core'
import type { Balance, Contact, NodeInfo, Transaction, ClaimLinkRecord, Timestamp } from '@botho/core'
import { buildSendTransaction, spendableBalance, buildOwnedHistory } from '@botho/wasm-signer'
import { buildAndSubmitSend, scanEphemeral, sweepEphemeral, SWEEP_FEE_RESERVE } from '../lib/claim-link-ops'
import { type NetworkConfig, loadSelectedNetwork, loadSelectedIngress, NETWORKS, DEFAULT_NETWORK_ID, DEFAULT_INGRESS_ID, createCustomNetwork, networkForIngress, getIngressNode } from '../config/networks'

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

  // Outstanding claim links (sender side, #460)
  claimLinks: ClaimLinkRecord[]
}

/** Result of creating a claimable payment link. */
export interface CreatedClaimLink {
  /** The shareable URL with the secret in the fragment. */
  url: string
  /** The ephemeral receiving address the funds were sent to. */
  ephAddress: string
  /** Net amount the recipient will receive, in picocredits. */
  amount: bigint
  /** Funding transaction hash. */
  fundingTxHash: string
  /** Local record id. */
  id: string
}

interface WalletContextValue extends WalletState {
  // Connection
  connect: () => Promise<void>
  disconnect: () => void

  // Adapter (for explorer/blockchain queries)
  adapter: RemoteNodeAdapter

  // Wallet
  createWallet: (mnemonic: string, password?: string) => Promise<void>
  importWallet: (seedPhrase: string, password?: string) => Promise<void>
  unlockWallet: (password: string) => Promise<void>
  exportWallet: (password?: string) => Promise<string | null>
  resetWallet: () => void

  /**
   * The unlocked vault key for the current session, or null when the wallet is
   * locked or stored in plaintext. Sibling features — claim-link secrets (#474)
   * and the encrypted address book (#476) — use this to encrypt/decrypt their
   * data under the SAME password-derived key while the wallet is unlocked. The
   * key lives in memory only and is cleared on reset/refresh.
   */
  getVaultKey: () => VaultKey | null

  // Transactions
  send: (to: string, amount: bigint, memo?: string) => Promise<string>
  refreshBalance: () => Promise<void>
  refreshTransactions: () => Promise<void>

  // Address book
  addContact: (name: string, address: string, notes?: string) => Promise<Contact>
  updateContact: (id: string, updates: Partial<Pick<Contact, 'name' | 'address' | 'notes'>>) => Promise<Contact>
  deleteContact: (id: string) => Promise<void>
  getContactName: (address: string) => string
  /**
   * Upsert + bump a "previously-paid" address book entry. If the address is not
   * yet a contact, create a minimal (blank-name) entry so it can be labelled
   * later, then record the payment; if it already exists, just bump its
   * txCount/lastTxAt. Idempotent per call (no double-count).
   */
  recordPayment: (address: string) => Promise<void>
  /** Search saved contacts by name or address (case-insensitive substring). */
  searchContacts: (query: string) => Contact[]

  // Claimable payment links (#460)
  /**
   * Create a claim link: fund a fresh ephemeral wallet from this wallet with
   * `amount` + a sweep-fee reserve, persist the outstanding record, and return
   * the shareable URL. `amount` is the NET the recipient receives.
   */
  sendViaLink: (amount: bigint) => Promise<CreatedClaimLink>
  /** Refresh outstanding-link statuses by re-scanning each ephemeral wallet. */
  refreshClaimLinks: () => Promise<void>
  /** Reclaim an unclaimed link's funds back to this wallet. */
  refundClaimLink: (id: string) => Promise<string>
  /** Forget a claim-link record locally (does not touch on-chain funds). */
  forgetClaimLink: (id: string) => Promise<void>
}

/** Encode bytes as a lowercase hex string. */
function toHex(bytes: Uint8Array): string {
  let out = ''
  for (const b of bytes) out += b.toString(16).padStart(2, '0')
  return out
}

/** Decode a hex string into bytes. */
function hexToBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2)
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
  }
  return out
}

/**
 * Compute the wallet's balance, spent-filtered for the thin-wallet path (#392).
 *
 * The node's `wallet_getBalance` (used by `adapter.getBalance`) only
 * spent-filters the node's OWN configured wallet — for an arbitrary thin-wallet
 * key it would either error or report ownership-only sums that count
 * already-spent outputs, overstating the balance after a send. When the wallet
 * is unlocked (mnemonic available), we instead compute the true SPENDABLE
 * balance entirely client-side: derive owned-output key images in wasm and ask
 * the node's `chain_areKeyImagesSpent` RPC which are spent. If the wallet is
 * locked (no mnemonic), fall back to the node RPC balance.
 */
async function fetchBalance(
  adapter: RemoteNodeAdapter,
  address: string,
  mnemonic: string | null,
): Promise<Balance> {
  if (!mnemonic) {
    return adapter.getBalance([address])
  }
  try {
    const kp = deriveKeypairs(mnemonic, 0)
    const available = await spendableBalance(
      {
        spendPrivateKey: toHex(kp.spendPrivate),
        viewPrivateKey: toHex(kp.viewPrivate),
      },
      {
        getChainHeight: () => adapter.getBlockHeight(),
        getOutputs: (start, end) => adapter.getRawOutputs(start, end),
        areKeyImagesSpent: (keyImages) => adapter.areKeyImagesSpent(keyImages),
      },
    )
    return { available, pending: 0n, total: available }
  } catch {
    // If the client-side spendable computation is unavailable (e.g. the wasm
    // artifact failed to load), fall back to the node RPC balance rather than
    // surfacing no balance at all.
    return adapter.getBalance([address])
  }
}

/**
 * Build the wallet's transaction history CLIENT-SIDE from its OWNED outputs
 * (#459), mirroring how {@link fetchBalance} computes balance.
 *
 * The node has no way to tell which on-chain outputs belong to a thin wallet, so
 * the old adapter `getTransactionHistory` mapped EVERY chain output to a bogus
 * "received 0 BTH" entry (~100+ rows of spam). Instead we reuse the wasm scan
 * path: fetch outputs (with block height) and let the wasm signer keep only the
 * ones this wallet owns, with their REAL decoded amounts, then map each owned
 * output to a `receive` (and a `spend` if its key image is spent). Requires the
 * mnemonic (unlocked wallet); when locked we return an empty history rather than
 * the old spam.
 */
async function fetchHistory(
  adapter: RemoteNodeAdapter,
  mnemonic: string | null,
): Promise<Transaction[]> {
  if (!mnemonic) return []
  try {
    const kp = deriveKeypairs(mnemonic, 0)
    const entries = await buildOwnedHistory(
      {
        spendPrivateKey: toHex(kp.spendPrivate),
        viewPrivateKey: toHex(kp.viewPrivate),
      },
      {
        getChainHeight: () => adapter.getBlockHeight(),
        getOutputsWithMeta: (start, end) => adapter.getRawOutputsWithMeta(start, end),
        areKeyImagesSpent: (keyImages) => adapter.areKeyImagesSpent(keyImages),
      },
    )
    return entries.map((e) => ({
      id: e.txHash,
      type: e.type === 'spend' ? ('send' as const) : ('receive' as const),
      amount: e.amount,
      fee: 0n,
      privacyLevel: 'private' as const,
      cryptoType: 'clsag' as const,
      status: 'confirmed' as const,
      timestamp: Date.now(),
      blockHeight: e.blockHeight,
      confirmations: 0,
    }))
  } catch {
    // wasm artifact missing or scan failed: show no history rather than spam.
    return []
  }
}

/**
 * Derive the session vault key bound to the stored seed blob's salt, so the
 * session key matches the seed blob exactly. Reads the just-written encrypted
 * seed from localStorage and re-derives from (password + blob). Returns null if
 * no encrypted seed is present.
 */
async function deriveSessionVaultKey(password: string): Promise<VaultKey | null> {
  const blob = localStorage.getItem('botho-wallet-mnemonic')
  const encrypted = localStorage.getItem('botho-wallet-encrypted') === 'true'
  if (!blob || !encrypted) return null
  return VaultKey.fromPasswordAndBlob(password, blob)
}

const WalletContext = createContext<WalletContextValue | null>(null)

const addressBook = new AddressBook()
const claimLinkStore = new ClaimLinkStore()

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

  // Route the adapter to the user's selected SCP ingress node on first load.
  const ingress = getIngressNode(loadSelectedIngress())
  if (ingress) {
    return networkForIngress(ingress)
  }

  return NETWORKS[networkId] || NETWORKS[DEFAULT_NETWORK_ID] || NETWORKS[DEFAULT_INGRESS_ID]
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
    claimLinks: [],
  })

  // Store adapter in ref so we can recreate it when network changes
  const adapterRef = useRef<RemoteNodeAdapter>(createAdapterFromNetwork(getInitialNetwork()))

  // Store mnemonic in memory after unlock (cleared on page refresh)
  const mnemonicRef = useRef<string | null>(null)

  // Store the unlocked vault key in memory for the session so sibling features
  // (#474 claim-link secrets, #476 address book) can encrypt under the same
  // password-derived key. Null while locked or for plaintext wallets. Cleared
  // on reset and on page refresh (in-memory only).
  const vaultKeyRef = useRef<VaultKey | null>(null)

  // Load address book on mount
  useEffect(() => {
    addressBook.load().then(() => {
      setState(s => ({ ...s, contacts: addressBook.getAll() }))
    })
  }, [])

  // Load outstanding claim links on mount
  useEffect(() => {
    claimLinkStore.load().then(() => {
      setState(s => ({ ...s, claimLinks: claimLinkStore.getAll() }))
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
          fetchBalance(adapter, state.address!, mnemonicRef.current),
          fetchHistory(adapter, mnemonicRef.current),
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
          fetchBalance(adapter, state.address!, mnemonicRef.current),
          fetchHistory(adapter, mnemonicRef.current),
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

        // If not encrypted, load balance immediately. Load the (unencrypted)
        // mnemonic into memory first so the balance is spent-filtered (#392).
        if (!walletInfo.isEncrypted && walletInfo.address) {
          if (!mnemonicRef.current) {
            const stored = await loadWallet()
            if (stored) mnemonicRef.current = stored.mnemonic
          }
          const [balance, transactions] = await Promise.all([
            fetchBalance(adapter, walletInfo.address, mnemonicRef.current),
            fetchHistory(adapter, mnemonicRef.current),
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
    // Derive + hold the session vault key (bound to the seed blob's salt) so
    // sibling data can be encrypted under the same key (#474/#476). Null for
    // plaintext wallets.
    vaultKeyRef.current = password ? await deriveSessionVaultKey(password) : null

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
    vaultKeyRef.current = password ? await deriveSessionVaultKey(password) : null

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
      const balance = await fetchBalance(adapter, address, mnemonicRef.current)
      const transactions = await fetchHistory(adapter, mnemonicRef.current)
      setState(s => ({ ...s, balance, transactions }))
    }
  }, [])

  const unlockWallet = useCallback(async (password: string) => {
    // loadWalletWithKey decrypts the seed AND returns the session vault key,
    // transparently migrating legacy (plaintext-header/100k) blobs to the
    // current versioned format on success (#475).
    const stored = await loadWalletWithKey(password)
    if (!stored) {
      throw new Error('No wallet found')
    }

    // Store mnemonic + session vault key in memory
    mnemonicRef.current = stored.mnemonic
    vaultKeyRef.current = stored.vaultKey

    setState(s => ({ ...s, isLocked: false }))

    // Fetch balance now that we're unlocked
    const adapter = adapterRef.current
    if (adapter.isConnected() && stored.address) {
      const [balance, transactions] = await Promise.all([
        fetchBalance(adapter, stored.address, mnemonicRef.current),
        fetchHistory(adapter, mnemonicRef.current),
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
    // Clear mnemonic + vault key from memory
    mnemonicRef.current = null
    vaultKeyRef.current = null
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

  const getVaultKey = useCallback(() => vaultKeyRef.current, [])

  const send = useCallback(async (to: string, amount: bigint, _memo?: string): Promise<string> => {
    const adapter = adapterRef.current
    if (!adapter.isConnected()) {
      throw new Error('Not connected to a node')
    }

    const mnemonic = mnemonicRef.current
    if (!mnemonic) {
      throw new Error('Wallet is locked. Unlock it before sending.')
    }

    // 1. Derive the account spend/view private keys from the mnemonic. These
    //    are byte-identical to the keys the node derives (verified by
    //    derivation-parity.test.ts), so a tx signed with them is accepted.
    const kp = deriveKeypairs(mnemonic, 0)

    // 2. Decode the recipient address into its raw spend/view public keys.
    const recipientKeys = parseAddress(to)

    // 3. Determine a fee. estimateFee returns the node's recommended/minimum
    //    fee in picocredits, but it can come back below the consensus minimum
    //    (e.g. a per-byte estimate of a few thousand picocredits). The signer
    //    rejects any tx whose fee is under MIN_TX_FEE, so clamp the fee to that
    //    floor regardless of what the estimator returns.
    const MIN_TX_FEE = 100_000_000n // signer's MIN_TX_FEE (picocredits)
    let fee: bigint
    try {
      fee = await adapter.estimateFee(0)
    } catch {
      fee = 0n
    }
    if (fee < MIN_TX_FEE) {
      fee = MIN_TX_FEE
    }

    // 4. Build + CLSAG-sign entirely client-side (wasm). The keys never leave
    //    the browser; only the signed bytes are submitted.
    const { txHex } = await buildSendTransaction({
      keys: {
        spendPrivateKey: toHex(kp.spendPrivate),
        viewPrivateKey: toHex(kp.viewPrivate),
      },
      recipient: {
        spend_public_key: toHex(recipientKeys.spendPublic),
        view_public_key: toHex(recipientKeys.viewPublic),
      },
      amount,
      fee,
      rpc: {
        getChainHeight: () => adapter.getBlockHeight(),
        getOutputs: (start, end) => adapter.getRawOutputs(start, end),
        areKeyImagesSpent: (keyImages) => adapter.areKeyImagesSpent(keyImages),
      },
    })

    // 5. Submit the signed tx to the node.
    const result = await adapter.submitTransaction(hexToBytes(txHex))
    if (!result.success || !result.txHash) {
      throw new Error(result.error || 'Transaction submission failed')
    }

    // Persist the recipient in the address book so it appears as "previously
    // paid" and can be labelled/annotated later. Upsert: create a blank-name
    // entry if new, then bump txCount/lastTxAt. Best-effort; never fail a send.
    try {
      const now = Math.floor(Date.now() / 1000) as Timestamp
      if (!addressBook.findByAddress(to)) {
        await addressBook.add('', to)
      }
      await addressBook.recordTransaction(to, now)
      setState((s) => ({ ...s, contacts: addressBook.getAll() }))
    } catch {
      // Address-book persistence is non-critical; ignore failures.
    }

    // Refresh balance/history opportunistically; ignore failures.
    if (state.address) {
      fetchBalance(adapter, state.address, mnemonicRef.current)
        .then((balance) => setState((s) => ({ ...s, balance })))
        .catch(() => {})
    }

    return result.txHash
  }, [state.address])

  const refreshBalance = useCallback(async () => {
    const adapter = adapterRef.current
    if (!state.address || !adapter.isConnected()) return
    const balance = await fetchBalance(adapter, state.address, mnemonicRef.current)
    setState(s => ({ ...s, balance }))
  }, [state.address])

  const refreshTransactions = useCallback(async () => {
    const adapter = adapterRef.current
    if (!state.address || !adapter.isConnected()) return
    const transactions = await fetchHistory(adapter, mnemonicRef.current)
    setState(s => ({ ...s, transactions }))
  }, [state.address])

  // Claimable payment link methods (#460) ---------------------------------

  const sendViaLink = useCallback(async (amount: bigint): Promise<CreatedClaimLink> => {
    const adapter = adapterRef.current
    if (!adapter.isConnected()) throw new Error('Not connected to a node')
    const mnemonic = mnemonicRef.current
    if (!mnemonic) throw new Error('Wallet is locked. Unlock it before sending.')
    if (amount <= 0n) throw new Error('Amount must be greater than 0')

    // 1. Generate the ephemeral wallet (the link's bearer secret) and its addr.
    const ephMnemonic = createClaimLinkMnemonic()
    const ephAddress = deriveAddress(ephMnemonic)

    // 2. Fund the ephemeral address with amount + a sweep-fee reserve, so the
    //    recipient nets `amount` after paying the sweep fee from the output.
    const fundingAmount = amount + SWEEP_FEE_RESERVE
    const fundingTxHash = await buildAndSubmitSend(adapter, mnemonic, ephAddress, fundingAmount)

    // 3. Persist the outstanding link locally so the sender can track/refund.
    const record = await claimLinkStore.add({
      ephMnemonic,
      ephAddress,
      amount,
      fundingTxHash,
    })
    setState(s => ({ ...s, claimLinks: claimLinkStore.getAll() }))

    // 4. Build the shareable URL with the secret in the fragment (+ amount hint).
    const origin =
      typeof window !== 'undefined' && window.location?.origin
        ? window.location.origin
        : 'https://wallet.botho.io'
    const url = buildClaimLink(origin, ephMnemonic, amount)

    // Refresh the sender's balance opportunistically.
    if (state.address) {
      fetchBalance(adapter, state.address, mnemonicRef.current)
        .then((balance) => setState((s) => ({ ...s, balance })))
        .catch(() => {})
    }

    return { url, ephAddress, amount, fundingTxHash, id: record.id }
  }, [state.address])

  const refreshClaimLinks = useCallback(async () => {
    const adapter = adapterRef.current
    if (!adapter.isConnected()) return
    const records = claimLinkStore.getAll()
    for (const r of records) {
      if (r.status !== 'outstanding') continue
      try {
        const { gross } = await scanEphemeral(adapter, r.ephMnemonic)
        // An outstanding link whose ephemeral output is no longer spendable
        // (gross === 0) AND whose funding has had time to confirm means it was
        // swept by someone — mark it claimed. We only flip on a zero result to
        // avoid racing the funding confirmation.
        if (gross === 0n) {
          await claimLinkStore.setStatus(r.id, 'claimed')
        }
      } catch {
        // Ignore scan errors; leave status unchanged.
      }
    }
    setState(s => ({ ...s, claimLinks: claimLinkStore.getAll() }))
  }, [])

  const refundClaimLink = useCallback(async (id: string): Promise<string> => {
    const adapter = adapterRef.current
    if (!adapter.isConnected()) throw new Error('Not connected to a node')
    if (!state.address) throw new Error('No wallet address to refund to')
    const record = claimLinkStore.getAll().find((r) => r.id === id)
    if (!record) throw new Error('Claim link not found')

    // Sweep the ephemeral output back to the sender's own address.
    const { txHash } = await sweepEphemeral(adapter, record.ephMnemonic, state.address)
    await claimLinkStore.setStatus(id, 'refunded')
    setState(s => ({ ...s, claimLinks: claimLinkStore.getAll() }))

    if (state.address) {
      fetchBalance(adapter, state.address, mnemonicRef.current)
        .then((balance) => setState((s) => ({ ...s, balance })))
        .catch(() => {})
    }
    return txHash
  }, [state.address])

  const forgetClaimLink = useCallback(async (id: string) => {
    await claimLinkStore.delete(id)
    setState(s => ({ ...s, claimLinks: claimLinkStore.getAll() }))
  }, [])

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

  const recordPayment = useCallback(async (address: string) => {
    const now = Math.floor(Date.now() / 1000) as Timestamp
    const existing = addressBook.findByAddress(address)
    if (!existing) {
      // Create a minimal, blank-name "previously paid" entry so the user can
      // label/annotate it later. `add` initializes txCount to 0.
      await addressBook.add('', address)
    }
    // Bump txCount/lastTxAt exactly once for this payment.
    await addressBook.recordTransaction(address, now)
    setState(s => ({ ...s, contacts: addressBook.getAll() }))
  }, [])

  const searchContacts = useCallback((query: string) => {
    return addressBook.search(query)
  }, [])

  return (
    <WalletContext.Provider
      value={{
        ...state,
        connect,
        disconnect,
        adapter: adapterRef.current,
        createWallet,
        importWallet,
        unlockWallet,
        exportWallet,
        resetWallet,
        getVaultKey,
        send,
        refreshBalance,
        refreshTransactions,
        addContact,
        updateContact,
        deleteContact,
        getContactName,
        recordPayment,
        searchContacts,
        sendViaLink,
        refreshClaimLinks,
        refundClaimLink,
        forgetClaimLink,
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
 * Returns the same adapter instance used by the WalletProvider
 */
export function useAdapter() {
  const context = useContext(WalletContext)
  if (!context) {
    throw new Error('useAdapter must be used within a WalletProvider')
  }
  return context.adapter
}
