import { describe, it, expect, vi } from 'vitest'
import {
  buildStatusResponse,
  buildWalletDeepLink,
  createPortalSession,
  fetchNodeHealth,
  lookupStatusForCustomer,
  StripePortalError,
} from './status'
import { FakeStore } from './test-fakes'
import type { NewNodeRecord, NodeState } from './node-store'

const WALLET = 'https://wallet.botho.io'

function nodeStatusOk(result: Record<string, unknown> = { chainHeight: 42, synced: true }) {
  return vi.fn(async () =>
    new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, result }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    }),
  )
}

async function seedNode(
  store: FakeStore,
  over: Partial<NewNodeRecord> = {},
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

describe('buildWalletDeepLink', () => {
  it('encodes the node RPC into a /wallet?rpc= deep link', () => {
    const link = buildWalletDeepLink(WALLET, 'https://node-x.testnet.botho.io/rpc')
    expect(link).toBe(
      'https://wallet.botho.io/wallet?rpc=https%3A%2F%2Fnode-x.testnet.botho.io%2Frpc',
    )
  })

  it('strips a trailing slash on the base url', () => {
    const link = buildWalletDeepLink('https://wallet.botho.io/', 'https://n/rpc')
    expect(link.startsWith('https://wallet.botho.io/wallet?rpc=')).toBe(true)
  })
})

describe('fetchNodeHealth', () => {
  it('reports online with chain height + sync from node_getStatus', async () => {
    const fetchMock = nodeStatusOk({ chainHeight: 100, synced: false, syncProgress: 73 })
    const health = await fetchNodeHealth('https://n/rpc', fetchMock as unknown as typeof fetch)
    expect(health.status).toBe('online')
    expect(health.chainHeight).toBe(100)
    expect(health.synced).toBe(false)
    expect(health.syncProgress).toBe(73)
  })

  it('reports offline on an RPC error payload', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, error: { code: -1 } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    const health = await fetchNodeHealth('https://n/rpc', fetchMock as unknown as typeof fetch)
    expect(health.status).toBe('offline')
  })

  it('reports offline when the node fetch throws (never propagates)', async () => {
    const fetchMock = vi.fn(async () => {
      throw new Error('network down')
    })
    const health = await fetchNodeHealth('https://n/rpc', fetchMock as unknown as typeof fetch)
    expect(health.status).toBe('offline')
  })
})

describe('buildStatusResponse', () => {
  it('probes health for a running node', async () => {
    const store = new FakeStore()
    await seedNode(store, {}, 'running')
    const node = await store.getByCustomer('cus_A')
    const fetchMock = nodeStatusOk()
    const status = await buildStatusResponse(node!, WALLET, fetchMock as unknown as typeof fetch)
    expect(status.state).toBe('running')
    expect(status.health.status).toBe('online')
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(status.walletDeepLink).toContain('/wallet?rpc=')
  })

  it('does NOT probe a provisioning node (health unknown, no node call)', async () => {
    const store = new FakeStore()
    await seedNode(store, {}, 'provisioning')
    const node = await store.getByCustomer('cus_A')
    const fetchMock = nodeStatusOk()
    const status = await buildStatusResponse(node!, WALLET, fetchMock as unknown as typeof fetch)
    expect(status.state).toBe('provisioning')
    expect(status.health.status).toBe('unknown')
    expect(fetchMock).not.toHaveBeenCalled()
  })
})

describe('lookupStatusForCustomer (authz)', () => {
  it('returns the requesting customer’s own node', async () => {
    const store = new FakeStore()
    await seedNode(store, { stripeCustomer: 'cus_A', subscriptionId: 'sub_A' }, 'running')
    const result = await lookupStatusForCustomer(
      'cus_A',
      store,
      WALLET,
      nodeStatusOk() as unknown as typeof fetch,
    )
    expect(result.ok).toBe(true)
    if (result.ok) expect(result.status.rpcUrl).toContain('node-abc123')
  })

  it('does NOT return another customer’s node (404, no leak)', async () => {
    const store = new FakeStore()
    // Only customer B has a node.
    await seedNode(
      store,
      {
        stripeCustomer: 'cus_B',
        subscriptionId: 'sub_B',
        nodeId: 'secret',
        rpcUrl: 'https://node-secret.testnet.botho.io/rpc',
      },
      'running',
    )
    // Customer A (authenticated) asks for their node — must get nothing.
    const result = await lookupStatusForCustomer(
      'cus_A',
      store,
      WALLET,
      nodeStatusOk() as unknown as typeof fetch,
    )
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.code).toBe('not_found')
  })

  it('prefers a live node over a terminated one for the same customer', async () => {
    const store = new FakeStore()
    await seedNode(
      store,
      { stripeCustomer: 'cus_A', subscriptionId: 'sub_old', nodeId: 'old', rpcUrl: 'https://old/rpc' },
      'terminated',
    )
    await seedNode(
      store,
      { stripeCustomer: 'cus_A', subscriptionId: 'sub_new', nodeId: 'new', rpcUrl: 'https://node-new/rpc' },
      'running',
    )
    const result = await lookupStatusForCustomer(
      'cus_A',
      store,
      WALLET,
      nodeStatusOk() as unknown as typeof fetch,
    )
    expect(result.ok).toBe(true)
    if (result.ok) expect(result.status.rpcUrl).toBe('https://node-new/rpc')
  })
})

describe('createPortalSession', () => {
  it('posts the verified customer + return url to Stripe and returns the url', async () => {
    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const body = String(init?.body)
      expect(body).toContain('customer=cus_A')
      expect(body).toContain('return_url=')
      return new Response(JSON.stringify({ url: 'https://billing.stripe.com/p/abc' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    })
    const session = await createPortalSession(
      'cus_A',
      'https://botho.io/node/status',
      'sk_test_dummy',
      fetchMock as unknown as typeof fetch,
    )
    expect(session.url).toBe('https://billing.stripe.com/p/abc')
  })

  it('throws StripePortalError on a Stripe failure', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(JSON.stringify({ error: { message: 'No such customer' } }), {
        status: 400,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    await expect(
      createPortalSession('cus_A', 'https://r', 'sk_test_dummy', fetchMock as unknown as typeof fetch),
    ).rejects.toBeInstanceOf(StripePortalError)
  })
})
