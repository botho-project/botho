import { describe, it, expect, vi } from 'vitest'
import {
  buildStatusLinkEmailBody,
  retrieveCustomerEmail,
  sendStatusLinkEmail,
} from './resend'

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

describe('buildStatusLinkEmailBody', () => {
  it('includes the status URL in both text and html, plus a cancel note', () => {
    const { subject, text, html } = buildStatusLinkEmailBody({
      to: 'a@b.com',
      statusUrl: 'https://botho.io/node/status?token=cus_A.1.sig',
    })
    expect(subject).toMatch(/Botho node/i)
    expect(text).toContain('https://botho.io/node/status?token=cus_A.1.sig')
    expect(html).toContain('https://botho.io/node/status?token=cus_A.1.sig')
    expect(text.toLowerCase()).toContain('cancel')
  })

  it('links the billing portal when a manage URL is supplied', () => {
    const { html } = buildStatusLinkEmailBody({
      to: 'a@b.com',
      statusUrl: 'https://botho.io/node/status?token=t',
      manageUrl: 'https://billing.stripe.com/p/session_abc',
    })
    expect(html).toContain('https://billing.stripe.com/p/session_abc')
  })
})

describe('sendStatusLinkEmail', () => {
  it('POSTs to the Resend API with a bearer key and the built payload', async () => {
    const fetchMock = vi.fn(async () => json({ id: 're_123' }))
    const result = await sendStatusLinkEmail(
      { to: 'a@b.com', statusUrl: 'https://botho.io/node/status?token=t' },
      {
        apiKey: 'rk_test',
        from: 'Botho <nodes@botho.io>',
        fetchImpl: fetchMock as unknown as typeof fetch,
      },
    )
    expect(result.ok).toBe(true)
    if (result.ok) expect(result.id).toBe('re_123')

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('https://api.resend.com/emails')
    expect(init.method).toBe('POST')
    const headers = init.headers as Record<string, string>
    expect(headers.Authorization).toBe('Bearer rk_test')
    const payload = JSON.parse(init.body as string) as {
      from: string
      to: string
      subject: string
    }
    expect(payload.from).toBe('Botho <nodes@botho.io>')
    expect(payload.to).toBe('a@b.com')
    expect(payload.subject.length).toBeGreaterThan(0)
  })

  it('returns a non-throwing error result on a Resend 4xx', async () => {
    const fetchMock = vi.fn(async () => json({ message: 'domain not verified' }, 403))
    const result = await sendStatusLinkEmail(
      { to: 'a@b.com', statusUrl: 'https://botho.io/node/status?token=t' },
      { apiKey: 'rk_test', fetchImpl: fetchMock as unknown as typeof fetch },
    )
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.status).toBe(403)
  })
})

describe('retrieveCustomerEmail', () => {
  it('reads the email off the Stripe customer', async () => {
    const fetchMock = vi.fn(async () => json({ id: 'cus_A', email: 'buyer@example.com' }))
    const email = await retrieveCustomerEmail(
      'cus_A',
      'sk_test',
      fetchMock as unknown as typeof fetch,
    )
    expect(email).toBe('buyer@example.com')
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('https://api.stripe.com/v1/customers/cus_A')
    const headers = init.headers as Record<string, string>
    expect(headers['Stripe-Version']).toBe('2024-06-20')
  })

  it('returns undefined (never throws) on error', async () => {
    const fetchMock = vi.fn(async () => json({ error: 'nope' }, 404))
    const email = await retrieveCustomerEmail(
      'cus_missing',
      'sk_test',
      fetchMock as unknown as typeof fetch,
    )
    expect(email).toBeUndefined()
  })
})
