import { describe, expect, it, vi } from 'vitest'
import { BridgeApiError, createBridgeClient } from './bridge-client'
import type { MintOrder, ReleaseOrder } from './types'

const ORDER: MintOrder = {
  id: '11111111-1111-1111-1111-111111111111',
  status: 'awaiting_deposit',
  destChain: 'ethereum',
  destAddress: '0x49b985ec427ee771a601f11b18f7d4402fa2dd7b',
  amount: '5000000000000',
  fee: '100000000',
  depositAddress: 'tbotho://1/reservedeposit',
  memo: '11111111111111111111111111111111',
  destTx: null,
  expiresAt: 1_760_000_000,
  failureReason: null,
}

function jsonResponse(body: unknown, ok = true, status = 200): Response {
  return {
    ok,
    status,
    text: async () => JSON.stringify(body),
  } as unknown as Response
}

describe('createBridgeClient', () => {
  it('POSTs order-create to the normalized base and returns the order', async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(ORDER))
    // Trailing slash must be stripped so the path never doubles up.
    const client = createBridgeClient('https://bridge.test/', fetchImpl as unknown as typeof fetch)

    const order = await client.createMintOrder({
      destChain: 'ethereum',
      destAddress: ORDER.destAddress,
      amount: ORDER.amount,
    })

    expect(order).toEqual(ORDER)
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('https://bridge.test/api/bridge/orders')
    expect(init.method).toBe('POST')
    expect(JSON.parse(init.body as string)).toEqual({
      destChain: 'ethereum',
      destAddress: ORDER.destAddress,
      amount: ORDER.amount,
    })
  })

  it('GETs order status by id (url-encoded)', async () => {
    const fetchImpl = vi.fn(async () => jsonResponse({ ...ORDER, status: 'completed' }))
    const client = createBridgeClient('https://bridge.test', fetchImpl as unknown as typeof fetch)

    const order = await client.getOrderStatus(ORDER.id)

    expect(order.status).toBe('completed')
    const [url] = fetchImpl.mock.calls[0] as unknown as [string]
    expect(url).toBe(`https://bridge.test/api/bridge/orders/${ORDER.id}`)
  })

  it('surfaces a server error body as a BridgeApiError', async () => {
    const fetchImpl = vi.fn(async () => jsonResponse({ error: 'amount below fee' }, false, 400))
    const client = createBridgeClient('https://bridge.test', fetchImpl as unknown as typeof fetch)

    await expect(
      client.createMintOrder({ destChain: 'ethereum', destAddress: ORDER.destAddress, amount: '1' }),
    ).rejects.toMatchObject({ message: 'amount below fee', status: 400 })
    await expect(
      client.createMintOrder({ destChain: 'ethereum', destAddress: ORDER.destAddress, amount: '1' }),
    ).rejects.toBeInstanceOf(BridgeApiError)
  })
})

const RELEASE: ReleaseOrder = {
  id: '22222222-2222-2222-2222-222222222222',
  status: 'burn_detected',
  sourceChain: 'ethereum',
  bthAddress: 'tbotho://2/releasedest',
  amount: '5000000000000',
  fee: '100000000',
  tokenAddress: '0x49b985ec427ee771a601f11b18f7d4402fa2dd7b',
  sourceTx: '0xburntx',
  destTx: null,
  expiresAt: 1_760_000_000,
  failureReason: null,
}

describe('createBridgeClient — release orders (#1032)', () => {
  it('POSTs release-order create to the normalized base and returns the order', async () => {
    const fetchImpl = vi.fn(async () => jsonResponse(RELEASE))
    const client = createBridgeClient('https://bridge.test/', fetchImpl as unknown as typeof fetch)

    const order = await client.createReleaseOrder({
      sourceChain: 'ethereum',
      bthAddress: RELEASE.bthAddress,
      amount: RELEASE.amount,
    })

    expect(order).toEqual(RELEASE)
    const [url, init] = fetchImpl.mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('https://bridge.test/api/bridge/release-orders')
    expect(init.method).toBe('POST')
    expect(JSON.parse(init.body as string)).toEqual({
      sourceChain: 'ethereum',
      bthAddress: RELEASE.bthAddress,
      amount: RELEASE.amount,
    })
  })

  it('GETs release-order status by id (url-encoded)', async () => {
    const fetchImpl = vi.fn(async () => jsonResponse({ ...RELEASE, status: 'released' }))
    const client = createBridgeClient('https://bridge.test', fetchImpl as unknown as typeof fetch)

    const order = await client.getReleaseOrderStatus(RELEASE.id)

    expect(order.status).toBe('released')
    const [url] = fetchImpl.mock.calls[0] as unknown as [string]
    expect(url).toBe(`https://bridge.test/api/bridge/release-orders/${RELEASE.id}`)
  })
})

describe('createBridgeClient — bridge stats (#1054)', () => {
  it('GETs the aggregate stats from the normalized base', async () => {
    const zero = { count: 0, volume: '0' }
    const window = { completed: zero, pending: zero, expired: zero, failed: zero }
    const STATS = {
      generatedAt: 1_760_000_000,
      wraps: {
        last24h: { ...window, completed: { count: 2, volume: '5000000000000' } },
        allTime: window,
      },
      unwraps: { last24h: window, allTime: window },
    }
    const fetchImpl = vi.fn(async () => jsonResponse(STATS))
    const client = createBridgeClient('https://bridge.test/', fetchImpl as unknown as typeof fetch)

    const stats = await client.getBridgeStats()

    expect(stats).toEqual(STATS)
    const [url] = fetchImpl.mock.calls[0] as unknown as [string]
    expect(url).toBe('https://bridge.test/api/bridge/stats')
  })
})
