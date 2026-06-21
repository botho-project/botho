/**
 * Outstanding claim-link tracking (sender side, #460 phase 3).
 *
 * When a sender creates a bearer claim link, they fund an ephemeral wallet and
 * hand out its secret. To support **refund** (reclaiming an unclaimed link),
 * the sender must be able to re-derive that ephemeral secret later. The MVP
 * stores the ephemeral mnemonic locally (mirrors `AddressBook`'s localStorage
 * pattern). This is device-bound — a refund only works from the same browser
 * that created the link. (A deterministic-from-sender-seed derivation would
 * remove that limitation; deferred per the architect's open decision #1.)
 *
 * SECURITY: each record contains the ephemeral mnemonic, which is bearer-secret
 * for the funds. It lives only in this browser's localStorage and is never sent
 * to a server — same trust model as the wallet's own stored mnemonic.
 *
 * AT-REST ENCRYPTION (#474): the bearer secret must NOT sit in plaintext on the
 * device. {@link EncryptedClaimLinks} wraps localStorage with the session
 * {@link VaultKey} (the same password-derived AES-256-GCM key that protects the
 * seed, landed by #475) so outstanding-link secrets are only readable while the
 * wallet is unlocked. The whole record array is stored as a single versioned
 * vault blob; no ephemeral mnemonic is ever written in cleartext for a
 * password-protected wallet. Legacy plaintext records (written before #474) are
 * transparently re-wrapped under the vault key on the first unlocked load.
 *
 * This module only persists/tracks records. Scanning the ephemeral wallet,
 * detecting "claimed" (output spent), and sweeping a refund all reuse the
 * existing wasm-signer send/scan path in the web-wallet — no node change.
 */

import { VaultKey, isVaultBlob } from '../wallet/vault'

/** Lifecycle status of an outstanding claim link. */
export type ClaimLinkStatus = 'outstanding' | 'claimed' | 'refunded'

/** A locally-tracked outstanding (or resolved) claim link. */
export interface ClaimLinkRecord {
  /** Stable local id. */
  id: string
  /** The ephemeral 12-word mnemonic that owns the funded output (bearer secret). */
  ephMnemonic: string
  /** The ephemeral receiving address the funds were sent to. */
  ephAddress: string
  /** Net amount intended for the recipient, in picocredits (excludes sweep fee). */
  amount: bigint
  /** Unix seconds when the link was created. */
  createdAt: number
  /** The funding transaction hash, if known. */
  fundingTxHash?: string
  /** Current status. */
  status: ClaimLinkStatus
}

/** JSON-serializable form (bigint -> decimal string) for localStorage. */
interface SerializedClaimLinkRecord {
  id: string
  ephMnemonic: string
  ephAddress: string
  amount: string
  createdAt: number
  fundingTxHash?: string
  status: ClaimLinkStatus
}

function serialize(r: ClaimLinkRecord): SerializedClaimLinkRecord {
  return { ...r, amount: r.amount.toString() }
}

function deserialize(r: SerializedClaimLinkRecord): ClaimLinkRecord {
  return { ...r, amount: BigInt(r.amount) }
}

/** Storage interface for claim-link persistence. */
export interface ClaimLinkStorage {
  load(): Promise<ClaimLinkRecord[]>
  save(records: ClaimLinkRecord[]): Promise<void>
}

/** localStorage-based claim-link storage. */
export class LocalStorageClaimLinks implements ClaimLinkStorage {
  private readonly key: string

  constructor(key = 'botho-claim-links') {
    this.key = key
  }

  async load(): Promise<ClaimLinkRecord[]> {
    const data = localStorage.getItem(this.key)
    if (!data) return []
    try {
      const parsed = JSON.parse(data) as SerializedClaimLinkRecord[]
      return parsed.map(deserialize)
    } catch {
      return []
    }
  }

  async save(records: ClaimLinkRecord[]): Promise<void> {
    localStorage.setItem(this.key, JSON.stringify(records.map(serialize)))
  }
}

/**
 * Thrown by {@link EncryptedClaimLinks} when an operation needs the session
 * vault key but the wallet is locked (or is a legacy plaintext wallet with no
 * key). Callers should surface "unlock to continue" rather than persisting a
 * bearer secret in cleartext.
 */
export class ClaimLinksLockedError extends Error {
  constructor(message = 'Claim-link store is locked: unlock the wallet to access bearer secrets') {
    super(message)
    this.name = 'ClaimLinksLockedError'
  }
}

