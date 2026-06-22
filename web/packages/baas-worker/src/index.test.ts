import { describe, it, expect, vi } from 'vitest'
import worker, { handleCheckout, type Env } from './index'

const ENV: Env = {
  STRIPE_SECRET_KEY: 'sk_test_dummy',
  STRIPE_PRICE_ID: 'price_test_50mo',
  CHECKOUT_SUCCESS_URL: 'https://botho.io/rig/success',
  CHECKOUT_CANCEL_URL: 'https://botho.io/rig',
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

  it('has NO public /provision route (security: launches only via /webhook)', async () => {
    const res = await worker.fetch(
      new Request('https://control.botho.io/provision', { method: 'POST', body: '{}' }),
      ENV,
    )
    expect(res.status).toBe(404)
  })
})
