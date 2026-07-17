import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useRef,
  type ReactNode,
} from 'react'
import { RemoteNodeAdapter, type FeeEstimate, type WsConnectionStatus } from '@botho/adapters'
import { AddressBook, EncryptedAddressBook, ClaimLinkStore, EncryptedClaimLinks, saveWallet, loadWallet, loadWalletWithKey, getWalletInfo, deriveKeypairs, parseAddress, isValidMnemonic, clearWallet, createClaimLinkMnemonic, buildClaimLink, assertClaimLinkAmountWithinCap, VaultKey, MIN_PASSWORD_LENGTH } from '@botho/core'
import { deriveV2Address } from '@botho/wasm-signer'
import type { Balance, Contact, NodeInfo, Transaction, ClaimLinkRecord, Timestamp } from '@botho/core'
import { buildSendTransaction, spendableBalance, buildOwnedHistory, netOwnedHistory, ownedOutputTargetKeys, deriveKemPublicKey, mnemonicToSeedHex } from '@botho/wasm-signer'
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

/**
 * Optional, TYPED extras for a {@link WalletContextValue.send}. The two fields
 * are DELIBERATELY distinct channels for two incompatible meanings (#1037):
 *
 * - `note` is a human free-text note (e.g. the Send-modal "Add a note…" field or
 *   a payment-request note). It is COSMETIC only and is NOT embedded on-chain —
 *   preserving the pre-#1037 behavior where the note was dropped, so a plain
 *   note like "lunch" never causes the send to fail.
 * - `bridgeDepositMemo` is the bridge deposit order memo: a 64-byte hex value
 *   (from the public order API, `BridgeOrder::generate_memo`) that IS embedded
 *   on the recipient output's encrypted `e_memo` so the bridge watcher can match
 *   the deposit to its mint order. Only the bridge ExportPanel sets this.
 *
 * Keeping them separate stops the free-text note from ever reaching the WASM
 * signer's strict 64-byte-hex validator (which is correct for the bridge memo
 * but would reject an ordinary note).
 */
export interface SendOptions {
  /** Human free-text note. Cosmetic; never embedded on-chain (#1037). */
  note?: string
  /** Bridge deposit order memo (64-byte hex), embedded on-chain (#1037). */
  bridgeDepositMemo?: string
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
  /**
   * Lock the wallet (#490): wipe the decrypted seed (`mnemonicRef`) and the
   * session vault key (`vaultKeyRef` + the module-scope `sessionVaultKey`) from
   * memory and set `isLocked: true`. Does NOT touch localStorage — the encrypted
   * wallet still exists on disk; unlocking re-derives via
   * {@link unlockWallet}. Balance/history polling is gated on `!isLocked`, so it
   * stops once locked.
   *
   * SAFETY: a PLAINTEXT (no-password) wallet has no key to unlock with, so
   * locking it would strand it in a state it could never leave. For a plaintext
   * wallet this is a NO-OP — the caller (and the auto-lock timer) must gate on
   * `isEncrypted` so a plaintext wallet is never locked.
   */
  lockWallet: () => void
  exportWallet: (password?: string) => Promise<string | null>
  resetWallet: () => void

  /**
   * Idle auto-lock timeout in MINUTES, persisted in localStorage (#490). `0`
   * means "Off/Never" (no auto-lock). Auto-lock only applies to encrypted
   * wallets — a plaintext wallet is never auto-locked.
   */
  autoLockMinutes: number
  /** Update + persist the idle auto-lock timeout (minutes; `0` = off). */
  setAutoLockMinutes: (minutes: number) => void

  /**
   * Set a password on a PLAINTEXT (no-password) wallet, upgrading it to
   * encrypted (#489). Re-saves the seed as an encrypted vault blob and re-wraps
   * the address book + outstanding claim links under the new password-derived
   * key, so contacts and claim links work and nothing remains in cleartext.
   * Rejects if the wallet is already encrypted or locked.
   */
  setPassword: (newPassword: string) => Promise<void>

  /**
   * Rotate the password of an ENCRYPTED wallet (#489). Verifies the old password,
   * re-encrypts the seed under the new password, and re-wraps the address book +
   * outstanding claim links under the new key. The OLD password no longer
   * decrypts anything afterward. Rejects with "Incorrect current password" if
   * the old password is wrong, or if the wallet is plaintext/locked.
   */
  changePassword: (oldPassword: string, newPassword: string) => Promise<void>

