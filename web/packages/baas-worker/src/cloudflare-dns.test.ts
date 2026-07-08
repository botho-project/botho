import { describe, it, expect, vi } from 'vitest'
import { HttpDnsClient } from './cloudflare-dns'

function cfJson(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

describe('HttpDnsClient.upsertARecord (mocked fetch — no real Cloudflare)', () => {
  it('creates a record when none exists (POST)', async () => {
    const fetchMock = vi
      .fn()
      // findByName -> empty
      .mockResolvedValueOnce(cfJson({ success: true, result: [] }))
      // create -> created
      .mockResolvedValueOnce(
        cfJson({ success: true, result: { id: 'rec1', name: 'node-a.testnet.botho.io', content: '1.2.3.4' } }),
      )

    const client = new HttpDnsClient('token', 'zone1', fetchMock as unknown as typeof fetch)
    const rec = await client.upsertARecord('node-a.testnet.botho.io', '1.2.3.4')
    expect(rec.id).toBe('rec1')

    const [, createInit] = fetchMock.mock.calls[1] as unknown as [string, RequestInit]
    expect(createInit.method).toBe('POST')
    const payload = JSON.parse(createInit.body as string)
    expect(payload).toMatchObject({ type: 'A', name: 'node-a.testnet.botho.io', content: '1.2.3.4' })
    const headers = createInit.headers as Record<string, string>
    expect(headers.Authorization).toBe('Bearer token')
  })

  it('updates an existing record (PUT) — idempotent on retry', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        cfJson({ success: true, result: [{ id: 'rec1', name: 'node-a.testnet.botho.io', content: '9.9.9.9' }] }),
      )
      .mockResolvedValueOnce(
        cfJson({ success: true, result: { id: 'rec1', name: 'node-a.testnet.botho.io', content: '1.2.3.4' } }),
      )

    const client = new HttpDnsClient('token', 'zone1', fetchMock as unknown as typeof fetch)
    await client.upsertARecord('node-a.testnet.botho.io', '1.2.3.4')

    const [updateUrl, updateInit] = fetchMock.mock.calls[1] as unknown as [string, RequestInit]
    expect(updateInit.method).toBe('PUT')
    expect(updateUrl).toContain('/dns_records/rec1')
  })

  it('throws DnsError on a Cloudflare failure', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(
      cfJson({ success: false, errors: [{ message: 'bad token' }] }, 403),
    )
    const client = new HttpDnsClient('token', 'zone1', fetchMock as unknown as typeof fetch)
    await expect(client.upsertARecord('x', '1.2.3.4')).rejects.toThrow(/bad token/)
  })
})

describe('HttpDnsClient.deleteARecord', () => {
  it('deletes when the record exists', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        cfJson({ success: true, result: [{ id: 'rec1', name: 'node-a.testnet.botho.io', content: '1.2.3.4' }] }),
      )
      .mockResolvedValueOnce(cfJson({ success: true, result: { id: 'rec1' } }))

    const client = new HttpDnsClient('token', 'zone1', fetchMock as unknown as typeof fetch)
    await client.deleteARecord('node-a.testnet.botho.io')

    const [, delInit] = fetchMock.mock.calls[1] as unknown as [string, RequestInit]
    expect(delInit.method).toBe('DELETE')
  })

  it('is a no-op when the record is absent', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(cfJson({ success: true, result: [] }))
    const client = new HttpDnsClient('token', 'zone1', fetchMock as unknown as typeof fetch)
    await client.deleteARecord('missing')
    expect(fetchMock).toHaveBeenCalledTimes(1) // only the lookup, no DELETE
  })
})
