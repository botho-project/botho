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
  deriveV2Address: vi.fn(async (mnemonic: string) => {
    // Deterministic per-mnemonic stand-in for the wasm v2 deriver: base58-only
    // body so `/^tbotho:\/\/2\/[A-Za-z1-9]+$/` matches and different
    // mnemonics yield different addresses.
    const b58 = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz123456789'
    let acc = ''
    for (let i = 0; i < mnemonic.length && acc.length < 40; i++) {
      acc += b58[mnemonic.charCodeAt(i) % b58.length]
    }
    return 'tbotho://2/' + (acc || '11111')
  }),
  spendableBalance: vi.fn().mockResolvedValue(0n),
  buildOwnedHistory: vi.fn().mockResolvedValue([]),
  buildSendTransaction: vi.fn().mockResolvedValue({ txHex: '0xstub' }),
  deriveKemPublicKey: vi.fn().mockResolvedValue('00'.repeat(1184)),
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
// A stand-in ephemeral bearer mnemonic for a claim link (its loss = fund loss).
const EPH_MNEMONIC =
  'legal winner thank year wave sausage worth useful legal winner thank yellow'
const EPH_ADDRESS = 'tbotho://1/ephaddrxyz'

const STORAGE_SEED = 'botho-wallet-mnemonic'
const STORAGE_ENCRYPTED = 'botho-wallet-encrypted'
const STORAGE_ADDRESS_BOOK = 'botho-address-book'
const STORAGE_CLAIM_LINKS = 'botho-claim-links'

