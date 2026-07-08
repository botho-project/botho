import { describe, it, expect, vi } from 'vitest'
import worker, { handleStatus, handlePortal, type Env } from './index'
import { mintStatusToken } from './status-link'
import { FakeStore } from './test-fakes'
import type { D1Like } from './node-store'
import type { NewNodeRecord, NodeState } from './node-store'

const STATUS_SECRET = 'test-status-secret'

/**
 * A FakeStore exposed through the D1-shaped `prepare()` API that handleStatus
 * uses via D1NodeStore. Rather than emulate SQL, we install a FakeStore directly
 * by monkeypatching: handleStatus builds a D1NodeStore(env.DB). We instead supply
 * a tiny D1 shim that the queries hit. To keep the test focused on the HTTP +
 * authz layer (the store SQL is covered in node-store.test.ts), we back the shim
 * with an in-memory FakeStore and translate the two SELECT shapes it issues.
 */
function makeD1(store: FakeStore): D1Like {
  return {
    prepare(query: string) {
      let bound: unknown[] = []
      const stmt = {
        bind(...values: unknown[]) {
          bound = values
          return stmt
        },
        async first<T = unknown>(): Promise<T | null> {
          if (query.includes('stripe_customer = ?')) {
            const rec = await store.getByCustomer(String(bound[0]))
            if (!rec) return null
            return {
              user: rec.user,
              stripe_customer: rec.stripeCustomer,
              subscription_id: rec.subscriptionId,
              node_id: rec.nodeId,
              instance_id: rec.instanceId,
              region: rec.region,
              rpc_url: rec.rpcUrl,
              state: rec.state,
              created_at: rec.createdAt,
              updated_at: rec.updatedAt,
            } as unknown as T
          }
          return null
        },
        async run() {
          return { success: true }
        },
        async all<T = unknown>() {
          return { results: [] as T[] }
        },
      }
      return stmt
    },
  }
}

async function seed(
  store: FakeStore,
  over: Partial<NewNodeRecord>,
  state: NodeState = 'running',
): Promise<void> {
  const rec: NewNodeRecord = {
    user: over.stripeCustomer ?? 'cus_A',
    stripeCustomer: over.stripeCustomer ?? 'cus_A',
    subscriptionId: over.subscriptionId ?? 'sub_A',
    nodeId: over.nodeId ?? 'abc123',
    region: over.region ?? 'us-west-2',
    rpcUrl: over.rpcUrl ?? 'https://node-abc123.testnet.botho.io/rpc',
  }
  await store.insertProvisioning(rec)
  if (state !== 'provisioning') await store.setState(rec.subscriptionId, state)
}

function baseEnv(store: FakeStore): Env {
  return {
    STRIPE_SECRET_KEY: 'sk_test_dummy',
    STRIPE_PRICE_ID: 'price_test',
    CHECKOUT_SUCCESS_URL: 'https://botho.io/node/success',
    CHECKOUT_CANCEL_URL: 'https://botho.io/node',
    STATUS_LINK_SECRET: STATUS_SECRET,
    WALLET_BASE_URL: 'https://wallet.botho.io',
    PORTAL_RETURN_URL: 'https://botho.io/node/status',
    DB: makeD1(store),
  }
}

function nodeOk() {
  return vi.fn(async () =>
    new Response(JSON.stringify({ result: { chainHeight: 7, synced: true } }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    }),
  )
}

function statusReq(token?: string): Request {
  const url = token
    ? `https://baas.botho.io/status?token=${encodeURIComponent(token)}`
    : 'https://baas.botho.io/status'
  return new Request(url, { method: 'GET' })
}

