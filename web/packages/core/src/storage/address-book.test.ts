import { describe, it, expect, beforeEach } from 'vitest'
import { AddressBook, EncryptedAddressBook } from './address-book'
import type { Address, Contact, Timestamp } from '../types'
import { VaultKey, isVaultBlob } from '../wallet/vault'

const localStorageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => {
      store[key] = value
    },
    removeItem: (key: string) => {
      delete store[key]
    },
    clear: () => {
      store = {}
    },
  }
})()

Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock })

const ADDR = 'tbotho://1/recipient' as Address

/**
 * The record-payment UPSERT used by the wallet context's `send()` /
 * `recordPayment`: if the recipient isn't a contact, create a minimal blank-name
 * entry (labelable later), then bump txCount/lastTxAt exactly once. If it
 * already exists, just record the transaction. Kept here so the no-double-count
 * and unnamed-then-named behaviors are unit-tested against the real backend.
 */
async function recordPayment(book: AddressBook, address: Address): Promise<void> {
  const now = Math.floor(Date.now() / 1000) as Timestamp
  if (!book.findByAddress(address)) {
    await book.add('', address)
  }
  await book.recordTransaction(address, now)
}

describe('AddressBook', () => {
  beforeEach(() => {
    localStorageMock.clear()
  })

  describe('basic CRUD', () => {
    it('adds, finds, updates and deletes a contact', async () => {
      const book = new AddressBook()
      await book.load()

      const c = await book.add('Alice', ADDR, 'a friend')
      expect(c.name).toBe('Alice')
      expect(c.txCount).toBe(0)
      expect(book.findByAddress(ADDR)?.id).toBe(c.id)

      const updated = await book.update(c.id, { name: 'Alice B', notes: 'best friend' })
      expect(updated.name).toBe('Alice B')
      expect(updated.notes).toBe('best friend')

      await book.delete(c.id)
      expect(book.findByAddress(ADDR)).toBeUndefined()
    })

    it('rejects a duplicate address', async () => {
      const book = new AddressBook()
      await book.load()
      await book.add('Alice', ADDR)
      await expect(book.add('Alice 2', ADDR)).rejects.toThrow(/already exists/)
    })

    it('search matches by name and address, case-insensitively', async () => {
      const book = new AddressBook()
      await book.load()
      await book.add('Alice', 'tbotho://1/alice' as Address)
      await book.add('Bob', 'tbotho://1/bob' as Address)

      expect(book.search('ALICE').map((c) => c.name)).toEqual(['Alice'])
      expect(book.search('tbotho://1/bob').map((c) => c.name)).toEqual(['Bob'])
      expect(book.search('tbotho').length).toBe(2)
    })
  })

  describe('record-payment upsert (send → address book)', () => {
    it('creates a blank-name "previously paid" entry for a new recipient', async () => {
      const book = new AddressBook()
      await book.load()

      await recordPayment(book, ADDR)

      const c = book.findByAddress(ADDR)
      expect(c).toBeDefined()
      expect(c!.name).toBe('') // unnamed — labelable later
      expect(c!.txCount).toBe(1)
      expect(c!.lastTxAt).toBeGreaterThan(0)
    })

    it('does NOT double-count: one payment bumps txCount by exactly one', async () => {
      const book = new AddressBook()
      await book.load()

      await recordPayment(book, ADDR)
      expect(book.findByAddress(ADDR)!.txCount).toBe(1)

      await recordPayment(book, ADDR)
      expect(book.findByAddress(ADDR)!.txCount).toBe(2)

      // Still only a single contact entry for the address.
      expect(book.search('tbotho').filter((c) => c.address === ADDR).length).toBe(1)
    })

    it('bumps an EXISTING named contact instead of creating a duplicate', async () => {
      const book = new AddressBook()
      await book.load()
      const existing = await book.add('Alice', ADDR)

      await recordPayment(book, ADDR)

      const c = book.findByAddress(ADDR)!
      expect(c.id).toBe(existing.id) // same entry
      expect(c.name).toBe('Alice') // name preserved
      expect(c.txCount).toBe(1)
    })

    it('lets an unnamed previously-paid entry be renamed (label later)', async () => {
      const book = new AddressBook()
      await book.load()

      await recordPayment(book, ADDR)
      const unnamed = book.findByAddress(ADDR)!
      expect(unnamed.name).toBe('')
      expect(unnamed.txCount).toBe(1)

      const renamed = await book.update(unnamed.id, { name: 'Alice', notes: 'from a payment' })
      expect(renamed.name).toBe('Alice')
      expect(renamed.notes).toBe('from a payment')
      // Renaming preserves the recorded payment history (no double-count / reset).
      expect(renamed.txCount).toBe(1)
      expect(book.findByAddress(ADDR)!.name).toBe('Alice')
    })

    it('persists across reloads (localStorage)', async () => {
      const book1 = new AddressBook()
      await book1.load()
      await recordPayment(book1, ADDR)
      await book1.update(book1.findByAddress(ADDR)!.id, { name: 'Alice' })

      const book2 = new AddressBook()
      await book2.load()
      const c = book2.findByAddress(ADDR)
      expect(c?.name).toBe('Alice')
      expect(c?.txCount).toBe(1)
    })
  })

  describe('getDisplayName', () => {
    it('returns the contact name when known', async () => {
      const book = new AddressBook()
      await book.load()
      await book.add('Alice', ADDR)
      expect(book.getDisplayName(ADDR)).toBe('Alice')
    })

    it('truncates an unknown address', async () => {
      const book = new AddressBook()
      await book.load()
      const long = ('tbotho://1/' + 'a'.repeat(40)) as Address
      const shown = book.getDisplayName(long)
      expect(shown).toContain('...')
      expect(shown.length).toBeLessThan(long.length)
    })
  })
})

