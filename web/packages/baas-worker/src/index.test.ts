import { describe, it, expect, vi } from 'vitest'
import worker, { handleCheckout, handleSessionStatus, type Env } from './index'

const ENV: Env = {
  STRIPE_SECRET_KEY: 'sk_test_dummy',
  STRIPE_PRICE_ID: 'price_test_50mo',
  CHECKOUT_SUCCESS_URL: 'https://botho.io/node/success',
  CHECKOUT_CANCEL_URL: 'https://botho.io/node',
  ALLOWED_ORIGINS: 'https://botho.io',
}

function postCheckout(body: unknown, origin?: string): Request {
  return new Request('https://control.botho.io/checkout', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(origin ? { Origin: origin } : {}),
    },
    body: JSON.stringify(body),
  })
}

function stripeOk() {
  return vi.fn(async () =>
    new Response(JSON.stringify({ id: 'cs_test_abc', url: 'https://checkout.stripe.com/c/abc' }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    }),
  )
}

describe('handleCheckout', () => {
  it('returns 200 with session id+url on a valid request', async () => {
    const fetchMock = stripeOk()
    const res = await handleCheckout(
      postCheckout({ region: 'us-west-2' }),
      ENV,
      fetchMock as unknown as typeof fetch,
    )
    expect(res.status).toBe(200)
    const json = (await res.json()) as { id: string; url: string }
    expect(json.id).toBe('cs_test_abc')
    expect(json.url).toBe('https://checkout.stripe.com/c/abc')
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('rejects non-POST with 405', async () => {
    const req = new Request('https://control.botho.io/checkout', { method: 'GET' })
    const res = await handleCheckout(req, ENV, stripeOk() as unknown as typeof fetch)
    expect(res.status).toBe(405)
  })

  it('returns 400 on invalid JSON', async () => {
    const req = new Request('https://control.botho.io/checkout', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{not json',
    })
    const res = await handleCheckout(req, ENV, stripeOk() as unknown as typeof fetch)
    expect(res.status).toBe(400)
  })

  it('returns 400 on an off-allowlist region without calling Stripe', async () => {
    const fetchMock = stripeOk()
    const res = await handleCheckout(
      postCheckout({ region: 'eu-central-1' }),
      ENV,
      fetchMock as unknown as typeof fetch,
    )
    expect(res.status).toBe(400)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('fails closed with 500 when env is unconfigured (never calls Stripe)', async () => {
    const fetchMock = stripeOk()
    const badEnv = { ...ENV, STRIPE_SECRET_KEY: '' }
    const res = await handleCheckout(
      postCheckout({ region: 'us-west-2' }),
      badEnv,
      fetchMock as unknown as typeof fetch,
    )
    expect(res.status).toBe(500)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('maps a Stripe failure to 502', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(JSON.stringify({ error: { message: 'No such price' } }), {
        status: 400,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    const res = await handleCheckout(
      postCheckout({ region: 'us-west-2' }),
      ENV,
      fetchMock as unknown as typeof fetch,
    )
    expect(res.status).toBe(502)
  })

  it('echoes CORS headers for an allowed origin', async () => {
    const res = await handleCheckout(
      postCheckout({ region: 'us-west-2' }, 'https://botho.io'),
      ENV,
      stripeOk() as unknown as typeof fetch,
    )
    expect(res.headers.get('Access-Control-Allow-Origin')).toBe('https://botho.io')
  })

  it('omits CORS headers for a disallowed origin', async () => {
    const res = await handleCheckout(
      postCheckout({ region: 'us-west-2' }, 'https://evil.example'),
      ENV,
      stripeOk() as unknown as typeof fetch,
    )
    expect(res.headers.get('Access-Control-Allow-Origin')).toBeNull()
  })
})

// Minimal D1 fake: `first<NodeRow>()` returns the seeded row for getByCustomer.
function fakeDbWithNode(row: Record<string, unknown> | null) {
  const stmt = {
    bind: () => stmt,
    first: async () => row,
    run: async () => ({ success: true }),
    all: async () => ({ results: [] }),
  }
  return { prepare: () => stmt } as unknown as Env['DB']
}

const SESSION_ENV: Env = {
  ...ENV,
  STATUS_LINK_SECRET: 'status-secret',
  WALLET_BASE_URL: 'https://botho.io',
}

function stripePaidSession() {
  return vi.fn(async () =>
    new Response(JSON.stringify({ payment_status: 'paid', customer: 'cus_paid' }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    }),
  )
}

describe('handleSessionStatus', () => {
  const NODE_ROW = {
    user: 'cus_paid',
    stripe_customer: 'cus_paid',
    subscription_id: 'sub_1',
    node_id: 'abc123',
    instance_id: 'i-1',
    region: 'us-west-2',
    rpc_url: 'https://node-abc123.testnet.botho.io/rpc',
    state: 'running',
    created_at: 1,
    updated_at: 1,
  }

  function get(sessionId?: string): Request {
    const q = sessionId != null ? `?session_id=${encodeURIComponent(sessionId)}` : ''
    return new Request(`https://control.botho.io/session-status${q}`, { method: 'GET' })
  }

  it('returns 200 + a status URL for a paid session with a provisioned node', async () => {
    const env = { ...SESSION_ENV, DB: fakeDbWithNode(NODE_ROW) }
    const res = await handleSessionStatus(
      get('cs_test_abc'),
      env,
      stripePaidSession() as unknown as typeof fetch,
    )
    expect(res.status).toBe(200)
    const json = (await res.json()) as { status: string; statusUrl: string }
    expect(json.status).toBe('ready')
    expect(json.statusUrl).toContain('https://botho.io/node/status?token=')
  })

  it('returns 202 pending when the session is paid but the node row is absent', async () => {
    const env = { ...SESSION_ENV, DB: fakeDbWithNode(null) }
    const res = await handleSessionStatus(
      get('cs_test_abc'),
      env,
      stripePaidSession() as unknown as typeof fetch,
    )
    expect(res.status).toBe(202)
    const json = (await res.json()) as { status: string }
    expect(json.status).toBe('pending')
  })

  it('returns a generic 401 for an unpaid session (no leak)', async () => {
    const env = { ...SESSION_ENV, DB: fakeDbWithNode(NODE_ROW) }
    const unpaid = vi.fn(async () =>
      new Response(JSON.stringify({ payment_status: 'unpaid', customer: 'cus_paid' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    const res = await handleSessionStatus(get('cs_test_abc'), env, unpaid as unknown as typeof fetch)
    expect(res.status).toBe(401)
  })

  it('returns a generic 401 for an unknown session (Stripe 404)', async () => {
    const env = { ...SESSION_ENV, DB: fakeDbWithNode(NODE_ROW) }
    const missing = vi.fn(async () =>
      new Response(JSON.stringify({ error: { message: 'no such session' } }), {
        status: 404,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    const res = await handleSessionStatus(get('cs_missing'), env, missing as unknown as typeof fetch)
    expect(res.status).toBe(401)
  })

  it('returns 400 when session_id is missing', async () => {
    const env = { ...SESSION_ENV, DB: fakeDbWithNode(NODE_ROW) }
    const res = await handleSessionStatus(get(), env, stripePaidSession() as unknown as typeof fetch)
    expect(res.status).toBe(400)
  })

  it('fails closed with 500 when STATUS_LINK_SECRET is unset (never calls Stripe)', async () => {
    const env = { ...SESSION_ENV, STATUS_LINK_SECRET: '', DB: fakeDbWithNode(NODE_ROW) }
    const fetchMock = stripePaidSession()
    const res = await handleSessionStatus(
      get('cs_test_abc'),
      env,
      fetchMock as unknown as typeof fetch,
    )
    expect(res.status).toBe(500)
    expect(fetchMock).not.toHaveBeenCalled()
  })
})

describe('fetch routing', () => {
  it('responds 200 on /healthz', async () => {
    const res = await worker.fetch(
      new Request('https://control.botho.io/healthz'),
      ENV,
    )
    expect(res.status).toBe(200)
  })

  it('routes /webhook (POST without a valid signature) to the webhook handler -> 400, not 404', async () => {
    // No Stripe-Signature header => the webhook handler rejects with 400. The key
    // assertion is that the route EXISTS (not a 404), proving /webhook is wired.
    const webhookEnv: Env = {
      ...ENV,
      STRIPE_WEBHOOK_SECRET: 'whsec_test',
      // provisioner env so the handler reaches the signature check (not the
      // fail-closed config 500) — the depsFromEnv default is never reached
      // because verification fails first.
      AWS_ACCESS_KEY_ID: 'AKIA_TEST',
      AWS_SECRET_ACCESS_KEY: 'secret',
      CF_DNS_API_TOKEN: 'cf',
      CF_DNS_ZONE_ID: 'zone',
      DB: {} as never,
    }
    const res = await worker.fetch(
      new Request('https://control.botho.io/webhook', { method: 'POST', body: '{}' }),
      webhookEnv,
    )
    expect(res.status).toBe(400)
  })

  it('routes /session-status (missing session_id) to its handler -> 400, not 404', async () => {
    const res = await worker.fetch(
      new Request('https://control.botho.io/session-status', { method: 'GET' }),
      SESSION_ENV,
    )
    // 400 (missing session_id) proves the route EXISTS and is wired.
    expect(res.status).toBe(400)
  })

  it('has NO public /provision route (security: launches only via /webhook)', async () => {
    const res = await worker.fetch(
      new Request('https://control.botho.io/provision', { method: 'POST', body: '{}' }),
      ENV,
    )
    expect(res.status).toBe(404)
  })
})
