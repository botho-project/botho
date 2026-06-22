import { describe, it, expect } from 'vitest'
import { D1RigStore, DuplicateSubscriptionError, type D1Like } from './rig-store'

/**
 * Tiny in-memory D1 fake honoring just enough SQL for the store: the specific
 * INSERT / SELECT / UPDATE / COUNT statements D1RigStore issues. Pattern-matches
 * on the query text rather than parsing SQL.
 */
interface Row {
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

function fakeD1(): { db: D1Like; rows: Map<string, Row> } {
  const rows = new Map<string, Row>()
  const db: D1Like = {
    prepare(query: string) {
      let bound: unknown[] = []
      const stmt = {
        bind(...values: unknown[]) {
          bound = values
          return stmt
        },
        async first<T>(): Promise<T | null> {
          if (query.startsWith('SELECT * FROM rigs WHERE subscription_id')) {
            return (rows.get(bound[0] as string) ?? null) as T | null
          }
          if (query.startsWith('SELECT COUNT(*)')) {
            const states = bound as string[]
            let n = 0
            for (const r of rows.values()) if (states.includes(r.state)) n++
            return { n } as unknown as T
          }
          return null
        },
        async run() {
          if (query.startsWith('INSERT INTO rigs')) {
            const [user, stripe_customer, subscription_id, rig_id, region, rpc_url, created_at, updated_at] =
              bound as [string, string, string, string, string, string, number, number]
            if (rows.has(subscription_id)) {
              throw new Error('UNIQUE constraint failed: rigs.subscription_id')
            }
            rows.set(subscription_id, {
              user,
              stripe_customer,
              subscription_id,
              rig_id,
              instance_id: null,
              region,
              rpc_url,
              state: 'provisioning',
              created_at,
              updated_at,
            })
            return { success: true }
          }
          if (query.startsWith('UPDATE rigs SET instance_id')) {
            const [instance_id, updated_at, subscription_id] = bound as [string, number, string]
            const r = rows.get(subscription_id)
            if (r) {
              r.instance_id = instance_id
              r.updated_at = updated_at
            }
            return { success: true }
          }
          if (query.startsWith('UPDATE rigs SET state')) {
            const [state, updated_at, subscription_id] = bound as [string, number, string]
            const r = rows.get(subscription_id)
            if (r) {
              r.state = state
              r.updated_at = updated_at
            }
            return { success: true }
          }
          return { success: true }
        },
        async all<T>() {
          return { results: [] as T[] }
        },
      }
      return stmt
    },
  }
  return { db, rows }
}

const NEW = {
  user: 'cus_X',
  stripeCustomer: 'cus_X',
  subscriptionId: 'sub_1',
  rigId: 'r1',
  region: 'us-west-2',
  rpcUrl: 'https://rig-r1.testnet.botho.io/rpc',
}

describe('D1RigStore', () => {
  it('inserts and reads back a provisioning row', async () => {
    const { db } = fakeD1()
    const store = new D1RigStore(db, () => 1000)
    const rec = await store.insertProvisioning(NEW)
    expect(rec.state).toBe('provisioning')
    expect(rec.instanceId).toBeNull()

    const got = await store.getBySubscription('sub_1')
    expect(got?.rigId).toBe('r1')
    expect(got?.rpcUrl).toBe('https://rig-r1.testnet.botho.io/rpc')
  })

  it('rejects a duplicate subscription with DuplicateSubscriptionError', async () => {
    const { db } = fakeD1()
    const store = new D1RigStore(db, () => 1000)
    await store.insertProvisioning(NEW)
    await expect(store.insertProvisioning(NEW)).rejects.toBeInstanceOf(DuplicateSubscriptionError)
  })

  it('attaches the instance id and transitions state', async () => {
    const { db } = fakeD1()
    const store = new D1RigStore(db, () => 1000)
    await store.insertProvisioning(NEW)
    await store.setInstanceId('sub_1', 'i-123')
    await store.setState('sub_1', 'running')
    const got = await store.getBySubscription('sub_1')
    expect(got?.instanceId).toBe('i-123')
    expect(got?.state).toBe('running')
  })

  it('counts only active (non-terminated) rows', async () => {
    const { db } = fakeD1()
    const store = new D1RigStore(db, () => 1000)
    await store.insertProvisioning(NEW)
    await store.insertProvisioning({ ...NEW, subscriptionId: 'sub_2', rigId: 'r2' })
    expect(await store.countActive()).toBe(2)
    await store.setState('sub_2', 'terminated')
    expect(await store.countActive()).toBe(1)
  })
})