// ---------------------------------------------------------------------------
// EncryptedAddressBook (#476): the contact graph is encrypted at rest under the
// session VaultKey, with graceful degradation when there is no key.
// ---------------------------------------------------------------------------

const STORAGE_KEY = 'botho-address-book'

// A privacy-sensitive contact whose name/address/notes must NOT appear in the
// stored blob for a password-protected wallet.
const SECRET_NAME = 'Alice Cooper'
const SECRET_ADDR = 'tbotho://1/counterparty-secret' as Address
const SECRET_NOTES = 'paid rent for the flat in Gaborone'

function legacyContact(): Contact {
  const now = 123 as Timestamp
  return {
    id: 'legacy-1',
    name: SECRET_NAME,
    address: SECRET_ADDR,
    notes: SECRET_NOTES,
    createdAt: now,
    updatedAt: now,
    txCount: 3,
    lastTxAt: now,
  }
}

describe('EncryptedAddressBook', () => {
  beforeEach(() => {
    localStorageMock.clear()
  })

  it('round-trips contacts WITHOUT writing the contact graph in plaintext', async () => {
    const key = await VaultKey.fromPassword('correct horse battery staple')
    const book = new AddressBook(new EncryptedAddressBook(() => key))
    await book.load()
    await book.add(SECRET_NAME, SECRET_ADDR, SECRET_NOTES)

    // The persisted blob must be a versioned vault blob and must NOT contain any
    // contact name/address/notes in cleartext.
    const raw = localStorage.getItem(STORAGE_KEY)!
    expect(raw).toBeTruthy()
    expect(isVaultBlob(raw)).toBe(true)
    expect(raw).not.toContain(SECRET_NAME)
    expect(raw).not.toContain(SECRET_ADDR)
    expect(raw).not.toContain(SECRET_NOTES)
    expect(raw).not.toContain('Gaborone')

    // A fresh book reading the same key decrypts it back faithfully.
    const book2 = new AddressBook(new EncryptedAddressBook(() => key))
    await book2.load()
    const found = book2.findByAddress(SECRET_ADDR)!
    expect(found).toBeTruthy()
    expect(found.name).toBe(SECRET_NAME)
    expect(found.notes).toBe(SECRET_NOTES)
  })

  it('migrates a legacy plaintext address book to an encrypted blob on unlocked load', async () => {
    // Simulate a pre-#476 plaintext contact array in localStorage.
    localStorage.setItem(STORAGE_KEY, JSON.stringify([legacyContact()]))
    expect(isVaultBlob(localStorage.getItem(STORAGE_KEY)!)).toBe(false)

    const key = await VaultKey.fromPassword('pw-migrate-12345')
    const storage = new EncryptedAddressBook(() => key)
    const contacts = await storage.load()

    // The contacts are returned...
    expect(contacts).toHaveLength(1)
    expect(contacts[0].name).toBe(SECRET_NAME)
    expect(contacts[0].txCount).toBe(3)

    // ...and the plaintext blob has been re-wrapped: no more cleartext graph.
    const raw = localStorage.getItem(STORAGE_KEY)!
    expect(isVaultBlob(raw)).toBe(true)
    expect(raw).not.toContain(SECRET_NAME)
    expect(raw).not.toContain(SECRET_ADDR)
    expect(raw).not.toContain(SECRET_NOTES)
  })

  it('reads as empty when locked (no key) without touching a legacy plaintext blob', async () => {
    const plaintext = JSON.stringify([legacyContact()])
    localStorage.setItem(STORAGE_KEY, plaintext)

    const lockedStorage = new EncryptedAddressBook(() => null)
    expect(await lockedStorage.load()).toEqual([])
    // The plaintext blob is untouched by a locked read (so a later unlock can
    // migrate it), and definitely not re-written.
    expect(localStorage.getItem(STORAGE_KEY)).toBe(plaintext)

    // An encrypted blob is likewise unavailable (not a crash) while locked.
    const key = await VaultKey.fromPassword('pw-lock-test-1')
    const enc = new EncryptedAddressBook(() => key)
    await enc.save([legacyContact()])
    const encryptedBlob = localStorage.getItem(STORAGE_KEY)!
    expect(await lockedStorage.load()).toEqual([])
    expect(localStorage.getItem(STORAGE_KEY)).toBe(encryptedBlob)
  })

  it('save() with no key is a NO-OP: never throws, never writes plaintext', async () => {
    const lockedStorage = new EncryptedAddressBook(() => null)
    // Must NOT throw (unlike claim links, the address book degrades gracefully).
    await expect(lockedStorage.save([legacyContact()])).resolves.toBeUndefined()
    // And nothing — encrypted or plaintext — was persisted.
    expect(localStorage.getItem(STORAGE_KEY)).toBeNull()
  })

  it('recordPayment-style upsert with no key does not throw (spend path safe)', async () => {
    // Mirrors the wallet context recordPayment / post-send address-book write
    // for a plaintext / locked wallet: add + recordTransaction must not throw.
    const book = new AddressBook(new EncryptedAddressBook(() => null))
    await book.load()
    const now = Math.floor(Date.now() / 1000) as Timestamp
    await expect(
      (async () => {
        if (!book.findByAddress(SECRET_ADDR)) {
          await book.add('', SECRET_ADDR)
        }
        await book.recordTransaction(SECRET_ADDR, now)
      })(),
    ).resolves.toBeUndefined()
    // No plaintext contact graph was persisted.
    expect(localStorage.getItem(STORAGE_KEY)).toBeNull()
  })

  it('persists and surfaces contacts again once a key becomes available', async () => {
    const key = await VaultKey.fromPassword('unlock-then-save-1')
    const book = new AddressBook(new EncryptedAddressBook(() => key))
    await book.load()
    await book.add(SECRET_NAME, SECRET_ADDR, SECRET_NOTES)
    await book.recordTransaction(SECRET_ADDR, 999 as Timestamp)

    const book2 = new AddressBook(new EncryptedAddressBook(() => key))
    await book2.load()
    const found = book2.findByAddress(SECRET_ADDR)!
    expect(found.txCount).toBe(1)
    expect(found.lastTxAt).toBe(999)
    expect(isVaultBlob(localStorage.getItem(STORAGE_KEY)!)).toBe(true)
  })
})
