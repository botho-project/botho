import { describe, it, expect, vi } from 'vitest'
import {
  buildStatusUrl,
  exchangeSessionForStatus,
  retrieveCheckoutSession,
  StripeSessionError,
} from './session-status'
import { verifyStatusToken } from './status-link'
import { FakeStore } from './test-fakes'
import type { NewNodeRecord } from './node-store'

const SECRET = 'test-status-link-secret'
const STRIPE_KEY = 'sk_test_dummy'
const WALLET_BASE = 'https://botho.io'

function stripeJson(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

function seedNode(store: FakeStore, over: Partial<NewNodeRecord> = {}) {
  return store.insertProvisioning({
    user: 'cus_paid',
    stripeCustomer: 'cus_paid',
    subscriptionId: 'sub_1',
    nodeId: 'abc123',
    region: 'us-west-2',
    rpcUrl: 'https://node-abc123.testnet.botho.io/rpc',
    ...over,
  })
}

describe('retrieveCheckoutSession', () => {
  it('GETs the Stripe session with a pinned API version + bearer auth', async () => {
    const fetchMock = vi.fn(async () =>
      stripeJson({ payment_status: 'paid', customer: 'cus_paid' }),
    )
    const out = await retrieveCheckoutSession(
      'cs_test_abc',
      STRIPE_KEY,
      fetchMock as unknown as typeof fetch,
    )
    expect(out.paymentStatus).toBe('paid')
    expect(out.customerId).toBe('cus_paid')

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('https://api.stripe.com/v1/checkout/sessions/cs_test_abc')
    expect(init.method).toBe('GET')
    const headers = init.headers as Record<string, string>
    expect(headers.Authorization).toBe(`Bearer ${STRIPE_KEY}`)
    expect(headers['Stripe-Version']).toBe('2024-06-20')
  })

  it('coerces an expanded customer object to its id', async () => {
    const fetchMock = vi.fn(async () =>
      stripeJson({ payment_status: 'paid', customer: { id: 'cus_paid' } }),
    )
    const out = await retrieveCheckoutSession(
      'cs_test_abc',
      STRIPE_KEY,
      fetchMock as unknown as typeof fetch,
    )
    expect(out.customerId).toBe('cus_paid')
  })

  it('throws StripeSessionError on a 404', async () => {
    const fetchMock = vi.fn(async () => stripeJson({ error: { message: 'no such session' } }, 404))
    await expect(
      retrieveCheckoutSession('cs_missing', STRIPE_KEY, fetchMock as unknown as typeof fetch),
    ).rejects.toBeInstanceOf(StripeSessionError)
  })
})

describe('buildStatusUrl', () => {
  it('appends the token to the /node/status route and trims trailing slashes', () => {
    expect(buildStatusUrl('https://botho.io/', 'cus_A.1.sig')).toBe(
      'https://botho.io/node/status?token=cus_A.1.sig',
    )
  })
})

describe('exchangeSessionForStatus', () => {
  const opts = (store: FakeStore, fetchImpl: typeof fetch) => ({
    stripeSecretKey: STRIPE_KEY,
    statusLinkSecret: SECRET,
    walletBaseUrl: WALLET_BASE,
    store,
    fetchImpl,
    nowSeconds: 1_000,
  })

  it('returns a ready status URL with a verifiable token for a paid session + provisioned node', async () => {
    const store = new FakeStore()
    await seedNode(store)
    const fetchMock = vi.fn(async () =>
      stripeJson({ payment_status: 'paid', customer: 'cus_paid' }),
    )
    const result = await exchangeSessionForStatus(
      'cs_test_abc',
      opts(store, fetchMock as unknown as typeof fetch),
    )
    expect(result.kind).toBe('ready')
    if (result.kind !== 'ready') throw new Error('expected ready')
    expect(result.statusUrl).toContain('https://botho.io/node/status?token=')

    // The minted token is valid and binds to the paying customer.
    const verified = await verifyStatusToken(result.token, SECRET, { nowSeconds: 1_001 })
    expect(verified.ok).toBe(true)
    if (verified.ok) expect(verified.customerId).toBe('cus_paid')
  })

  it('returns pending (not an error) when the session is paid but the node row is absent', async () => {
    const store = new FakeStore() // empty — webhook hasn't landed
    const fetchMock = vi.fn(async () =>
      stripeJson({ payment_status: 'paid', customer: 'cus_paid' }),
    )
    const result = await exchangeSessionForStatus(
      'cs_test_abc',
      opts(store, fetchMock as unknown as typeof fetch),
    )
    expect(result.kind).toBe('pending')
  })

  it('rejects an unpaid session generically', async () => {
    const store = new FakeStore()
    await seedNode(store)
    const fetchMock = vi.fn(async () =>
      stripeJson({ payment_status: 'unpaid', customer: 'cus_paid' }),
    )
    const result = await exchangeSessionForStatus(
      'cs_test_abc',
      opts(store, fetchMock as unknown as typeof fetch),
    )
    expect(result.kind).toBe('rejected')
  })

  it('rejects an unknown session (Stripe 404) generically', async () => {
    const store = new FakeStore()
    const fetchMock = vi.fn(async () => stripeJson({ error: { message: 'no such session' } }, 404))
    const result = await exchangeSessionForStatus(
      'cs_unknown',
      opts(store, fetchMock as unknown as typeof fetch),
    )
    expect(result.kind).toBe('rejected')
  })

  it('rejects a malformed session id WITHOUT calling Stripe', async () => {
    const store = new FakeStore()
    const fetchMock = vi.fn(async () => stripeJson({}))
    const result = await exchangeSessionForStatus(
      'not-a-session',
      opts(store, fetchMock as unknown as typeof fetch),
    )
    expect(result.kind).toBe('rejected')
    expect(fetchMock).not.toHaveBeenCalled()
  })
})
