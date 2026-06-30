import { describe, it, expect, beforeEach } from 'vitest'
import {
  ClaimLinkStore,
  LocalStorageClaimLinks,
  EncryptedClaimLinks,
  ClaimLinksLockedError,
  CLAIM_LINK_EXPIRY_WINDOW_SECONDS,
  type ClaimLinkRecord,
} from './claim-links'
import { VaultKey, isVaultBlob } from '../wallet/vault'

const localStorageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => { store[key] = value },
    removeItem: (key: string) => { delete store[key] },
    clear: () => { store = {} },
  }
})()

Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock })

const SAMPLE = {
  ephMnemonic:
    'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
  ephAddress: 'tbotho://1/sample',
  amount: 5_000_000_000_000n,
}

describe('ClaimLinkStore', () => {
  beforeEach(() => {
    localStorageMock.clear()
  })

  it('adds and lists records', async () => {
    const store = new ClaimLinkStore()
    await store.load()
    const rec = await store.add(SAMPLE)
    expect(rec.status).toBe('outstanding')
    expect(rec.id).toBeTruthy()
    expect(store.getAll()).toHaveLength(1)
    expect(store.getAll()[0].amount).toBe(SAMPLE.amount)
  })

  it('persists bigint amounts across reload (round-trip serialization)', async () => {
    const store1 = new ClaimLinkStore()
    await store1.load()
    await store1.add(SAMPLE)

    const store2 = new ClaimLinkStore()
    await store2.load()
    const all = store2.getAll()
    expect(all).toHaveLength(1)
    expect(all[0].amount).toBe(SAMPLE.amount)
    expect(typeof all[0].amount).toBe('bigint')
  })

  it('updates status', async () => {
    const store = new ClaimLinkStore()
    await store.load()
    const rec = await store.add(SAMPLE)
    await store.setStatus(rec.id, 'claimed')
    expect(store.getAll()[0].status).toBe('claimed')
  })

  it('deletes records', async () => {
    const store = new ClaimLinkStore()
    await store.load()
    const rec = await store.add(SAMPLE)
    await store.delete(rec.id)
    expect(store.getAll()).toHaveLength(0)
  })

  it('sorts newest first', async () => {
    const store = new ClaimLinkStore()
    await store.load()
    await store.add({ ...SAMPLE, createdAt: 100 })
    await store.add({ ...SAMPLE, ephAddress: 'tbotho://1/newer', createdAt: 200 })
    const all = store.getAll()
    expect(all[0].createdAt).toBe(200)
    expect(all[1].createdAt).toBe(100)
  })

  it('detects expiry against the UX window', async () => {
    const store = new ClaimLinkStore()
    const now = 1_000_000
    const fresh: ClaimLinkRecord = {
      id: 'a', ...SAMPLE, createdAt: now - 10, status: 'outstanding',
    }
    const old: ClaimLinkRecord = {
      id: 'b', ...SAMPLE, createdAt: now - CLAIM_LINK_EXPIRY_WINDOW_SECONDS - 1, status: 'outstanding',
    }
    expect(store.isExpired(fresh, now)).toBe(false)
    expect(store.isExpired(old, now)).toBe(true)
  })

  it('tolerates corrupt localStorage data', async () => {
    localStorage.setItem('botho-claim-links', 'not json')
    const storage = new LocalStorageClaimLinks()
    expect(await storage.load()).toEqual([])
  })
})

// ---------------------------------------------------------------------------
// EncryptedClaimLinks (#474): bearer secrets encrypted at rest under VaultKey
// ---------------------------------------------------------------------------

const STORAGE_KEY = 'botho-claim-links'

