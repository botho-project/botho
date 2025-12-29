import type { Address, Contact, Timestamp } from '../types'

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
