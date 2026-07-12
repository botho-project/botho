import { describe, it, expect, vi } from 'vitest'
import {
  appendSessionIdTemplate,
  buildCheckoutSessionParams,
  createCheckoutSession,
  isAllowedRegion,
  isTestModeKey,
  missingEnvKeys,
  REGION_ALLOWLIST,
  StripeCheckoutError,
  validateCheckoutRequest,
  type CheckoutEnv,
} from './checkout'

const ENV: CheckoutEnv = {
  STRIPE_SECRET_KEY: 'sk_test_dummy',
  STRIPE_PRICE_ID: 'price_test_50mo',
  CHECKOUT_SUCCESS_URL: 'https://botho.io/node/success',
  CHECKOUT_CANCEL_URL: 'https://botho.io/node',
}

describe('region allowlist', () => {
  it('starts with only us-west-2 (#458 §5)', () => {
    expect([...REGION_ALLOWLIST]).toEqual(['us-west-2'])
  })

  it('accepts an allowed region', () => {
    expect(isAllowedRegion('us-west-2')).toBe(true)
  })

  it('rejects an off-list region', () => {
    expect(isAllowedRegion('us-east-1')).toBe(false)
    expect(isAllowedRegion('')).toBe(false)
  })
})

describe('validateCheckoutRequest', () => {
  it('accepts a minimal valid request', () => {
    const r = validateCheckoutRequest({ region: 'us-west-2' })
    expect(r.ok).toBe(true)
    if (r.ok) {
      expect(r.value.region).toBe('us-west-2')
      expect(r.value.email).toBeUndefined()
    }
  })

  it('accepts a valid email', () => {
    const r = validateCheckoutRequest({ region: 'us-west-2', email: 'a@b.co' })
    expect(r.ok).toBe(true)
    if (r.ok) expect(r.value.email).toBe('a@b.co')
  })

  it('accepts a catalog preferredRegion (demand capture)', () => {
    const r = validateCheckoutRequest({ region: 'us-west-2', preferredRegion: 'af-south-1' })
    expect(r.ok).toBe(true)
    if (r.ok) expect(r.value.preferredRegion).toBe('af-south-1')
  })

  it('rejects a preferredRegion outside the catalog', () => {
    const r = validateCheckoutRequest({ region: 'us-west-2', preferredRegion: 'mars-north-1' })
    expect(r.ok).toBe(false)
  })

  it('rejects a non-string preferredRegion', () => {
    const r = validateCheckoutRequest({ region: 'us-west-2', preferredRegion: 42 })
    expect(r.ok).toBe(false)
  })

  it('preferredRegion never widens the provisioning region', () => {
    // A catalog region is still NOT a valid provisioning region.
    const r = validateCheckoutRequest({ region: 'af-south-1' })
    expect(r.ok).toBe(false)
  })

  it('rejects a non-object body', () => {
    expect(validateCheckoutRequest(null).ok).toBe(false)
    expect(validateCheckoutRequest('nope').ok).toBe(false)
  })

  it('rejects a missing region', () => {
    const r = validateCheckoutRequest({})
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error).toMatch(/region is required/)
  })

  it('rejects an off-allowlist region (defense in depth)', () => {
    const r = validateCheckoutRequest({ region: 'eu-central-1' })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error).toMatch(/allowlist/)
  })

  it('rejects a malformed email', () => {
    const r = validateCheckoutRequest({ region: 'us-west-2', email: 'not-an-email' })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error).toMatch(/email/)
  })

  it('treats empty-string email as absent', () => {
    const r = validateCheckoutRequest({ region: 'us-west-2', email: '' })
    expect(r.ok).toBe(true)
    if (r.ok) expect(r.value.email).toBeUndefined()
  })
})

describe('missingEnvKeys', () => {
  it('reports nothing when fully configured', () => {
    expect(missingEnvKeys(ENV)).toEqual([])
  })

  it('reports each missing key', () => {
    expect(missingEnvKeys({ ...ENV, STRIPE_SECRET_KEY: '' })).toEqual(['STRIPE_SECRET_KEY'])
    expect(missingEnvKeys({})).toEqual([
      'STRIPE_SECRET_KEY',
      'STRIPE_PRICE_ID',
      'CHECKOUT_SUCCESS_URL',
      'CHECKOUT_CANCEL_URL',
    ])
  })
})

describe('isTestModeKey', () => {
  it('detects test keys', () => {
    expect(isTestModeKey('sk_test_abc')).toBe(true)
    expect(isTestModeKey('rk_test_abc')).toBe(true)
  })
  it('treats live keys as non-test', () => {
    expect(isTestModeKey('sk_live_abc')).toBe(false)
  })
})