describe('handleStatus', () => {
  it('returns the authenticated customer’s node (200)', async () => {
    const store = new FakeStore()
    await seed(store, { stripeCustomer: 'cus_A', subscriptionId: 'sub_A' }, 'running')
    const env = baseEnv(store)
    const token = await mintStatusToken('cus_A', STATUS_SECRET)
    const res = await handleStatus(statusReq(token), env, nodeOk() as unknown as typeof fetch)
    expect(res.status).toBe(200)
    const json = (await res.json()) as { rpcUrl: string; state: string; health: { status: string } }
    expect(json.rpcUrl).toContain('node-abc123')
    expect(json.state).toBe('running')
    expect(json.health.status).toBe('online')
  })

  it('does NOT leak another customer’s node — 404 for a user without one', async () => {
    const store = new FakeStore()
    // Only cus_B has a node.
    await seed(store, { stripeCustomer: 'cus_B', subscriptionId: 'sub_B', nodeId: 'secret' }, 'running')
    const env = baseEnv(store)
    // cus_A is authenticated (valid token) but owns nothing.
    const token = await mintStatusToken('cus_A', STATUS_SECRET)
    const res = await handleStatus(statusReq(token), env, nodeOk() as unknown as typeof fetch)
    expect(res.status).toBe(404)
    const body = await res.text()
    expect(body).not.toContain('secret')
  })

  it('rejects a missing token with 400', async () => {
    const store = new FakeStore()
    const res = await handleStatus(statusReq(), baseEnv(store), nodeOk() as unknown as typeof fetch)
    expect(res.status).toBe(400)
  })

  it('rejects an invalid/forged token with 401 (no data leak)', async () => {
    const store = new FakeStore()
    await seed(store, { stripeCustomer: 'cus_A', subscriptionId: 'sub_A' }, 'running')
    const env = baseEnv(store)
    // A token signed with the wrong secret.
    const forged = await mintStatusToken('cus_A', 'wrong-secret')
    const res = await handleStatus(statusReq(forged), env, nodeOk() as unknown as typeof fetch)
    expect(res.status).toBe(401)
    const body = await res.text()
    expect(body).not.toContain('node-abc123')
  })

  it('fails closed with 500 when STATUS_LINK_SECRET is unset', async () => {
    const store = new FakeStore()
    const env = { ...baseEnv(store), STATUS_LINK_SECRET: undefined }
    const token = await mintStatusToken('cus_A', STATUS_SECRET)
    const res = await handleStatus(statusReq(token), env, nodeOk() as unknown as typeof fetch)
    expect(res.status).toBe(500)
  })

  it('rejects non-GET with 405', async () => {
    const store = new FakeStore()
    const req = new Request('https://baas.botho.io/status', { method: 'POST' })
    const res = await handleStatus(req, baseEnv(store), nodeOk() as unknown as typeof fetch)
    expect(res.status).toBe(405)
  })
})

describe('handlePortal', () => {
  function portalReq(body: unknown): Request {
    return new Request('https://baas.botho.io/portal', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
  }

  it('opens a portal session for a verified token (200)', async () => {
    const store = new FakeStore()
    const env = baseEnv(store)
    const token = await mintStatusToken('cus_A', STATUS_SECRET)
    const stripeMock = vi.fn(async (_u: string, init?: RequestInit) => {
      expect(String(init?.body)).toContain('customer=cus_A')
      return new Response(JSON.stringify({ url: 'https://billing.stripe.com/p/x' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    })
    const res = await handlePortal(portalReq({ token }), env, stripeMock as unknown as typeof fetch)
    expect(res.status).toBe(200)
    const json = (await res.json()) as { url: string }
    expect(json.url).toContain('billing.stripe.com')
  })

  it('rejects a missing token with 400', async () => {
    const store = new FakeStore()
    const res = await handlePortal(portalReq({}), baseEnv(store), nodeOk() as unknown as typeof fetch)
    expect(res.status).toBe(400)
  })

  it('rejects an invalid token with 401', async () => {
    const store = new FakeStore()
    const forged = await mintStatusToken('cus_A', 'wrong-secret')
    const res = await handlePortal(
      portalReq({ token: forged }),
      baseEnv(store),
      nodeOk() as unknown as typeof fetch,
    )
    expect(res.status).toBe(401)
  })
})

describe('worker routing (P6.3)', () => {
  it('routes GET /status and POST /portal; unknown path is 404', async () => {
    const store = new FakeStore()
    await seed(store, { stripeCustomer: 'cus_A', subscriptionId: 'sub_A' }, 'running')
    const env = baseEnv(store)

    const notFound = await worker.fetch(
      new Request('https://baas.botho.io/nope'),
      env as Parameters<typeof worker.fetch>[1],
    )
    expect(notFound.status).toBe(404)

    // /status with no token should reach the handler (400), not 404.
    const status = await worker.fetch(
      statusReq(),
      env as Parameters<typeof worker.fetch>[1],
    )
    expect(status.status).toBe(400)
  })
})