/** A serialized ClaimLinkRecord array as stored on disk (amount as string). */
function serializedClaimLink() {
  return [
    {
      id: 'cl1',
      ephMnemonic: EPH_MNEMONIC,
      ephAddress: EPH_ADDRESS,
      amount: '123456789',
      createdAt: 100,
      fundingTxHash: '0xfund',
      status: 'outstanding',
    },
  ]
}

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

  it('changePassword rotates the password; the old password no longer decrypts and a CLAIM LINK (bearer secret) and contact survive re-wrap', async () => {
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

    // Persist a REAL claim-link record (with its ephemeral bearer mnemonic)
    // ENCRYPTED under the OLD key, exactly as sendViaLink would have. We write
    // the on-disk blob directly so changePassword's strict pre-rewrap load reads
    // it into memory and re-wraps it (no node/funding needed in a unit test).
    const oldClBlob = await oldKey.encryptJSON(serializedClaimLink())
    localStorage.setItem(STORAGE_CLAIM_LINKS, oldClBlob)
    expect(isVaultBlob(oldClBlob)).toBe(true)
    // Sanity: the OLD key decrypts the bearer secret pre-rotation.
    const clBefore = await oldKey.decryptJSON<Array<{ ephMnemonic: string }>>(oldClBlob)
    expect(clBefore[0].ephMnemonic).toBe(EPH_MNEMONIC)

    // CHANGE the password.
    await act(async () => { await get().changePassword('oldpassword', 'newpassword') })
    expect(get().isEncrypted).toBe(true)

    // Seed is re-encrypted: loadWallet with the OLD password must now FAIL, and
    // with the NEW password must succeed.
    const { loadWallet } = await import('@botho/core')
    await expect(loadWallet('oldpassword')).rejects.toThrow(/incorrect password/i)
    const reloaded = await loadWallet('newpassword')
    expect(reloaded?.mnemonic).toBe(TEST_MNEMONIC_12)

    const newKey = get().getVaultKey()!

    // Address book is re-wrapped under the NEW key: a freshly derived OLD key can
    // no longer decrypt it, but the NEW session key can.
    const newAbBlob = localStorage.getItem(STORAGE_ADDRESS_BOOK)!
    expect(isVaultBlob(newAbBlob)).toBe(true)
    const after = await newKey.decryptJSON<Array<{ name: string }>>(newAbBlob)
    expect(after.map((c) => c.name)).toContain('Bob')

    // CLAIM LINK (= funds) re-wrap: the blob changed and the NEW key decrypts the
    // FULL record, including its ephemeral bearer mnemonic, intact.
    const newClBlob = localStorage.getItem(STORAGE_CLAIM_LINKS)!
    expect(isVaultBlob(newClBlob)).toBe(true)
    expect(newClBlob).not.toBe(oldClBlob)
    expect(newClBlob).not.toContain(EPH_MNEMONIC) // no plaintext bearer secret
    const clAfter = await newKey.decryptJSON<
      Array<{ id: string; ephMnemonic: string; ephAddress: string; amount: string; status: string }>
    >(newClBlob)
    expect(clAfter).toHaveLength(1)
    expect(clAfter[0].id).toBe('cl1')
    expect(clAfter[0].ephMnemonic).toBe(EPH_MNEMONIC)
    expect(clAfter[0].ephAddress).toBe(EPH_ADDRESS)
    expect(clAfter[0].amount).toBe('123456789')
    expect(clAfter[0].status).toBe('outstanding')

    // The OLD key (re-derived against the NEW blobs) can NO LONGER read either
    // the address book or the bearer secret.
    const staleAb = await VaultKey.fromPasswordAndBlob('oldpassword', newAbBlob)
    await expect(staleAb.decryptJSON(newAbBlob)).rejects.toThrow()
    const staleCl = await VaultKey.fromPasswordAndBlob('oldpassword', newClBlob)
    await expect(staleCl.decryptJSON(newClBlob)).rejects.toThrow()

    // The claim link is also visible in context state under the new key.
    expect(get().claimLinks.map((r) => r.ephMnemonic)).toContain(EPH_MNEMONIC)
  })

  it('changePassword ABORTS if a store re-wrap fails: the wallet stays on the OLD password with bearer secrets intact', async () => {
    const { get } = await mountWallet()

    await act(async () => { await get().createWallet(TEST_MNEMONIC_12, 'oldpassword') })
    const oldKey = get().getVaultKey()!
    await act(async () => { await get().addContact('Bob', A_CONTACT) })

    // Persist a real claim link under the OLD key.
    const oldClBlob = await oldKey.encryptJSON(serializedClaimLink())
    localStorage.setItem(STORAGE_CLAIM_LINKS, oldClBlob)
    const oldAbBlob = localStorage.getItem(STORAGE_ADDRESS_BOOK)!
    const oldSeedBlob = localStorage.getItem(STORAGE_SEED)!

    // Force the FINAL irreversible step (seed re-save) to fail AFTER the store
    // re-wraps succeed, exercising the rollback path. setItem on the seed key
    // throws; all other keys behave normally.
    const realSet = localStorage.setItem.bind(localStorage)
    const setSpy = vi
      .spyOn(localStorage, 'setItem')
      .mockImplementation((k: string, v: string) => {
        if (k === STORAGE_SEED) throw new Error('disk full')
        realSet(k, v)
      })

    await act(async () => {
      await expect(get().changePassword('oldpassword', 'newpassword')).rejects.toThrow()
    })

    setSpy.mockRestore()

    // ROLLBACK: the wallet remains on the OLD password.
    //  - seed blob is unchanged and the OLD password still decrypts it.
    expect(localStorage.getItem(STORAGE_SEED)).toBe(oldSeedBlob)
    const { loadWallet } = await import('@botho/core')
    const stored = await loadWallet('oldpassword')
    expect(stored?.mnemonic).toBe(TEST_MNEMONIC_12)
    await expect(loadWallet('newpassword')).rejects.toThrow(/incorrect password/i)

    //  - sibling blobs were restored to the OLD-key versions, so the OLD key
    //    still reads the contact graph AND the bearer secret (no fund loss).
    expect(localStorage.getItem(STORAGE_ADDRESS_BOOK)).toBe(oldAbBlob)
    expect(localStorage.getItem(STORAGE_CLAIM_LINKS)).toBe(oldClBlob)
    const ab = await oldKey.decryptJSON<Array<{ name: string }>>(oldAbBlob)
    expect(ab.map((c) => c.name)).toContain('Bob')
    const cl = await oldKey.decryptJSON<Array<{ ephMnemonic: string }>>(oldClBlob)
    expect(cl[0].ephMnemonic).toBe(EPH_MNEMONIC)

    //  - the live session key is the OLD key (rolled back), so the wallet stays
    //    usable: it can still decrypt the on-disk bearer secret.
    const liveKey = get().getVaultKey()!
    const liveCl = await liveKey.decryptJSON<Array<{ ephMnemonic: string }>>(
      localStorage.getItem(STORAGE_CLAIM_LINKS)!,
    )
    expect(liveCl[0].ephMnemonic).toBe(EPH_MNEMONIC)
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
