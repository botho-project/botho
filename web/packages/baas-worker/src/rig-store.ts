/**
 * D1-backed user<->rig mapping for the Botho-as-a-Service provisioner
 * (#458 §3 step 4). Schema in `schema.sql`.
 *
 * The provisioner depends on the `RigStore` *interface* so tests use an
 * in-memory fake — no D1 binding is touched under test (#502 requirement). The
 * `D1RigStore` wraps Cloudflare's `D1Database` binding for production.
 *
 * Idempotency anchor: `subscription_id` is UNIQUE. `getBySubscription` is the
 * first check the provisioner performs so a replayed trigger never launches a
 * second instance (#458 §3, §5).
 */

/** Lifecycle state of a rig row (#458 §3 step 4). */
export type RigState = 'provisioning' | 'running' | 'suspended' | 'terminated'

/** A row in the rig mapping table. */
export interface RigRecord {
  /** Stripe customer id == our user identity (#458 §4). */
  user: string
  /** Stripe customer id (denormalized for lookups). */
  stripeCustomer: string
  /** Stripe subscription id — the idempotency key (UNIQUE). */
  subscriptionId: string
  /** Short opaque rig id used for the hostname (`rig-<id>`). */
  rigId: string
  /** EC2 instance id once launched (null while still pre-launch). */
  instanceId: string | null
  /** AWS region. */
  region: string
  /** HTTPS `/rpc` URL the user points the PWA at. */
  rpcUrl: string
  state: RigState
  createdAt: number
  updatedAt: number
}

/** Fields supplied when first inserting a provisioning row (pre-launch). */
export interface NewRigRecord {
  user: string
  stripeCustomer: string
  subscriptionId: string
  rigId: string
  region: string
  rpcUrl: string
}

/** Injectable persistence surface for the rig mapping. */
export interface RigStore {
  /** Idempotency lookup by Stripe subscription id. */
  getBySubscription(subscriptionId: string): Promise<RigRecord | undefined>
  /**
   * Insert a fresh `provisioning` row. MUST reject (throw) if a row with the
   * same `subscriptionId` already exists, so a race can't create two rows.
   */
  insertProvisioning(rec: NewRigRecord): Promise<RigRecord>
  /** Attach the launched instance id (provisioning -> still provisioning). */
  setInstanceId(subscriptionId: string, instanceId: string): Promise<void>
  /** Transition a row's lifecycle state. */
  setState(subscriptionId: string, state: RigState): Promise<void>
  /** Count rows currently consuming fleet capacity (non-terminated). */
  countActive(): Promise<number>
}

/** Thrown when an insert collides with an existing subscription row. */
export class DuplicateSubscriptionError extends Error {
  constructor(public readonly subscriptionId: string) {
    super(`a rig already exists for subscription ${subscriptionId}`)
    this.name = 'DuplicateSubscriptionError'
  }
}

/** States that still consume fleet capacity (count toward the global cap). */
export const ACTIVE_STATES: RigState[] = ['provisioning', 'running', 'suspended']

/* eslint-disable @typescript-eslint/no-explicit-any */
// Minimal structural type for the D1 binding so we don't need the runtime
// import. Mirrors @cloudflare/workers-types' D1Database.
interface D1PreparedStatement {
  bind(...values: unknown[]): D1PreparedStatement
  first<T = unknown>(colName?: string): Promise<T | null>
  run(): Promise<{ success: boolean; meta?: { changes?: number } }>
  all<T = unknown>(): Promise<{ results: T[] }>
}
export interface D1Like {
  prepare(query: string): D1PreparedStatement
}
/* eslint-enable @typescript-eslint/no-explicit-any */

interface RigRow {
  user: string
  stripe_customer: string
  subscription_id: string
  rig_id: string
  instance_id: string | null
  region: string
  rpc_url: string
  state: string
  created_at: number
  updated_at: number
}

function rowToRecord(r: RigRow): RigRecord {
  return {
    user: r.user,
    stripeCustomer: r.stripe_customer,
    subscriptionId: r.subscription_id,
    rigId: r.rig_id,
    instanceId: r.instance_id,
    region: r.region,
    rpcUrl: r.rpc_url,
    state: r.state as RigState,
    createdAt: r.created_at,
    updatedAt: r.updated_at,
  }
}

/** Production `RigStore` over Cloudflare D1. */
export class D1RigStore implements RigStore {
  constructor(
    private readonly db: D1Like,
    private readonly now: () => number = () => Date.now(),
  ) {}

  async getBySubscription(subscriptionId: string): Promise<RigRecord | undefined> {
    const row = await this.db
      .prepare('SELECT * FROM rigs WHERE subscription_id = ?')
      .bind(subscriptionId)
      .first<RigRow>()
    return row ? rowToRecord(row) : undefined
  }

  async insertProvisioning(rec: NewRigRecord): Promise<RigRecord> {
    const ts = this.now()
    try {
      await this.db
        .prepare(
          `INSERT INTO rigs
             (user, stripe_customer, subscription_id, rig_id, instance_id,
              region, rpc_url, state, created_at, updated_at)
           VALUES (?, ?, ?, ?, NULL, ?, ?, 'provisioning', ?, ?)`,
        )
        .bind(
          rec.user,
          rec.stripeCustomer,
          rec.subscriptionId,
          rec.rigId,
          rec.region,
          rec.rpcUrl,
          ts,
          ts,
        )
        .run()
    } catch (err) {
      // D1 surfaces a UNIQUE violation as a thrown error — translate it.
      if (String(err).includes('UNIQUE')) {
        throw new DuplicateSubscriptionError(rec.subscriptionId)
      }
      throw err
    }
    return {
      ...rec,
      instanceId: null,
      state: 'provisioning',
      createdAt: ts,
      updatedAt: ts,
    }
  }

  async setInstanceId(subscriptionId: string, instanceId: string): Promise<void> {
    await this.db
      .prepare('UPDATE rigs SET instance_id = ?, updated_at = ? WHERE subscription_id = ?')
      .bind(instanceId, this.now(), subscriptionId)
      .run()
  }

  async setState(subscriptionId: string, state: RigState): Promise<void> {
    await this.db
      .prepare('UPDATE rigs SET state = ?, updated_at = ? WHERE subscription_id = ?')
      .bind(state, this.now(), subscriptionId)
      .run()
  }

  async countActive(): Promise<number> {
    const placeholders = ACTIVE_STATES.map(() => '?').join(', ')
    const row = await this.db
      .prepare(`SELECT COUNT(*) AS n FROM rigs WHERE state IN (${placeholders})`)
      .bind(...ACTIVE_STATES)
      .first<{ n: number }>()
    return row?.n ?? 0
  }
}
