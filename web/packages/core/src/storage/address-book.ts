import type { Address, Contact, Timestamp } from '../types'
import { VaultKey, isVaultBlob } from '../wallet/vault'

/**
 * Storage interface for address book persistence
 * Implementations can use localStorage, IndexedDB, or other storage
 */
export interface AddressBookStorage {
  load(): Promise<Contact[]>
  save(contacts: Contact[]): Promise<void>
}

/**
 * localStorage-based storage for address book
 */
export class LocalStorageAddressBook implements AddressBookStorage {
  private readonly key: string

  constructor(key = 'botho-address-book') {
    this.key = key
  }

  async load(): Promise<Contact[]> {
    const data = localStorage.getItem(this.key)
    if (!data) return []
    try {
      return JSON.parse(data) as Contact[]
    } catch {
      return []
    }
  }

  async save(contacts: Contact[]): Promise<void> {
    localStorage.setItem(this.key, JSON.stringify(contacts))
  }
}

/**
 * localStorage-based address book encrypted at rest under the session
 * {@link VaultKey} (#476).
 *
 * The address book leaks the user's counterparty graph and personal annotations
 * (names, addresses, notes, tx counts) — privacy-sensitive even though the
 * entries are not bearer secrets. This storage encrypts the WHOLE contact array
 * as a single versioned vault blob (the same password-derived AES-256-GCM key
 * that protects the seed (#475) and claim-link secrets (#474)) so the contact
 * graph is only readable while the wallet is unlocked, and is NEVER written to
 * localStorage in plaintext for a password-protected wallet.
 *
 * The vault key is read lazily via a getter (the wallet context holds the
 * session key in a ref) so this storage can be constructed once at module scope
 * and pick up the key as soon as the wallet unlocks.
 *
 * GRACEFUL DEGRADATION (the address book is less critical than bearer secrets):
 *   - {@link load} with no key (locked, or a legacy plaintext / no-password
 *     wallet) returns `[]` (contacts are unavailable until unlock) and does NOT
 *     touch any legacy plaintext blob, so nothing is lost.
 *   - {@link save} with no key is a NO-OP: it does NOT throw and does NOT write
 *     the contact graph in plaintext under the encrypt-by-default posture.
 *     Contacts are simply not persisted until the wallet has a password / is
 *     unlocked. This keeps the spend path (recordPayment after a send) from
 *     throwing or losing money when there is no vault key.
 *
 * MIGRATION: if {@link load} finds a legacy PLAINTEXT JSON blob (written before
 * #476) AND a key is available, it parses, then re-encrypts (re-wraps) it under
 * the vault key, overwriting the plaintext so the contact graph no longer sits
 * in cleartext.
 */
export class EncryptedAddressBook implements AddressBookStorage {
  private readonly key: string
  private readonly getKey: () => VaultKey | null

  constructor(getKey: () => VaultKey | null, key = 'botho-address-book') {
    this.getKey = getKey
    this.key = key
  }

  async load(): Promise<Contact[]> {
    const data = localStorage.getItem(this.key)
    if (!data) return []

    const vaultKey = this.getKey()

    // Encrypted (vault) blob: only readable while unlocked. If locked, degrade
    // to "unavailable" rather than crashing.
    if (isVaultBlob(data)) {
      if (!vaultKey) return []
      try {
        return await vaultKey.decryptJSON<Contact[]>(data)
      } catch {
        // Wrong key / tampered blob — degrade to empty rather than throw.
        return []
      }
    }

    // Legacy PLAINTEXT JSON blob (pre-#476). When there is no key, degrade to
    // empty and leave the blob untouched so a later unlock can migrate it.
    if (!vaultKey) return []

    let contacts: Contact[]
    try {
      contacts = JSON.parse(data) as Contact[]
    } catch {
      return []
    }

    // Re-wrap under the vault key so the contact graph stops living in cleartext.
    if (contacts.length > 0) {
      try {
        await this.save(contacts)
      } catch {
        // Best-effort migration: keep the (working) plaintext blob and retry on
        // the next unlocked load.
      }
    }
    return contacts
  }

  async save(contacts: Contact[]): Promise<void> {
    const vaultKey = this.getKey()
    if (!vaultKey) {
      // No session key (locked / plaintext wallet): do NOT persist the contact
      // graph in cleartext, and do NOT throw — contacts are simply not saved
      // until the wallet has a password. This keeps the spend path safe.
      return
    }
    const blob = await vaultKey.encryptJSON(contacts)
    localStorage.setItem(this.key, blob)
  }
}

/**
 * Address book manager for storing and managing contacts
 */
export class AddressBook {
  private contacts: Map<string, Contact> = new Map()
  private storage: AddressBookStorage

