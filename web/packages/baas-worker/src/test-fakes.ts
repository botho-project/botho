/**
 * In-memory fakes for the provisioner's injected dependencies. Tests use these
 * so NO real AWS / Cloudflare / D1 call is ever made (#502 test requirement).
 */

import type { DnsClient, DnsRecord } from './cloudflare-dns'
import type { Ec2Client, Ec2Instance, RunInstanceParams } from './ec2'
import {
  ACTIVE_STATES,
  DuplicateSubscriptionError,
  type NewRigRecord,
  type RigRecord,
  type RigState,
  type RigStore,
} from './rig-store'
import {
  StripeSubscriptionError,
  type SubscriptionChecker,
} from './stripe-subscriptions'

/** Records each call so tests can assert on the request that was built. */
export class FakeEc2 implements Ec2Client {
  runCalls: RunInstanceParams[] = []
  terminateCalls: { region: string; instanceId: string }[] = []
  /** Instances keyed by subscription tag, seeded by tests for reconcile paths. */
  bySubscription = new Map<string, Ec2Instance[]>()
  /**
   * All managed rigs per region, seeded by tests for the reconciliation sweep
   * (#508). `describeManagedRigs(region)` returns this list. When a test does
   * not seed it, falls back to every instance tracked via runInstance so the
   * existing provisioner tests need no change.
   */
  managedByRegion = new Map<string, Ec2Instance[]>()
  /** If set, describeManagedRigs throws for this region (transient-error tests). */
  describeManagedRigsError: string | undefined
  private seq = 0
  /** Public IP returned by runInstance (undefined => not yet assigned). */
  runPublicIp: string | undefined

  async runInstance(params: RunInstanceParams): Promise<Ec2Instance> {
    this.runCalls.push(params)
    const instanceId = `i-fake${++this.seq}`
    const inst: Ec2Instance = {
      instanceId,
      state: 'pending',
      publicIp: this.runPublicIp,
      subscriptionTag: params.tags['botho:subscription'],
      rigIdTag: params.tags['botho:rig-id'],
    }
    const sub = params.tags['botho:subscription']
    const list = this.bySubscription.get(sub) ?? []
    list.push(inst)
    this.bySubscription.set(sub, list)
    return inst
  }

  async describeBySubscription(
    _region: string,
    subscriptionId: string,
  ): Promise<Ec2Instance[]> {
    return this.bySubscription.get(subscriptionId) ?? []
  }

  async describeManagedRigs(region: string): Promise<Ec2Instance[]> {
    if (this.describeManagedRigsError) {
      throw new Error(this.describeManagedRigsError)
    }
    if (this.managedByRegion.has(region)) {
      return this.managedByRegion.get(region) ?? []
    }
    // Fallback: every instance ever launched (used when a test only seeds via
    // runInstance and doesn't care about the per-region managed list).
    const all: Ec2Instance[] = []
    for (const list of this.bySubscription.values()) all.push(...list)
    return all
  }

  async terminateInstance(region: string, instanceId: string): Promise<void> {
    this.terminateCalls.push({ region, instanceId })
  }
}

/**
 * In-memory Stripe subscription checker for reconciliation tests. Seed
 * `active` with the subscription ids that should be treated as active; any id
 * not present is inactive (orphan). Set `throwFor` to simulate a transient
 * Stripe error for specific subscription ids.
 */
export class FakeSubscriptionChecker implements SubscriptionChecker {
  active = new Set<string>()
  throwFor = new Set<string>()
  calls: string[] = []

  async isActive(subscriptionId: string): Promise<boolean> {
    this.calls.push(subscriptionId)
    if (this.throwFor.has(subscriptionId)) {
      // Mirror HttpSubscriptionChecker: a transient error throws so the sweep
      // skips the rig rather than reaping it.
      throw new StripeSubscriptionError('simulated transient error', 503)
    }
    return this.active.has(subscriptionId)
  }
}

/** Records DNS upserts/deletes. */
export class FakeDns implements DnsClient {
  records = new Map<string, DnsRecord>()
  upsertCalls: { name: string; ip: string }[] = []
  deleteCalls: string[] = []
  private seq = 0

  async upsertARecord(name: string, ip: string): Promise<DnsRecord> {
    this.upsertCalls.push({ name, ip })
    const rec: DnsRecord = { id: `dns-${++this.seq}`, name, content: ip }
    this.records.set(name, rec)
    return rec
  }

  async deleteARecord(name: string): Promise<void> {
    this.deleteCalls.push(name)
    this.records.delete(name)
  }
}

/** In-memory RigStore with a UNIQUE constraint on subscription_id. */
export class FakeStore implements RigStore {
  rows = new Map<string, RigRecord>()
  private now: number

  constructor(now = 1_700_000_000_000) {
    this.now = now
  }

  async getBySubscription(subscriptionId: string): Promise<RigRecord | undefined> {
    return this.rows.get(subscriptionId)
  }

  async getByCustomer(stripeCustomer: string): Promise<RigRecord | undefined> {
    // Mirror D1RigStore: live rigs before terminated, then newest createdAt.
    const matches = [...this.rows.values()].filter(
      (r) => r.stripeCustomer === stripeCustomer,
    )
    matches.sort((a, b) => {
      const aTerm = a.state === 'terminated' ? 1 : 0
      const bTerm = b.state === 'terminated' ? 1 : 0
      if (aTerm !== bTerm) return aTerm - bTerm
      return b.createdAt - a.createdAt
    })
    return matches[0]
  }

  async insertProvisioning(rec: NewRigRecord): Promise<RigRecord> {
    if (this.rows.has(rec.subscriptionId)) {
      throw new DuplicateSubscriptionError(rec.subscriptionId)
    }
    const record: RigRecord = {
      ...rec,
      instanceId: null,
      state: 'provisioning',
      createdAt: this.now,
      updatedAt: this.now,
    }
    this.rows.set(rec.subscriptionId, record)
    return record
  }

  async setInstanceId(subscriptionId: string, instanceId: string): Promise<void> {
    const row = this.rows.get(subscriptionId)
    if (row) {
      row.instanceId = instanceId
      row.updatedAt = ++this.now
    }
  }

  async setState(subscriptionId: string, state: RigState): Promise<void> {
    const row = this.rows.get(subscriptionId)
    if (row) {
      row.state = state
      row.updatedAt = ++this.now
    }
  }

  async countActive(): Promise<number> {
    let n = 0
    for (const row of this.rows.values()) {
      if (ACTIVE_STATES.includes(row.state)) n++
    }
    return n
  }
}

/** A trivial base64 encoder for tests (deterministic, no Worker btoa needed). */
export function testBase64(s: string): string {
  // Node + jsdom + workerd test env all expose btoa; fall back to a manual
  // implementation only if it is missing.
  if (typeof btoa === 'function') {
    const bytes = new TextEncoder().encode(s)
    let bin = ''
    for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i])
    return btoa(bin)
  }
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (globalThis as any).Buffer.from(s, 'utf-8').toString('base64')
}

/** Decode a base64 string back to UTF-8 (test helper; mirrors testBase64). */
export function testBase64Decode(b64: string): string {
  if (typeof atob === 'function') {
    const bin = atob(b64)
    const bytes = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
    return new TextDecoder().decode(bytes)
  }
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (globalThis as any).Buffer.from(b64, 'base64').toString('utf-8')
}
