import { describe, it, expect } from 'vitest'
import { D1NodeStore, DuplicateSubscriptionError, type D1Like } from './node-store'

/**
 * Tiny in-memory D1 fake honoring just enough SQL for the store: the specific
 * INSERT / SELECT / UPDATE / COUNT statements D1NodeStore issues. Pattern-matches
 * on the query text rather than parsing SQL.
 */
interface Row {
  user: string
  stripe_customer: string
  subscription_id: string
  node_id: string
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
          if (query.startsWith('SELECT * FROM nodes WHERE subscription_id')) {
            return (rows.get(bound[0] as string) ?? null) as T | null
          }
          if (query.includes('WHERE stripe_customer = ?')) {
            // Mirror the ORDER BY: live nodes before terminated, newest first.
            const matches = [...rows.values()].filter(
              (r) => r.stripe_customer === (bound[0] as string),
            )
            matches.sort((a, b) => {
              const aTerm = a.state === 'terminated' ? 1 : 0
              const bTerm = b.state === 'terminated' ? 1 : 0
              if (aTerm !== bTerm) return aTerm - bTerm
              return b.created_at - a.created_at
            })
            return (matches[0] ?? null) as T | null
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
          if (query.startsWith('INSERT INTO nodes')) {
            const [user, stripe_customer, subscription_id, node_id, region, rpc_url, created_at, updated_at] =
              bound as [string, string, string, string, string, string, number, number]
            if (rows.has(subscription_id)) {
              throw new Error('UNIQUE constraint failed: nodes.subscription_id')
            }
            rows.set(subscription_id, {
              user,
              stripe_customer,
              subscription_id,
              node_id,
              instance_id: null,
              region,
              rpc_url,
              state: 'provisioning',
              created_at,
              updated_at,
            })
            return { success: true }
          }
          if (query.startsWith('UPDATE nodes SET instance_id')) {
            const [instance_id, updated_at, subscription_id] = bound as [string, number, string]
            const r = rows.get(subscription_id)
            if (r) {
              r.instance_id = instance_id
              r.updated_at = updated_at
            }
            return { success: true }
          }
          if (query.startsWith('UPDATE nodes SET state')) {
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
  nodeId: 'r1',
  region: 'us-west-2',
  rpcUrl: 'https://node-r1.testnet.botho.io/rpc',
}

describe('D1NodeStore', () => {
  it('inserts and reads back a provisioning row', async () => {
    const { db } = fakeD1()
    const store = new D1NodeStore(db, () => 1000)
    const rec = await store.insertProvisioning(NEW)
    expect(rec.state).toBe('provisioning')
    expect(rec.instanceId).toBeNull()

    const got = await store.getBySubscription('sub_1')
    expect(got?.nodeId).toBe('r1')
    expect(got?.rpcUrl).toBe('https://node-r1.testnet.botho.io/rpc')
  })

  it('rejects a duplicate subscription with DuplicateSubscriptionError', async () => {
    const { db } = fakeD1()
    const store = new D1NodeStore(db, () => 1000)
    await store.insertProvisioning(NEW)
    await expect(store.insertProvisioning(NEW)).rejects.toBeInstanceOf(DuplicateSubscriptionError)
  })

  it('attaches the instance id and transitions state', async () => {
    const { db } = fakeD1()
    const store = new D1NodeStore(db, () => 1000)
    await store.insertProvisioning(NEW)
    await store.setInstanceId('sub_1', 'i-123')
    await store.setState('sub_1', 'running')
    const got = await store.getBySubscription('sub_1')
    expect(got?.instanceId).toBe('i-123')
    expect(got?.state).toBe('running')
  })

  it('getByCustomer returns the customer’s node, preferring a live one', async () => {
    const { db } = fakeD1()
    let t = 1000
    const store = new D1NodeStore(db, () => t)
    // Older terminated node for the customer.
    await store.insertProvisioning(NEW)
    await store.setState('sub_1', 'terminated')
    // Newer running node for the same customer.
    t = 2000
    await store.insertProvisioning({
      ...NEW,
      subscriptionId: 'sub_2',
      nodeId: 'r2',
      rpcUrl: 'https://node-r2.testnet.botho.io/rpc',
    })
    await store.setState('sub_2', 'running')

    const got = await store.getByCustomer('cus_X')
    expect(got?.nodeId).toBe('r2')
    expect(got?.state).toBe('running')

    // A different customer gets nothing (authz boundary).
    expect(await store.getByCustomer('cus_OTHER')).toBeUndefined()
  })

  it('counts only active (non-terminated) rows', async () => {
    const { db } = fakeD1()
    const store = new D1NodeStore(db, () => 1000)
    await store.insertProvisioning(NEW)
    await store.insertProvisioning({ ...NEW, subscriptionId: 'sub_2', nodeId: 'r2' })
    expect(await store.countActive()).toBe(2)
    await store.setState('sub_2', 'terminated')
    expect(await store.countActive()).toBe(1)
  })
})