/**
 * Thrown by {@link EncryptedClaimLinks.loadStrict} when a PRESENT encrypted
 * claim-link blob fails to decrypt under the current key (wrong key / tampered).
 *
 * The lenient {@link EncryptedClaimLinks.load} returns `[]` on decrypt failure
 * (so a locked app degrades gracefully); the strict path used by the
 * password-rotation re-wrap (#489) must instead surface the failure so it never
 * mistakes a decrypt FAILURE for an empty store and clobbers bearer secrets.
 */
export class ClaimLinksDecryptError extends Error {
  constructor(
    message = 'Claim-link store failed to decrypt under the current key',
    options?: { cause?: unknown },
  ) {
    super(message)
    this.name = 'ClaimLinksDecryptError'
    if (options && 'cause' in options) {
      ;(this as { cause?: unknown }).cause = options.cause
    }
  }
}

/**
 * localStorage-based claim-link storage encrypted under the session
 * {@link VaultKey} (#474).
 *
 * The entire record array is encrypted as a single versioned vault blob, so the
 * ephemeral bearer mnemonics are NEVER written to localStorage in plaintext for
 * a password-protected wallet.
 *
 * The vault key is read lazily via a getter (the wallet context holds the
 * session key in a ref) so this storage can be constructed once at module scope
 * and pick up the key as soon as the wallet unlocks.
 *
 * Behavior when locked (`getKey()` returns null):
 *   - {@link load} returns `[]` (records are unavailable until unlock) and does
 *     NOT touch any legacy plaintext blob, so nothing is lost.
 *   - {@link save} throws {@link ClaimLinksLockedError} rather than persisting a
 *     bearer secret in cleartext.
 *
 * MIGRATION: if {@link load} finds a legacy PLAINTEXT JSON blob (written before
 * #474) AND a key is available, it parses, then re-encrypts (re-wraps) it under
 * the vault key, overwriting the plaintext so the secret no longer sits in
 * cleartext. Refund ability is preserved across the migration.
 */
export class EncryptedClaimLinks implements ClaimLinkStorage {
  private readonly key: string
  private readonly getKey: () => VaultKey | null

  constructor(getKey: () => VaultKey | null, key = 'botho-claim-links') {
    this.getKey = getKey
    this.key = key
  }

  async load(): Promise<ClaimLinkRecord[]> {
    return this.loadInternal(false)
  }

  /**
   * Strict load for the password-rotation re-wrap path (#489).
   *
   * Unlike {@link load}, if a PRESENT encrypted (vault) blob fails to decrypt
   * under the current session key, this THROWS instead of returning `[]`. This
   * lets the rotation distinguish a genuinely empty store from a decrypt FAILURE
   * — so it never re-wraps an empty store over real bearer secrets that merely
   * failed to decrypt. (A truly empty store / absent blob still returns `[]`.)
   */
  async loadStrict(): Promise<ClaimLinkRecord[]> {
    return this.loadInternal(true)
  }

  private async loadInternal(strict: boolean): Promise<ClaimLinkRecord[]> {
    const data = localStorage.getItem(this.key)
    if (!data) return []

    const vaultKey = this.getKey()

    // Encrypted (vault) blob: only readable while unlocked. If locked, degrade
    // to "unavailable" rather than crashing.
    if (isVaultBlob(data)) {
      if (!vaultKey) {
        if (strict) {
          throw new ClaimLinksLockedError()
        }
        return []
      }
      try {
        const parsed = await vaultKey.decryptJSON<SerializedClaimLinkRecord[]>(data)
        return parsed.map(deserialize)
      } catch (err) {
        // Wrong key / tampered blob. In strict mode (rotation) surface this so
        // the caller never re-wraps an empty store over real bearer secrets;
        // otherwise degrade to empty rather than throw.
        if (strict) {
          throw new ClaimLinksDecryptError(undefined, { cause: err })
        }
        return []
      }
    }

    // Legacy PLAINTEXT JSON blob (pre-#474). When locked, degrade to empty and
    // leave the blob untouched so a later unlock can migrate it — consistent
    // with the locked behavior for vault blobs.
    if (!vaultKey) return []

    let parsed: SerializedClaimLinkRecord[]
    try {
      parsed = JSON.parse(data) as SerializedClaimLinkRecord[]
    } catch {
      return []
    }
    const records = parsed.map(deserialize)

    // Re-wrap under the vault key so the bearer secret stops living in
    // cleartext, preserving refund ability across the migration.
    if (records.length > 0) {
      try {
        await this.save(records)
      } catch {
        // Best-effort migration: keep the (working) plaintext blob and retry on
        // the next unlocked load.
      }
    }
    return records
  }

