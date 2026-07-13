import { describe, it, expect, vi } from 'vitest'
import {
  NodeStatusError,
  createPortalUrl,
  fetchNodeStatus,
  fetchSessionStatus,
  sessionIdFromSearch,
  tokenFromSearch,
  type NodeStatus,
} from './node-status'

function okResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

const SAMPLE: NodeStatus = {
  nodeId: 'abc123',
  rpcUrl: 'https://node-abc123.testnet.botho.io/rpc',
  state: 'running',
  region: 'us-west-2',
  health: { status: 'online', chainHeight: 42, synced: true },
  walletDeepLink: 'https://wallet.botho.io/wallet?rpc=https%3A%2F%2Fnode-abc123%2Frpc',
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

describe('fetchNodeStatus', () => {
  it('GETs /status?token= and returns the node status', async () => {
    const fetchMock = vi.fn(async () => okResponse(SAMPLE))
    const result = await fetchNodeStatus('cus_A.1.sig', fetchMock as unknown as typeof fetch)
    expect(result.rpcUrl).toBe(SAMPLE.rpcUrl)
    expect(result.health.status).toBe('online')

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toMatch(/\/status\?token=cus_A\.1\.sig$/)
    expect(init.method).toBe('GET')
  })

  it('maps 401 to an expired/invalid link error', async () => {
    const fetchMock = vi.fn(async () => okResponse({ error: 'unauthorized' }, 401))
    await expect(
      fetchNodeStatus('bad', fetchMock as unknown as typeof fetch),
    ).rejects.toMatchObject({ status: 401 })
  })

  it('maps 404 to a "no node yet" error', async () => {
    const fetchMock = vi.fn(async () => okResponse({ error: 'no node found' }, 404))
    await expect(
      fetchNodeStatus('cus_A.1.sig', fetchMock as unknown as typeof fetch),
    ).rejects.toMatchObject({ status: 404 })
  })

  it('throws NodeStatusError when the network is unreachable', async () => {
    const fetchMock = vi.fn(async () => {
      throw new Error('network down')
    })
    await expect(
      fetchNodeStatus('cus_A.1.sig', fetchMock as unknown as typeof fetch),
    ).rejects.toBeInstanceOf(NodeStatusError)
  })
})

describe('sessionIdFromSearch', () => {
  it('extracts the session_id param', () => {
    expect(sessionIdFromSearch('?session_id=cs_test_abc')).toBe('cs_test_abc')
  })
  it('returns null when absent or empty', () => {
    expect(sessionIdFromSearch('')).toBeNull()
    expect(sessionIdFromSearch('?token=x')).toBeNull()
    expect(sessionIdFromSearch('?session_id=')).toBeNull()
  })
})

describe('fetchSessionStatus', () => {
  it('GETs /session-status?session_id= and returns a ready status URL on 200', async () => {
    const fetchMock = vi.fn(async () =>
      okResponse({ status: 'ready', statusUrl: 'https://botho.io/node/status?token=t' }),
    )
    const result = await fetchSessionStatus('cs_test_abc', fetchMock as unknown as typeof fetch)
    expect(result).toEqual({ kind: 'ready', statusUrl: 'https://botho.io/node/status?token=t' })
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toMatch(/\/session-status\?session_id=cs_test_abc$/)
    expect(init.method).toBe('GET')
  })

  it('maps a 202 to a pending result (keep polling)', async () => {
    const fetchMock = vi.fn(async () => okResponse({ status: 'pending' }, 202))
    const result = await fetchSessionStatus('cs_test_abc', fetchMock as unknown as typeof fetch)
    expect(result).toEqual({ kind: 'pending' })
  })

  it('throws a terminal 401 for an unknown/unpaid session (stop polling)', async () => {
    const fetchMock = vi.fn(async () => okResponse({ error: 'unauthorized' }, 401))
    await expect(
      fetchSessionStatus('cs_bad', fetchMock as unknown as typeof fetch),
    ).rejects.toMatchObject({ status: 401 })
  })

  it('throws NodeStatusError when the network is unreachable', async () => {
    const fetchMock = vi.fn(async () => {
      throw new Error('network down')
    })
    await expect(
      fetchSessionStatus('cs_test_abc', fetchMock as unknown as typeof fetch),
    ).rejects.toBeInstanceOf(NodeStatusError)
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
    ).rejects.toBeInstanceOf(NodeStatusError)
  })
})