  constructor(storage: AddressBookStorage = new LocalStorageAddressBook()) {
    this.storage = storage
  }

  async load(): Promise<void> {
    const contacts = await this.storage.load()
    this.contacts = new Map(contacts.map(c => [c.id, c]))
  }

  /**
   * Re-persist the CURRENT in-memory contacts through the storage layer without
   * re-reading from disk.
   *
   * This is the re-wrap primitive used when the wallet's password changes
   * (#489): the encrypted storage reads the session {@link VaultKey} lazily, so
   * once the context has swapped the session key to the NEW key, calling this
   * re-encrypts the existing contact graph under the new key (overwriting the
   * blob that was encrypted under the old key). The in-memory contacts must
   * already be loaded (the wallet is unlocked) before this is called.
   */
  async rewrap(): Promise<void> {
    await this.persist()
  }

  private async persist(): Promise<void> {
    await this.storage.save(Array.from(this.contacts.values()))
  }

  /**
   * Get all contacts, optionally sorted
   */
  getAll(sortBy: 'name' | 'lastTx' | 'txCount' = 'name'): Contact[] {
    const contacts = Array.from(this.contacts.values())

    switch (sortBy) {
      case 'name':
        return contacts.sort((a, b) => a.name.localeCompare(b.name))
      case 'lastTx':
        return contacts.sort((a, b) => (b.lastTxAt ?? 0) - (a.lastTxAt ?? 0))
      case 'txCount':
        return contacts.sort((a, b) => b.txCount - a.txCount)
      default:
        return contacts
    }
  }

  /**
   * Find a contact by address
   */
  findByAddress(address: Address): Contact | undefined {
    return Array.from(this.contacts.values()).find(
      c => c.address.toLowerCase() === address.toLowerCase()
    )
  }

  /**
   * Search contacts by name or address
   */
  search(query: string): Contact[] {
    const q = query.toLowerCase()
    return Array.from(this.contacts.values()).filter(
      c => c.name.toLowerCase().includes(q) || c.address.toLowerCase().includes(q)
    )
  }

  /**
   * Add a new contact
   */
  async add(name: string, address: Address, notes?: string): Promise<Contact> {
    // Check for duplicate address
    const existing = this.findByAddress(address)
    if (existing) {
      throw new Error(`Contact with address ${address} already exists: ${existing.name}`)
    }

    const now = Math.floor(Date.now() / 1000) as Timestamp
    const contact: Contact = {
      id: crypto.randomUUID(),
      name: name.trim(),
      address,
      notes: notes?.trim(),
      createdAt: now,
      updatedAt: now,
      txCount: 0,
    }

    this.contacts.set(contact.id, contact)
    await this.persist()
    return contact
  }

  /**
   * Update an existing contact
   */
  async update(id: string, updates: Partial<Pick<Contact, 'name' | 'address' | 'notes'>>): Promise<Contact> {
    const contact = this.contacts.get(id)
    if (!contact) {
      throw new Error(`Contact not found: ${id}`)
    }

    // If updating address, check for duplicates
    if (updates.address && updates.address !== contact.address) {
      const existing = this.findByAddress(updates.address)
      if (existing && existing.id !== id) {
        throw new Error(`Contact with address ${updates.address} already exists: ${existing.name}`)
      }
    }

    const updated: Contact = {
      ...contact,
      ...updates,
      name: updates.name?.trim() ?? contact.name,
      notes: updates.notes?.trim() ?? contact.notes,
      updatedAt: Math.floor(Date.now() / 1000) as Timestamp,
    }

    this.contacts.set(id, updated)
    await this.persist()
    return updated
  }

  /**
   * Delete a contact
   */
  async delete(id: string): Promise<void> {
    if (!this.contacts.has(id)) {
      throw new Error(`Contact not found: ${id}`)
    }
    this.contacts.delete(id)
    await this.persist()
  }

  /**
   * Record a transaction with a contact (updates txCount and lastTxAt)
   */
  async recordTransaction(address: Address, timestamp: Timestamp): Promise<void> {
    const contact = this.findByAddress(address)
    if (contact) {
      contact.txCount++
      contact.lastTxAt = timestamp
      contact.updatedAt = Math.floor(Date.now() / 1000) as Timestamp
      await this.persist()
    }
  }

  /**
   * Get the display name for an address (contact name or truncated address)
   */
  getDisplayName(address: Address, truncateLength = 8): string {
    const contact = this.findByAddress(address)
    if (contact) {
      return contact.name
    }
    if (address.length <= truncateLength * 2 + 3) {
      return address
    }
    return `${address.slice(0, truncateLength)}...${address.slice(-truncateLength)}`
  }
}
