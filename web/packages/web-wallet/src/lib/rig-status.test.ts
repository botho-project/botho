import { describe, it, expect, vi } from 'vitest'
import {
  RigStatusError,
  createPortalUrl,
  fetchRigStatus,
  tokenFromSearch,
  type RigStatus,
} from './rig-status'

function okResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

const SAMPLE: RigStatus = {
  rigId: 'abc123',
  rpcUrl: 'https://rig-abc123.testnet.botho.io/rpc',
  state: 'running',
  region: 'us-west-2',
  health: { status: 'online', chainHeight: 42, synced: true },
  walletDeepLink: 'https://wallet.botho.io/wallet?rpc=https%3A%2F%2Frig-abc123%2Frpc',
}

describe('tokenFromSearch', () => {
  it('extracts the token param', () => {
    expect(tokenFromSearch('?token=cus_A.1.sig')).toBe('cus_A.1.sig')
  })
  it('returns null when absent or empty', () => {
    expect(tokenFromSearch('')).toBeNull()
    expect(tokenFromSearch('?other=1')).toBeNull()
    expect(tokenFromSearch('?token=')).toBeNull()
  })
})

describe('fetchRigStatus', () => {
  it('GETs /status?token= and returns the rig status', async () => {
    const fetchMock = vi.fn(async () => okResponse(SAMPLE))
    const result = await fetchRigStatus('cus_A.1.sig', fetchMock as unknown as typeof fetch)
    expect(result.rpcUrl).toBe(SAMPLE.rpcUrl)
    expect(result.health.status).toBe('online')

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toMatch(/\/status\?token=cus_A\.1\.sig$/)
    expect(init.method).toBe('GET')
  })

  it('maps 401 to an expired/invalid link error', async () => {
    const fetchMock = vi.fn(async () => okResponse({ error: 'unauthorized' }, 401))
    await expect(
      fetchRigStatus('bad', fetchMock as unknown as typeof fetch),
    ).rejects.toMatchObject({ status: 401 })
  })

  it('maps 404 to a "no rig yet" error', async () => {
    const fetchMock = vi.fn(async () => okResponse({ error: 'no rig found' }, 404))
    await expect(
      fetchRigStatus('cus_A.1.sig', fetchMock as unknown as typeof fetch),
    ).rejects.toMatchObject({ status: 404 })
  })

  it('throws RigStatusError when the network is unreachable', async () => {
    const fetchMock = vi.fn(async () => {
      throw new Error('network down')
    })
    await expect(
      fetchRigStatus('cus_A.1.sig', fetchMock as unknown as typeof fetch),
    ).rejects.toBeInstanceOf(RigStatusError)
  })
})

describe('createPortalUrl', () => {
  it('POSTs the token and returns the Stripe portal url', async () => {
    const fetchMock = vi.fn(async (_u: string, init?: RequestInit) => {
      expect(JSON.parse(init?.body as string)).toEqual({ token: 'cus_A.1.sig' })
      return okResponse({ url: 'https://billing.stripe.com/p/x' })
    })
    const url = await createPortalUrl('cus_A.1.sig', fetchMock as unknown as typeof fetch)
    expect(url).toBe('https://billing.stripe.com/p/x')
    const [endpoint, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(endpoint).toMatch(/\/portal$/)
    expect(init.method).toBe('POST')
  })

  it('throws on a non-ok portal response', async () => {
    const fetchMock = vi.fn(async () => okResponse({ error: 'unauthorized' }, 401))
    await expect(
      createPortalUrl('bad', fetchMock as unknown as typeof fetch),
    ).rejects.toBeInstanceOf(RigStatusError)
  })
})
