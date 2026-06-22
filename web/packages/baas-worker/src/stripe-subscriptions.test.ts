import { describe, it, expect } from 'vitest'
import {
  HttpSubscriptionChecker,
  StripeSubscriptionError,
  isActiveSubscriptionStatus,
} from './stripe-subscriptions'

/** Build a fetch stub that returns a single canned response. */
function fetchStub(status: number, body: unknown): typeof fetch {
  return (async () =>
    new Response(JSON.stringify(body), {
      status,
      headers: { 'Content-Type': 'application/json' },
    })) as unknown as typeof fetch
}

describe('isActiveSubscriptionStatus', () => {
  it('treats active and trialing as active', () => {
    expect(isActiveSubscriptionStatus('active')).toBe(true)
    expect(isActiveSubscriptionStatus('trialing')).toBe(true)
  })
  it('treats everything else as inactive', () => {
    for (const s of [
      'canceled',
      'unpaid',
      'past_due',
      'incomplete',
      'incomplete_expired',
      'paused',
      undefined,
    ]) {
      expect(isActiveSubscriptionStatus(s as string)).toBe(false)
    }
  })
})

describe('HttpSubscriptionChecker', () => {
  const KEY = 'sk_test_x'

  it('returns true for an active subscription', async () => {
    const checker = new HttpSubscriptionChecker(KEY, fetchStub(200, { status: 'active' }))
    expect(await checker.isActive('sub_1')).toBe(true)
  })

  it('returns false for a cancelled subscription', async () => {
    const checker = new HttpSubscriptionChecker(KEY, fetchStub(200, { status: 'canceled' }))
    expect(await checker.isActive('sub_1')).toBe(false)
  })

  it('returns false for a 404 (subscription does not exist)', async () => {
    const checker = new HttpSubscriptionChecker(
      KEY,
      fetchStub(404, { error: { message: 'No such subscription' } }),
    )
    expect(await checker.isActive('sub_missing')).toBe(false)
  })

  it('THROWS on a transient (non-404) error so the sweep skips, not reaps', async () => {
    const checker = new HttpSubscriptionChecker(
      KEY,
      fetchStub(503, { error: { message: 'service unavailable' } }),
    )
    await expect(checker.isActive('sub_1')).rejects.toBeInstanceOf(
      StripeSubscriptionError,
    )
  })

  it('sends the Bearer token and hits the subscriptions endpoint', async () => {
    let seenUrl = ''
    let seenAuth = ''
    const fetchImpl = (async (url: string, init?: RequestInit) => {
      seenUrl = url
      seenAuth = (init?.headers as Record<string, string>)?.Authorization ?? ''
      return new Response(JSON.stringify({ status: 'active' }), { status: 200 })
    }) as unknown as typeof fetch
    const checker = new HttpSubscriptionChecker(KEY, fetchImpl)
    await checker.isActive('sub_abc')
    expect(seenUrl).toBe('https://api.stripe.com/v1/subscriptions/sub_abc')
    expect(seenAuth).toBe(`Bearer ${KEY}`)
  })
})
