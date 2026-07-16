import { describe, expect, it, vi } from 'vitest'
import { BridgeApiError, createBridgeClient } from './bridge-client'
import type { MintOrder } from './types'

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
