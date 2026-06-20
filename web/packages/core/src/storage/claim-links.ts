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
 * This module only persists/tracks records. Scanning the ephemeral wallet,
 * detecting "claimed" (output spent), and sweeping a refund all reuse the
 * existing wasm-signer send/scan path in the web-wallet — no node change.
 */

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