  async save(records: ClaimLinkRecord[]): Promise<void> {
    const vaultKey = this.getKey()
    if (!vaultKey) {
      throw new ClaimLinksLockedError()
    }
    const blob = await vaultKey.encryptJSON(records.map(serialize))
    localStorage.setItem(this.key, blob)
  }
}

/** Default UX expiry-nudge window (7 days, seconds). Reclaim is always allowed. */
export const CLAIM_LINK_EXPIRY_WINDOW_SECONDS = 7 * 24 * 60 * 60

/** Manager for the sender's outstanding claim links. */
export class ClaimLinkStore {
  private records: Map<string, ClaimLinkRecord> = new Map()
  private storage: ClaimLinkStorage

  constructor(storage: ClaimLinkStorage = new LocalStorageClaimLinks()) {
    this.storage = storage
  }

  async load(): Promise<void> {
    const records = await this.storage.load()
    this.records = new Map(records.map((r) => [r.id, r]))
  }

  /**
   * Like {@link load}, but if the underlying storage supports a strict load
   * (e.g. {@link EncryptedClaimLinks.loadStrict}) it THROWS when a present blob
   * fails to decrypt instead of silently loading empty. Used by the
   * password-rotation re-wrap (#489) so a decrypt failure aborts the rotation
   * rather than clobbering the on-disk bearer secrets with an empty store.
   * Falls back to {@link load} for storages without strict support.
   */
  async loadStrict(): Promise<void> {
    const strictCapable = this.storage as ClaimLinkStorage & {
      loadStrict?: () => Promise<ClaimLinkRecord[]>
    }
    const records =
      typeof strictCapable.loadStrict === 'function'
        ? await strictCapable.loadStrict()
        : await this.storage.load()
    this.records = new Map(records.map((r) => [r.id, r]))
  }

  /**
   * Re-persist the CURRENT in-memory records through the storage layer without
   * re-reading from disk.
   *
   * This is the re-wrap primitive used when the wallet's password changes
   * (#489): the encrypted storage reads the session {@link VaultKey} lazily, so
   * once the context has swapped the session key to the NEW key, calling this
   * re-encrypts the existing bearer secrets under the new key (overwriting the
   * blob that was encrypted under the old key). The in-memory records must
   * already be loaded (the wallet is unlocked) before this is called.
   */
  async rewrap(): Promise<void> {
    await this.persist()
  }

  private async persist(): Promise<void> {
    await this.storage.save(Array.from(this.records.values()))
  }

  /** All records, newest first. */
  getAll(): ClaimLinkRecord[] {
    return Array.from(this.records.values()).sort((a, b) => b.createdAt - a.createdAt)
  }

  /** Add a freshly-created outstanding link. */
  async add(
    input: Omit<ClaimLinkRecord, 'id' | 'createdAt' | 'status'> &
      Partial<Pick<ClaimLinkRecord, 'id' | 'createdAt' | 'status'>>,
  ): Promise<ClaimLinkRecord> {
    const record: ClaimLinkRecord = {
      id: input.id ?? crypto.randomUUID(),
      ephMnemonic: input.ephMnemonic,
      ephAddress: input.ephAddress,
      amount: input.amount,
      createdAt: input.createdAt ?? Math.floor(Date.now() / 1000),
      fundingTxHash: input.fundingTxHash,
      status: input.status ?? 'outstanding',
    }
    this.records.set(record.id, record)
    await this.persist()
    return record
  }

  /** Update a record's status (e.g. mark claimed / refunded). */
  async setStatus(id: string, status: ClaimLinkStatus): Promise<void> {
    const r = this.records.get(id)
    if (!r) return
    r.status = status
    await this.persist()
  }

  /** Remove a record. */
  async delete(id: string): Promise<void> {
    if (this.records.delete(id)) {
      await this.persist()
    }
  }

  /** True if the record is past the UX expiry-nudge window. */
  isExpired(record: ClaimLinkRecord, nowSeconds = Math.floor(Date.now() / 1000)): boolean {
    return nowSeconds - record.createdAt >= CLAIM_LINK_EXPIRY_WINDOW_SECONDS
  }
}