  /**
   * The unlocked vault key for the current session, or null when the wallet is
   * locked or stored in plaintext. Sibling features — claim-link secrets (#474)
   * and the encrypted address book (#476) — use this to encrypt/decrypt their
   * data under the SAME password-derived key while the wallet is unlocked. The
   * key lives in memory only and is cleared on reset/refresh.
   */
  getVaultKey: () => VaultKey | null

  // Transactions
  send: (to: string, amount: bigint, options?: SendOptions) => Promise<string>
  /**
   * Estimate the fee for a send, returning both the fee and the node-computed
   * cluster fee factor display string (#635). Mirrors the {@link send} flow:
   * derives the wallet's cluster wealth from its owned output target keys and
   * forwards it to the node so the progressive fee factor is applied. Falls back
   * to a base-rate `{ fee: 0n, clusterFactorDisplay: '1.00x' }` on any failure
   * (locked wallet, not connected, network error) so the send modal never
   * crashes on the pre-send fee display.
   */
  estimateFee: (amount: bigint) => Promise<FeeEstimate>
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
        // Seed so the scan detects 6.0.0 hybrid incoming payments + change (#988).
        seed: mnemonicToSeedHex(mnemonic),
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
        // Seed so hybrid receives + change appear in history (#988).
        seed: mnemonicToSeedHex(mnemonic),
      },
      {
        getChainHeight: () => adapter.getBlockHeight(),
        getOutputsWithMeta: (start, end) => adapter.getRawOutputsWithMeta(start, end),
        areKeyImagesSpent: (keyImages) => adapter.areKeyImagesSpent(keyImages),
      },
    )
    // Collapse per-output entries into per-event rows (#675): unique ids (no
    // duplicate React keys), sends netted against same-block change, and a
    // real pending/confirmed status instead of a hardcoded one.
    const chainHeight = await adapter.getBlockHeight()
    return netOwnedHistory(entries).map((e) => ({
      id: e.id,
      type: e.type,
      amount: e.amount,
      // Fee is not knowable client-side (the ring hides the consuming tx);
      // 0n is the type's "unknown" and the row does not render it.
      fee: 0n,
      privacyLevel: 'private' as const,
      cryptoType: 'clsag' as const,
      status: e.status,
      // Block timestamps are not exposed by the outputs RPC. 0 marks "no
      // wall-clock time known": the row falls back to showing the block
      // height instead of fabricating "just now" on every refresh (#675).
      timestamp: 0,
      blockHeight: e.blockHeight > 0 ? e.blockHeight : undefined,
      confirmations:
        e.status === 'confirmed' && e.blockHeight > 0
          ? Math.max(0, chainHeight - e.blockHeight + 1)
          : 0,
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

// Session vault key holder for at-rest encryption of sibling data (#474, #476).
// The wallet context keeps this in sync with `vaultKeyRef.current` on every
// unlock/create/import/reset so the module-scope stores can read the key lazily.
// Null while locked or for a legacy plaintext wallet.
let sessionVaultKey: VaultKey | null = null
function setSessionVaultKey(key: VaultKey | null): void {
  sessionVaultKey = key
}

// Claim-link bearer secrets are encrypted at rest under the session vault key
// (#474). When locked (no key), the store reads as empty and refuses to write
// plaintext secrets — records become available again on unlock.
const claimLinkStore = new ClaimLinkStore(new EncryptedClaimLinks(() => sessionVaultKey))

// The address book (counterparty graph + annotations) is encrypted at rest
// under the same session vault key (#476). When there is no key (locked /
// plaintext wallet) it reads as empty and silently does NOT persist (no
// plaintext contact graph, no throw) — contacts become available and persist
// again once the wallet has a password and is unlocked.
const addressBook = new AddressBook(new EncryptedAddressBook(() => sessionVaultKey))

/** Polling interval when WebSocket is disconnected (30 seconds) */
const FALLBACK_POLL_INTERVAL = 30000

/** localStorage key for the idle auto-lock timeout preference (#490). */
const STORAGE_AUTO_LOCK = 'botho-auto-lock-minutes'

/** Default idle auto-lock timeout in minutes (#490). `0` would mean off. */
const DEFAULT_AUTO_LOCK_MINUTES = 15

/**
 * Read the persisted idle auto-lock timeout (minutes) from localStorage (#490).
 * Returns {@link DEFAULT_AUTO_LOCK_MINUTES} when unset; `0` means off/never.
 * Guards against non-numeric / negative junk.
 */
function loadAutoLockMinutes(): number {
  try {
    const raw = localStorage.getItem(STORAGE_AUTO_LOCK)
    if (raw === null) return DEFAULT_AUTO_LOCK_MINUTES
    const n = Number(raw)
    if (!Number.isFinite(n) || n < 0) return DEFAULT_AUTO_LOCK_MINUTES
    return Math.floor(n)
  } catch {
    return DEFAULT_AUTO_LOCK_MINUTES
  }
}

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
 * Which address-string network prefix the wallet emits (`botho://2/` vs
 * `tbotho://2/`). The live deployment is testnet; this preserves the address
 * network the wallet used before v2 (the old `deriveAddress` defaulted to
 * testnet). Kept as a single named constant so a future mainnet cutover flips
 * one place.
 */
const ADDRESS_NETWORK: 'mainnet' | 'testnet' = 'testnet'

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

  // Idle auto-lock timeout preference in minutes (#490); 0 = off/never.
  // Persisted to localStorage so it survives refresh.
  const [autoLockMinutes, setAutoLockMinutesState] = useState<number>(() => loadAutoLockMinutes())

  /**
   * Set the session vault key in memory AND publish it to the module-scope
   * holder that the encrypted claim-link store reads (#474). When a key becomes
   * available (unlock/create/import), reload the claim links so records that were
   * unavailable while locked — and any legacy plaintext records needing
   * re-wrapping — are loaded/migrated. Passing `null` clears the key, after which
   * the encrypted store reads as empty.
   */
  const applyVaultKey = useCallback(async (key: VaultKey | null) => {
    vaultKeyRef.current = key
    setSessionVaultKey(key)
    try {
      await claimLinkStore.load()
    } catch {
      // Locked / no key: store degrades to empty rather than throwing.
    }
    // The address book is encrypted under the same key (#476): reload so a
    // newly-available key surfaces (and migrates legacy plaintext) contacts,
    // and a null key clears them from view.
    try {
      await addressBook.load()
    } catch {
      // Locked / no key: store degrades to empty rather than throwing.
    }
    setState(s => ({
      ...s,
      claimLinks: claimLinkStore.getAll(),
      contacts: addressBook.getAll(),
    }))
  }, [])

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

    // Derive the wallet's v2 (`botho://2/…`) address — carries the account's
    // post-quantum keys, derived node-identically in wasm — then persist it with
    // the seed so `Receive`/storage show the v2 address the node can pay.
    const address = await deriveV2Address(mnemonic, ADDRESS_NETWORK)
    await saveWallet(mnemonic, password, address)

    // Store mnemonic in memory
    mnemonicRef.current = mnemonic
    // Derive + hold the session vault key (bound to the seed blob's salt) so
    // sibling data can be encrypted under the same key (#474/#476). Null for
    // plaintext wallets. Publishing the key migrates+loads claim links.
    await applyVaultKey(password ? await deriveSessionVaultKey(password) : null)

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

    // Derive the wallet's v2 address (post-quantum keys derived in wasm) and
    // persist it with the seed.
    const address = await deriveV2Address(normalized, ADDRESS_NETWORK)
    await saveWallet(normalized, password, address)

    // Store mnemonic in memory
    mnemonicRef.current = normalized
    await applyVaultKey(password ? await deriveSessionVaultKey(password) : null)

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

    // Store mnemonic + session vault key in memory. Publishing the key loads +
    // migrates outstanding claim links now that we can decrypt them (#474).
    mnemonicRef.current = stored.mnemonic
    await applyVaultKey(stored.vaultKey)

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

  const lockWallet = useCallback(() => {
    // SAFETY (#490): never lock a wallet that cannot be unlocked. A plaintext
    // (no-password) wallet has no vault key, so there is nothing to unlock with —
    // locking it would strand it. Gate on the in-memory key: it is non-null only
    // for an unlocked, encrypted wallet.
    if (vaultKeyRef.current === null) {
      return
    }
    // Wipe the decrypted seed from memory.
    mnemonicRef.current = null
    // Wipe the session vault key (vaultKeyRef + module-scope sessionVaultKey).
    // Passing null also makes the encrypted claim-link store + address book read
    // as empty until the next unlock re-derives the key.
    void applyVaultKey(null)
    // Show the unlock screen and stop balance/history polling (gated on
    // !isLocked). Clear the in-memory balance/history so nothing sensitive
    // lingers on screen behind the unlock view.
    setState(s => ({
      ...s,
      isLocked: true,
      balance: null,
      transactions: [],
    }))
  }, [applyVaultKey])

  const setAutoLockMinutes = useCallback((minutes: number) => {
    const m = Number.isFinite(minutes) && minutes > 0 ? Math.floor(minutes) : 0
    setAutoLockMinutesState(m)
    try {
      localStorage.setItem(STORAGE_AUTO_LOCK, String(m))
    } catch {
      // Best-effort persistence; the in-memory preference still applies.
    }
  }, [])

  // Idle auto-lock (#490): when the wallet is UNLOCKED + ENCRYPTED and the user
  // chose a timeout (> 0), lock it after `autoLockMinutes` of inactivity. Any
  // user activity (pointer/keyboard/click/scroll) or the tab regaining focus
  // resets the countdown; `visibilitychange` is wired so returning to the tab
  // restarts a fresh timer. All listeners + the timer are torn down on unmount,
  // when locked, when the wallet has no password, or when the preference is off.
  useEffect(() => {
    if (autoLockMinutes <= 0) return
    if (state.isLocked || !state.isEncrypted || !state.hasWallet) return

    const timeoutMs = autoLockMinutes * 60_000
    let timer: ReturnType<typeof setTimeout> | undefined

    const reset = () => {
      if (timer !== undefined) clearTimeout(timer)
      timer = setTimeout(() => {
        lockWallet()
      }, timeoutMs)
    }

    const onActivity = () => reset()
    const onVisibility = () => {
      if (document.visibilityState === 'visible') reset()
    }

    const activityEvents: Array<keyof WindowEventMap> = [
      'pointermove',
      'pointerdown',
      'keydown',
      'click',
      'scroll',
      'wheel',
      'touchstart',
    ]
    for (const ev of activityEvents) {
      window.addEventListener(ev, onActivity, { passive: true })
    }
    document.addEventListener('visibilitychange', onVisibility)

    // Start the initial countdown.
    reset()

    return () => {
      if (timer !== undefined) clearTimeout(timer)
      for (const ev of activityEvents) {
        window.removeEventListener(ev, onActivity)
      }
      document.removeEventListener('visibilitychange', onVisibility)
    }
  }, [autoLockMinutes, state.isLocked, state.isEncrypted, state.hasWallet, lockWallet])

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
    // Clear mnemonic + vault key from memory (also clears the module-scope key
    // the encrypted claim-link store reads).
    mnemonicRef.current = null
    vaultKeyRef.current = null
    setSessionVaultKey(null)
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

  /**
   * Swap the session vault key to `newKey` and RE-WRAP the sibling stores
   * (claim links + address book) under it ATOMICALLY, then publish state.
   *
   * Unlike {@link applyVaultKey} (which RELOADS the stores from disk — correct
   * for unlock, where the on-disk blobs are already under the key being applied),
   * this re-encrypts the CURRENT in-memory store contents under the new key. That
   * is exactly what a password change needs: the on-disk blobs are still under
   * the OLD key, so reloading would fail to decrypt and silently drop the data;
   * instead we persist the already-decrypted, in-memory data under the new key,
   * overwriting the old-key blobs. The stores must already be STRICT-loaded
   * (wallet unlocked, decrypt succeeded) before this is called.
   *
   * ATOMICITY / FUND SAFETY (#489 Judge feedback): the claim-link blob holds
   * bearer secrets (= funds), so a re-wrap failure must NEVER be swallowed.
   *   - The claim-link store is re-wrapped FIRST and its failure is FATAL (throws).
   *   - We snapshot both on-disk blobs before writing; if EITHER re-wrap throws,
   *     we restore the original blobs and the OLD session key, then re-throw so
   *     the caller ABORTS the whole rotation before the seed is re-saved. This
   *     guarantees we never leave a half-rotated state (seed-under-new-key while a
   *     bearer-secret blob is still under-old-key).
   * The session key is only left swapped to `newKey` on full success.
   *
   * On success it returns a `rollback()` the caller can invoke if a LATER step
   * (the seed re-save) fails, restoring the old blobs + old session key so the
   * inverse half-rotated state (stores-new while seed-old) is also avoided.
   */
  const rewrapUnderNewKey = useCallback(async (newKey: VaultKey): Promise<() => void> => {
    // Snapshot the on-disk blobs so we can roll back if a re-wrap fails.
    const prevClaimBlob = localStorage.getItem('botho-claim-links')
    const prevAddrBlob = localStorage.getItem('botho-address-book')
    const prevKey = vaultKeyRef.current

    const restore = () => {
      if (prevClaimBlob === null) localStorage.removeItem('botho-claim-links')
      else localStorage.setItem('botho-claim-links', prevClaimBlob)
      if (prevAddrBlob === null) localStorage.removeItem('botho-address-book')
      else localStorage.setItem('botho-address-book', prevAddrBlob)
      vaultKeyRef.current = prevKey
      setSessionVaultKey(prevKey)
    }

    // Swap to the new key so the lazy-getter stores encrypt under it.
    vaultKeyRef.current = newKey
    setSessionVaultKey(newKey)

    try {
      // Re-wrap the BEARER-SECRET store first; its failure is fatal (never
      // swallowed — losing this blob loses funds).
      await claimLinkStore.rewrap()
      // Then the (non-bearer, privacy-only) address book.
      await addressBook.rewrap()
    } catch (err) {
      // Roll back blobs + session key so the wallet stays consistently on the
      // OLD password, then propagate to abort the rotation before the seed is
      // re-saved under the new password.
      restore()
      throw err
    }

    setState(s => ({
      ...s,
      claimLinks: claimLinkStore.getAll(),
      contacts: addressBook.getAll(),
    }))

    // Return a rollback for a later (seed re-save) failure.
    return restore
  }, [])

  const setPassword = useCallback(async (newPassword: string) => {
    if (state.isLocked) {
      throw new Error('Unlock the wallet before setting a password')
    }
    if (vaultKeyRef.current !== null) {
      throw new Error('Wallet already has a password. Use change password instead.')
    }
    const mnemonic = mnemonicRef.current
    if (!mnemonic) {
      throw new Error('No wallet loaded')
    }
    if (newPassword.length < MIN_PASSWORD_LENGTH) {
      throw new Error(`Password must be at least ${MIN_PASSWORD_LENGTH} characters`)
    }

    // 1. Re-save the seed ENCRYPTED under the new password (versioned vault blob).
    await saveWallet(mnemonic, newPassword)

    // 2. Derive the session vault key bound to the just-written seed blob's salt
    //    so the session key matches the seed exactly.
    const newKey = await deriveSessionVaultKey(newPassword)
    if (!newKey) {
      throw new Error('Failed to derive vault key')
    }

    // 3. Publish the key and RELOAD the sibling stores. For a plaintext wallet
    //    any on-disk sibling data is a LEGACY PLAINTEXT blob (pre-#474/#476);
    //    applyVaultKey's load() path migrates those to encrypted under the new
    //    key (the same automatic plaintext->encrypted re-wrap used on unlock), so
    //    contacts and claim links survive the upgrade and no cleartext remains.
    await applyVaultKey(newKey)

    setState(s => ({ ...s, isEncrypted: true, isLocked: false }))
  }, [state.isLocked, applyVaultKey])

  const changePassword = useCallback(async (oldPassword: string, newPassword: string) => {
    if (!state.isEncrypted) {
      throw new Error('Wallet has no password to change. Set a password instead.')
    }
    if (state.isLocked) {
      throw new Error('Unlock the wallet before changing the password')
    }
    if (newPassword.length < MIN_PASSWORD_LENGTH) {
      throw new Error(`Password must be at least ${MIN_PASSWORD_LENGTH} characters`)
    }

    // 1. Verify the old password by decrypting the stored seed with it. A wrong
    //    password throws "Incorrect password" from loadWallet; surface a clearer
    //    message and abort BEFORE re-writing anything.
    let mnemonic: string
    try {
      const stored = await loadWallet(oldPassword)
      if (!stored) throw new Error('No wallet found')
      mnemonic = stored.mnemonic
    } catch {
      throw new Error('Incorrect current password')
    }

    // Keep the in-memory mnemonic authoritative.
    mnemonicRef.current = mnemonic

    // 2. STRICT-load the sibling stores under the still-active OLD key, so the
    //    re-wrap re-encrypts the REAL decrypted data. loadStrict() THROWS if a
    //    present blob fails to decrypt (vs. the lenient load() that returns []),
    //    so a decrypt failure aborts the rotation here instead of silently
    //    re-wrapping an empty store over real bearer secrets (= fund loss).
    //    A genuinely empty/absent store loads as empty without throwing.
    try {
      await claimLinkStore.loadStrict()
      await addressBook.loadStrict()
    } catch {
      throw new Error(
        'Cannot change password: your saved data could not be decrypted with the current session. Unlock the wallet and try again.',
      )
    }

    // 3. Derive the NEW session vault key independently (fresh salt) — NOT from
    //    the seed blob, which is still under the OLD password. Each blob is
    //    self-describing (salt+iterations in its header), so this key decrypts
    //    the new sibling blobs directly and the new seed blob via salt-fallback.
    const newKey = await VaultKey.fromPassword(newPassword)

    // 4. ATOMICALLY re-wrap the bearer-secret claim links + the address book
    //    under the new key. This is the irreversible-but-recoverable step done
    //    BEFORE the seed re-save: if it throws, rewrapUnderNewKey has already
    //    restored the old blobs + old session key, so we abort WITHOUT re-saving
    //    the seed — the wallet stays consistently on the OLD password.
    const rollbackRewrap = await rewrapUnderNewKey(newKey)

    // 5. Only now — after the sibling re-wraps SUCCEEDED — perform the LAST
    //    irreversible step: re-save the seed ENCRYPTED under the new password.
    //    After this, the old password decrypts none of the three data types.
    //    If this final write fails, roll the sibling stores back to the OLD key
    //    so we don't leave the inverse half-rotated state (stores-new/seed-old).
    try {
      await saveWallet(mnemonic, newPassword)
    } catch (err) {
      rollbackRewrap()
      throw err
    }

    setState(s => ({ ...s, isEncrypted: true, isLocked: false }))
  }, [state.isEncrypted, state.isLocked, rewrapUnderNewKey])

  const send = useCallback(async (to: string, amount: bigint, options?: SendOptions): Promise<string> => {
    // Two DISTINCT channels (#1037): a human free-text `note` is cosmetic and is
    // intentionally NOT threaded into the signer (preserving pre-#1037 behavior,
    // where a note like "lunch" is dropped and never fails the send). Only the
    // bridge deposit order memo (64-byte hex) is embedded on-chain.
    const bridgeDepositMemo = options?.bridgeDepositMemo
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
    //
    //    Before estimating, derive the wallet's cluster wealth from its owned
    //    output target keys and pass it to estimateFee so the node applies the
    //    correct progressive fee factor (#626/#628/#634). Without this the node
    //    always returns the 1.00x base rate. Best-effort: any failure falls back
    //    to a zero cluster wealth (base-rate estimate) rather than blocking the
    //    send.
    const MIN_TX_FEE = 100_000_000n // signer's MIN_TX_FEE (picocredits)
    let clusterWealth = 0n
    try {
      const targetKeys = await ownedOutputTargetKeys(
        {
          spendPrivateKey: toHex(kp.spendPrivate),
          viewPrivateKey: toHex(kp.viewPrivate),
          // Seed so hybrid outputs count toward cluster identity (#988).
          seed: mnemonicToSeedHex(mnemonic),
        },
        {
          getChainHeight: () => adapter.getBlockHeight(),
          getOutputs: (start, end) => adapter.getRawOutputs(start, end),
          areKeyImagesSpent: (keyImages) => adapter.areKeyImagesSpent(keyImages),
        },
      )
      clusterWealth = await adapter.getClusterWealth(targetKeys)
    } catch {
      clusterWealth = 0n
    }
    let fee: bigint
    try {
      fee = (await adapter.estimateFee(0, clusterWealth)).fee
    } catch {
      fee = 0n
    }
    if (fee < MIN_TX_FEE) {
      fee = MIN_TX_FEE
    }

    // 4. Build + CLSAG-sign entirely client-side (wasm). The keys never leave
    //    the browser; only the signed bytes are submitted. Every output is a
    //    hybrid post-quantum output (6.0.0, #978): the recipient output
    //    encapsulates against the recipient's published ML-KEM key
    //    (recipientKeys.kemPublic), and the change output against the wallet's
    //    OWN ML-KEM key (derived from the mnemonic).
    const senderKemPublicKey = await deriveKemPublicKey(mnemonic)
    const { txHex } = await buildSendTransaction({
      keys: {
        spendPrivateKey: toHex(kp.spendPrivate),
        viewPrivateKey: toHex(kp.viewPrivate),
        // Seed so the send's own scan/spent-filter sees hybrid inputs + change (#988).
        seed: mnemonicToSeedHex(mnemonic),
      },
      recipient: {
        spend_public_key: toHex(recipientKeys.spendPublic),
        view_public_key: toHex(recipientKeys.viewPublic),
        kem_public_key: toHex(recipientKeys.kemPublic),
      },
      senderKemPublicKey,
      amount,
      fee,
      // Bridge deposit order memo: for a BTH→wBTH bridge deposit this is the
      // mint order memo (64-byte hex), embedded on-chain so the bridge watcher
      // can match the deposit to its order (#1037). Undefined for an ordinary
      // send or a free-text note.
      bridgeDepositMemo,
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

  // Pre-send fee estimate for the send modal (#635). Mirrors the cluster-wealth
  // derivation in `send` above so the displayed fee — and the node-computed
  // `clusterFactorDisplay` that explains it — match what the actual send will
  // pay. Best-effort throughout: a locked/disconnected wallet, or any network
  // failure, resolves to the base-rate `{ fee: 0n, clusterFactorDisplay:
  // '1.00x' }` rather than throwing into the modal's render effect.
  const estimateFee = useCallback(async (_amount: bigint): Promise<FeeEstimate> => {
    const adapter = adapterRef.current
    const fallback: FeeEstimate = { fee: 0n, clusterFactorDisplay: '1.00x' }
    if (!adapter.isConnected()) return fallback

    const mnemonic = mnemonicRef.current
    if (!mnemonic) return fallback

    // Derive the wallet's cluster wealth from its owned output target keys, the
    // same way `send` does, so the node applies the correct progressive fee
    // factor (#626/#628/#634). Any failure falls back to a zero cluster wealth
    // (base-rate estimate).
    let clusterWealth = 0n
    try {
      const kp = deriveKeypairs(mnemonic, 0)
      const targetKeys = await ownedOutputTargetKeys(
        {
          spendPrivateKey: toHex(kp.spendPrivate),
          viewPrivateKey: toHex(kp.viewPrivate),
          // Seed so hybrid outputs count toward cluster identity (#988).
          seed: mnemonicToSeedHex(mnemonic),
        },
        {
          getChainHeight: () => adapter.getBlockHeight(),
          getOutputs: (start, end) => adapter.getRawOutputs(start, end),
          areKeyImagesSpent: (keyImages) => adapter.areKeyImagesSpent(keyImages),
        },
      )
      clusterWealth = await adapter.getClusterWealth(targetKeys)
    } catch {
      clusterWealth = 0n
    }

    try {
      return await adapter.estimateFee(0, clusterWealth)
    } catch {
      return fallback
    }
  }, [])

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

    // Per-link amount cap (#589): a claim link is a bearer instrument whose
    // secret lingers in chat history — bound the loss by treating it like cash.
    // Reject an over-cap amount BEFORE any on-chain spend; large transfers
    // should use a request link instead.
    assertClaimLinkAmountWithinCap(amount)

    // CRITICAL: the claim-link bearer secret can only be persisted under a vault
    // key (encrypted at rest). A plaintext / no-password wallet has no session
    // vault key, so persisting the secret would throw AFTER funding the ephemeral
    // address — losing the funds (the bearer secret is the only key to them).
    // Fail fast here, BEFORE any on-chain spend, so no money can move.
    if (vaultKeyRef.current === null) {
      throw new Error(
        'Claim links require a password-protected wallet. Add a password to your wallet to send via link.',
      )
    }

    // 1. Generate the ephemeral wallet (the link's bearer secret) and its addr.
    const ephMnemonic = createClaimLinkMnemonic()
    const ephAddress = await deriveV2Address(ephMnemonic, ADDRESS_NETWORK)

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
        : 'https://botho.io'
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
        lockWallet,
        exportWallet,
        resetWallet,
        autoLockMinutes,
        setAutoLockMinutes,
        setPassword,
        changePassword,
        getVaultKey,
        send,
        estimateFee,
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