describe('EncryptedClaimLinks', () => {
  beforeEach(() => {
    localStorageMock.clear()
  })

  it('round-trips a record WITHOUT writing the mnemonic in plaintext', async () => {
    const key = await VaultKey.fromPassword('correct horse battery staple')
    const store = new ClaimLinkStore(new EncryptedClaimLinks(() => key))
    await store.load()
    await store.add(SAMPLE)

    // The persisted blob must be a versioned vault blob and must NOT contain the
    // bearer mnemonic (or any of its words) in cleartext.
    const raw = localStorage.getItem(STORAGE_KEY)!
    expect(raw).toBeTruthy()
    expect(isVaultBlob(raw)).toBe(true)
    expect(raw).not.toContain('abandon')
    expect(raw).not.toContain(SAMPLE.ephMnemonic)
    expect(raw).not.toContain(SAMPLE.ephAddress)

    // A fresh store reading the same key decrypts it back faithfully.
    const store2 = new ClaimLinkStore(new EncryptedClaimLinks(() => key))
    await store2.load()
    const all = store2.getAll()
    expect(all).toHaveLength(1)
    expect(all[0].ephMnemonic).toBe(SAMPLE.ephMnemonic)
    expect(all[0].amount).toBe(SAMPLE.amount)
    expect(typeof all[0].amount).toBe('bigint')
  })

  it('migrates a legacy plaintext record to an encrypted blob on unlocked load', async () => {
    // Simulate a pre-#474 plaintext record sitting in localStorage.
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify([
        {
          id: 'legacy-1',
          ephMnemonic: SAMPLE.ephMnemonic,
          ephAddress: SAMPLE.ephAddress,
          amount: SAMPLE.amount.toString(),
          createdAt: 123,
          status: 'outstanding',
        },
      ]),
    )
    expect(isVaultBlob(localStorage.getItem(STORAGE_KEY)!)).toBe(false)

    const key = await VaultKey.fromPassword('pw-migrate-12345')
    const storage = new EncryptedClaimLinks(() => key)
    const records = await storage.load()

    // The records are returned (refund ability preserved)...
    expect(records).toHaveLength(1)
    expect(records[0].ephMnemonic).toBe(SAMPLE.ephMnemonic)
    expect(records[0].amount).toBe(SAMPLE.amount)

    // ...and the plaintext blob has been re-wrapped: no more cleartext secret.
    const raw = localStorage.getItem(STORAGE_KEY)!
    expect(isVaultBlob(raw)).toBe(true)
    expect(raw).not.toContain('abandon')
    expect(raw).not.toContain(SAMPLE.ephMnemonic)
  })

  it('reads as empty when locked (no key) without touching a legacy plaintext blob', async () => {
    // Legacy plaintext present, but the wallet is locked.
    const plaintext = JSON.stringify([
      {
        id: 'legacy-1',
        ephMnemonic: SAMPLE.ephMnemonic,
        ephAddress: SAMPLE.ephAddress,
        amount: SAMPLE.amount.toString(),
        createdAt: 123,
        status: 'outstanding',
      },
    ])
    localStorage.setItem(STORAGE_KEY, plaintext)

    // Encrypted blob present, but locked => unavailable, not a crash.
    const key = await VaultKey.fromPassword('pw-lock-test-1')
    const enc = new EncryptedClaimLinks(() => key)
    await enc.save([{ id: 'x', ...SAMPLE, createdAt: 1, status: 'outstanding' }])
    const encryptedBlob = localStorage.getItem(STORAGE_KEY)!

    const lockedStorage = new EncryptedClaimLinks(() => null)
    expect(await lockedStorage.load()).toEqual([])
    // The encrypted blob is untouched by a locked read.
    expect(localStorage.getItem(STORAGE_KEY)).toBe(encryptedBlob)

    // And a locked read of a plaintext blob also degrades to empty without
    // rewriting it (so a later unlock can migrate it).
    localStorage.setItem(STORAGE_KEY, plaintext)
    expect(await lockedStorage.load()).toEqual([])
    expect(localStorage.getItem(STORAGE_KEY)).toBe(plaintext)
  })

  it('refuses to write bearer secrets when locked (throws instead of cleartext)', async () => {
    const lockedStorage = new EncryptedClaimLinks(() => null)
    await expect(
      lockedStorage.save([{ id: 'x', ...SAMPLE, createdAt: 1, status: 'outstanding' }]),
    ).rejects.toBeInstanceOf(ClaimLinksLockedError)
    // Nothing was written in plaintext.
    expect(localStorage.getItem(STORAGE_KEY)).toBeNull()
  })

  it('supports the full refund flow after encryption (add -> setStatus refunded)', async () => {
    const key = await VaultKey.fromPassword('refund-flow-pw-1')
    const store = new ClaimLinkStore(new EncryptedClaimLinks(() => key))
    await store.load()
    const rec = await store.add(SAMPLE)
    expect(rec.status).toBe('outstanding')

    // Reload (e.g. after an unlock) and reclaim the link.
    const store2 = new ClaimLinkStore(new EncryptedClaimLinks(() => key))
    await store2.load()
    const reloaded = store2.getAll().find((r) => r.id === rec.id)!
    expect(reloaded).toBeTruthy()
    // The bearer secret needed to sweep the refund survived the round-trip.
    expect(reloaded.ephMnemonic).toBe(SAMPLE.ephMnemonic)
    await store2.setStatus(rec.id, 'refunded')

    const store3 = new ClaimLinkStore(new EncryptedClaimLinks(() => key))
    await store3.load()
    expect(store3.getAll().find((r) => r.id === rec.id)!.status).toBe('refunded')
    // Still encrypted at rest after the status update.
    expect(isVaultBlob(localStorage.getItem(STORAGE_KEY)!)).toBe(true)
  })

  // Confirm the #474 at-rest guarantee STILL holds for the sender's
  // outstanding-link records across the full lifecycle (#589): the bearer
  // secret must never appear in cleartext in localStorage — not after add, not
  // after a status transition. This is the persistence side of "treat a claim
  // link like cash": if the device's storage is read, no live secret leaks.
  it('never leaves a bearer secret in plaintext across the record lifecycle (#589)', async () => {
    const key = await VaultKey.fromPassword('hygiene-589-pw')
    const store = new ClaimLinkStore(new EncryptedClaimLinks(() => key))
    await store.load()
    const rec = await store.add(SAMPLE)

    const assertNoCleartext = () => {
      const raw = localStorage.getItem(STORAGE_KEY)!
      expect(isVaultBlob(raw)).toBe(true)
      for (const word of SAMPLE.ephMnemonic.split(' ')) {
        expect(raw).not.toContain(word)
      }
      expect(raw).not.toContain(SAMPLE.ephMnemonic)
      expect(raw).not.toContain(SAMPLE.ephAddress)
    }

    assertNoCleartext() // after add
    await store.setStatus(rec.id, 'claimed')
    assertNoCleartext() // after marking claimed
    await store.setStatus(rec.id, 'refunded')
    assertNoCleartext() // after marking refunded
  })
})
