import { describe, it, expect, vi } from 'vitest'
import {
  DEFAULT_RIG_REGION,
  RIG_REGIONS,
  RigCheckoutError,
  startRigCheckout,
} from './rig-checkout'

function okResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

describe('rig region allowlist', () => {
  it('starts with only us-west-2', () => {
    expect(RIG_REGIONS.map((r) => r.id)).toEqual(['us-west-2'])
    expect(DEFAULT_RIG_REGION).toBe('us-west-2')
  })
})

describe('startRigCheckout', () => {
  it('POSTs region (+ email) to /checkout and returns id+url', async () => {
    const fetchMock = vi.fn(async () =>
      okResponse({ id: 'cs_test_1', url: 'https://checkout.stripe.com/c/1' }),
    )

    const result = await startRigCheckout(
      { region: 'us-west-2', email: 'a@b.co' },
      fetchMock as unknown as typeof fetch,
    )

    expect(result).toEqual({ id: 'cs_test_1', url: 'https://checkout.stripe.com/c/1' })

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toMatch(/\/checkout$/)
    expect(init.method).toBe('POST')
    const sent = JSON.parse(init.body as string)
    expect(sent).toEqual({ region: 'us-west-2', email: 'a@b.co' })
  })

  it('omits email when not provided', async () => {
    const fetchMock = vi.fn(async () =>
      okResponse({ id: 'cs_test_2', url: 'https://checkout.stripe.com/c/2' }),
    )
    await startRigCheckout({ region: 'us-west-2' }, fetchMock as unknown as typeof fetch)
    const init = (fetchMock.mock.calls[0] as unknown as [string, RequestInit])[1]
    expect(JSON.parse(init.body as string)).toEqual({ region: 'us-west-2' })
  })

  it('throws RigCheckoutError with the server message on a 4xx', async () => {
    const fetchMock = vi.fn(async () => okResponse({ error: 'region not in allowlist' }, 400))
    await expect(
      startRigCheckout({ region: 'eu-central-1' }, fetchMock as unknown as typeof fetch),
    ).rejects.toMatchObject({ message: 'region not in allowlist', status: 400 })
  })

  it('throws when the response lacks a url', async () => {
    const fetchMock = vi.fn(async () => okResponse({ id: 'cs_test_3' }, 200))
    await expect(
      startRigCheckout({ region: 'us-west-2' }, fetchMock as unknown as typeof fetch),
    ).rejects.toBeInstanceOf(RigCheckoutError)
  })

  it('throws a friendly error when the network is unreachable', async () => {
    const fetchMock = vi.fn(async () => {
      throw new Error('network down')
    })
    await expect(
      startRigCheckout({ region: 'us-west-2' }, fetchMock as unknown as typeof fetch),
    ).rejects.toMatchObject({ message: /Could not reach/ })
  })
})
