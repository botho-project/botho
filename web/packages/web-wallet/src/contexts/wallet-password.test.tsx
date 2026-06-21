/**
 * @vitest-environment jsdom
 *
 * Tests for set/change password on the wallet context (#489).
 *
 * Unlike wallet.test.tsx, this file intentionally does NOT mock @botho/core's
 * AddressBook / ClaimLinkStore: the whole point is to verify the REAL re-wrap of
 * the seed, address book, and claim-link records under the new vault key, using
 * the real EncryptedAddressBook / EncryptedClaimLinks + VaultKey crypto. Only
 * the network adapter and the wasm signer are stubbed.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, waitFor, act, cleanup } from '@testing-library/react'
import { WalletProvider, useWallet } from './wallet'
import { VaultKey, isVaultBlob } from '@botho/core'

// Stub the wasm signer so the module import is hermetic (no wasm artifact on a
// fresh checkout). None of these tests exercise real signing/scanning.
vi.mock('@botho/wasm-signer', () => ({
  spendableBalance: vi.fn().mockResolvedValue(0n),
  buildOwnedHistory: vi.fn().mockResolvedValue([]),
  buildSendTransaction: vi.fn().mockResolvedValue({ txHex: '0xstub' }),
}))

// Real-backing localStorage mock so the encrypted stores actually persist blobs
// we can inspect.
const localStorageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => { store[key] = value },
    removeItem: (key: string) => { delete store[key] },
    clear: () => { store = {} },
    _dump: () => ({ ...store }),
  }
})()
Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock })

// Stub the adapter (no real node).
vi.mock('@botho/adapters', () => ({
  RemoteNodeAdapter: class MockRemoteNodeAdapter {
    connect = vi.fn().mockResolvedValue(undefined)
    disconnect = vi.fn()
    isConnected = vi.fn().mockReturnValue(false)
    getNodeInfo = vi.fn().mockReturnValue({ version: '1.0.0', network: 'testnet' })
    getBalance = vi.fn().mockResolvedValue({ available: 0n, pending: 0n, total: 0n })
    getTransactionHistory = vi.fn().mockResolvedValue([])
    onNewBlock = vi.fn().mockReturnValue(() => {})
    onTransaction = vi.fn().mockReturnValue(() => {})
    onMempoolUpdate = vi.fn().mockReturnValue(() => {})
    onPeerStatus = vi.fn().mockReturnValue(() => {})
    getWsStatus = vi.fn().mockReturnValue('disconnected')
    onWsStatusChange = vi.fn().mockReturnValue(() => {})
  },
}))

const TEST_MNEMONIC_12 =
  'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about'
const A_CONTACT = 'tbotho://1/contactaddrabc'

const STORAGE_SEED = 'botho-wallet-mnemonic'
const STORAGE_ENCRYPTED = 'botho-wallet-encrypted'
const STORAGE_ADDRESS_BOOK = 'botho-address-book'

function Harness({ onMount }: { onMount: (w: ReturnType<typeof useWallet>) => void }) {
  const wallet = useWallet()
  onMount(wallet)
  return null
}

async function mountWallet(): Promise<{ get: () => ReturnType<typeof useWallet> }> {
  let ref: ReturnType<typeof useWallet> | null = null
  render(
    <WalletProvider>
      <Harness onMount={(w) => { ref = w }} />
    </WalletProvider>
  )
  await waitFor(() => expect(ref).not.toBeNull())
  return { get: () => ref! }
}

describe('wallet password set/change (#489)', () => {
  beforeEach(() => {
    localStorageMock.clear()
    vi.clearAllMocks()
  })
  afterEach(() => {
    cleanup()
    localStorageMock.clear()
  })

  it('setPassword on a plaintext wallet encrypts the seed, re-wraps the address book, and sets the session key', async () => {
    const { get } = await mountWallet()

    // Create a PLAINTEXT wallet (no password => no session vault key).
    await act(async () => { await get().createWallet(TEST_MNEMONIC_12) })
    expect(get().isEncrypted).toBe(false)
    // Seed is stored in cleartext for a plaintext wallet.
    expect(localStorage.getItem(STORAGE_ENCRYPTED)).toBe('false')
    expect(get().getVaultKey()).toBeNull()

    // Add a contact while plaintext. With no key the encrypted address book does
    // not persist, so seed a legacy PLAINTEXT address-book blob directly to model
    // a pre-#476 wallet, then reload so the context holds it in memory.
    localStorage.setItem(
      STORAGE_ADDRESS_BOOK,
      JSON.stringify([
        { id: 'c1', name: 'Alice', address: A_CONTACT, createdAt: 1, updatedAt: 1, txCount: 0 },
      ])
    )
    // Confirm it is plaintext (not a vault blob) before the upgrade.
    expect(isVaultBlob(localStorage.getItem(STORAGE_ADDRESS_BOOK)!)).toBe(false)

    // SET a password (plaintext -> encrypted).
    await act(async () => { await get().setPassword('hunter2pw') })

    // 1) Wallet is now encrypted; seed blob is a versioned vault blob.
    expect(get().isEncrypted).toBe(true)
    expect(localStorage.getItem(STORAGE_ENCRYPTED)).toBe('true')
    const seedBlob = localStorage.getItem(STORAGE_SEED)!
    expect(isVaultBlob(seedBlob)).toBe(true)
    expect(seedBlob).not.toContain('abandon') // no plaintext mnemonic left

    // 2) Session key is set and decrypts the seed back to the mnemonic.
    const key = get().getVaultKey()
    expect(key).not.toBeNull()
    const seed = await key!.decryptString(seedBlob)
    expect(seed).toBe(TEST_MNEMONIC_12)

    // 3) Address book is re-wrapped: the blob is now an encrypted vault blob with
    //    no plaintext contact data, and decrypts under the new key.
    const abBlob = localStorage.getItem(STORAGE_ADDRESS_BOOK)!
    expect(isVaultBlob(abBlob)).toBe(true)
    expect(abBlob).not.toContain('Alice')
    expect(abBlob).not.toContain(A_CONTACT)
    const contacts = await key!.decryptJSON<Array<{ name: string; address: string }>>(abBlob)
    expect(contacts.map((c) => c.name)).toContain('Alice')
  })

  it('changePassword rotates the password; the old password no longer decrypts and a claim link survives re-wrap', async () => {
    const { get } = await mountWallet()

    // Create an ENCRYPTED wallet with an initial password.
    await act(async () => { await get().createWallet(TEST_MNEMONIC_12, 'oldpassword') })
    expect(get().isEncrypted).toBe(true)
    const oldKey = get().getVaultKey()!

    // Add a contact (persists encrypted under the old key).
    await act(async () => { await get().addContact('Bob', A_CONTACT) })
    const oldAbBlob = localStorage.getItem(STORAGE_ADDRESS_BOOK)!
    expect(isVaultBlob(oldAbBlob)).toBe(true)
    // The OLD key decrypts the address book pre-rotation.
    const before = await oldKey.decryptJSON<Array<{ name: string }>>(oldAbBlob)
    expect(before.map((c) => c.name)).toContain('Bob')

    // CHANGE the password.
    await act(async () => { await get().changePassword('oldpassword', 'newpassword') })
    expect(get().isEncrypted).toBe(true)

    // Seed is re-encrypted: loadWallet with the OLD password must now FAIL, and
    // with the NEW password must succeed.
    const { loadWallet } = await import('@botho/core')
    await expect(loadWallet('oldpassword')).rejects.toThrow(/incorrect password/i)
    const reloaded = await loadWallet('newpassword')
    expect(reloaded?.mnemonic).toBe(TEST_MNEMONIC_12)

    // Address book is re-wrapped under the NEW key: a freshly derived OLD key can
    // no longer decrypt it, but the NEW session key can.
    const newAbBlob = localStorage.getItem(STORAGE_ADDRESS_BOOK)!
    expect(isVaultBlob(newAbBlob)).toBe(true)
    const newKey = get().getVaultKey()!
    const after = await newKey.decryptJSON<Array<{ name: string }>>(newAbBlob)
    expect(after.map((c) => c.name)).toContain('Bob')

    // A key derived from the OLD password against the NEW blob must NOT decrypt it.
    const staleOldKey = await VaultKey.fromPasswordAndBlob('oldpassword', newAbBlob)
    await expect(staleOldKey.decryptJSON(newAbBlob)).rejects.toThrow()
  })

  it('changePassword with the wrong current password is rejected and does not rotate', async () => {
    const { get } = await mountWallet()

    await act(async () => { await get().createWallet(TEST_MNEMONIC_12, 'oldpassword') })
    const seedBefore = localStorage.getItem(STORAGE_SEED)

    await act(async () => {
      await expect(get().changePassword('WRONGpassword', 'newpassword')).rejects.toThrow(
        /incorrect current password/i
      )
    })

    // Nothing was rotated: the original password still decrypts the seed and the
    // seed blob is unchanged.
    expect(localStorage.getItem(STORAGE_SEED)).toBe(seedBefore)
    const { loadWallet } = await import('@botho/core')
    const stored = await loadWallet('oldpassword')
    expect(stored?.mnemonic).toBe(TEST_MNEMONIC_12)
  })
})
