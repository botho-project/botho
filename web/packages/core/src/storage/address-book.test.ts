import { describe, it, expect, beforeEach } from 'vitest'
import { AddressBook } from './address-book'
import type { Address, Timestamp } from '../types'

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