describe('appendSessionIdTemplate', () => {
  it('adds the Stripe session template with ? when no query', () => {
    expect(appendSessionIdTemplate('https://x/y')).toBe(
      'https://x/y?session_id={CHECKOUT_SESSION_ID}',
    )
  })
  it('uses & when a query already exists', () => {
    expect(appendSessionIdTemplate('https://x/y?a=1')).toBe(
      'https://x/y?a=1&session_id={CHECKOUT_SESSION_ID}',
    )
  })
})

describe('buildCheckoutSessionParams', () => {
  const params = buildCheckoutSessionParams({ region: 'us-west-2' }, ENV)

  it('uses subscription mode', () => {
    expect(params.get('mode')).toBe('subscription')
  })

  it('uses the env price id (not a hard-coded live id) with quantity 1', () => {
    expect(params.get('line_items[0][price]')).toBe('price_test_50mo')
    expect(params.get('line_items[0][quantity]')).toBe('1')
  })

  it('sets success/cancel urls and appends the session-id template', () => {
    expect(params.get('success_url')).toBe(
      'https://botho.io/node/success?session_id={CHECKOUT_SESSION_ID}',
    )
    expect(params.get('cancel_url')).toBe('https://botho.io/node')
  })

  it('captures region on both session and subscription metadata (#458 §3)', () => {
    expect(params.get('metadata[region]')).toBe('us-west-2')
    expect(params.get('subscription_data[metadata][region]')).toBe('us-west-2')
  })

  it('omits preferred_region metadata when not supplied', () => {
    expect(params.has('metadata[preferred_region]')).toBe(false)
    expect(params.has('subscription_data[metadata][preferred_region]')).toBe(false)
  })

  it('captures preferred_region on both metadata surfaces (demand data)', () => {
    const p = buildCheckoutSessionParams(
      { region: 'us-west-2', preferredRegion: 'af-south-1' },
      ENV,
    )
    expect(p.get('metadata[preferred_region]')).toBe('af-south-1')
    expect(p.get('subscription_data[metadata][preferred_region]')).toBe('af-south-1')
    // The provisioning region is unchanged by the preference.
    expect(p.get('metadata[region]')).toBe('us-west-2')
  })

  it('does not set customer_creation (subscription mode creates a Customer implicitly; Stripe rejects the param)', () => {
    expect(params.has('customer_creation')).toBe(false)
  })

  it('omits customer_email when no email given', () => {
    expect(params.has('customer_email')).toBe(false)
  })

  it('includes customer_email when provided', () => {
    const p = buildCheckoutSessionParams({ region: 'us-west-2', email: 'a@b.co' }, ENV)
    expect(p.get('customer_email')).toBe('a@b.co')
  })
})

describe('createCheckoutSession', () => {
  it('POSTs a correct request to Stripe and returns id+url', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(JSON.stringify({ id: 'cs_test_123', url: 'https://checkout.stripe.com/c/123' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )

    const result = await createCheckoutSession(
      { region: 'us-west-2', email: 'a@b.co' },
      ENV,
      fetchMock as unknown as typeof fetch,
    )

    expect(result).toEqual({ id: 'cs_test_123', url: 'https://checkout.stripe.com/c/123' })

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [calledUrl, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(calledUrl).toBe('https://api.stripe.com/v1/checkout/sessions')
    expect(init.method).toBe('POST')

    const headers = init.headers as Record<string, string>
    expect(headers.Authorization).toBe('Bearer sk_test_dummy')
    expect(headers['Content-Type']).toBe('application/x-www-form-urlencoded')
    expect(headers['Stripe-Version']).toBe('2024-06-20')

    // Body is form-encoded and carries the key fields.
    const sent = new URLSearchParams(init.body as string)
    expect(sent.get('mode')).toBe('subscription')
    expect(sent.get('line_items[0][price]')).toBe('price_test_50mo')
    expect(sent.get('metadata[region]')).toBe('us-west-2')
    expect(sent.get('customer_email')).toBe('a@b.co')
  })

  it('throws StripeCheckoutError on a Stripe error response', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(JSON.stringify({ error: { message: 'No such price' } }), {
        status: 400,
        headers: { 'Content-Type': 'application/json' },
      }),
    )

    await expect(
      createCheckoutSession({ region: 'us-west-2' }, ENV, fetchMock as unknown as typeof fetch),
    ).rejects.toBeInstanceOf(StripeCheckoutError)
  })

  it('throws when Stripe omits id/url even with 200', async () => {
    const fetchMock = vi.fn(async () =>
      new Response(JSON.stringify({ id: 'cs_test_1' }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
    )
    await expect(
      createCheckoutSession({ region: 'us-west-2' }, ENV, fetchMock as unknown as typeof fetch),
    ).rejects.toBeInstanceOf(StripeCheckoutError)
  })
})
