import { afterEach, expect, it, vi } from 'vitest'
import { LocalNodeAdapter } from '../local'
import { RemoteNodeAdapter } from '../remote'
afterEach(() => vi.unstubAllGlobals())
it.each([undefined, null, 42, {}, '', ' ', 'botho-testnet', 'unrecognized-network'])('never invents a network for %j', async network => {
  vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, json: async () => ({ jsonrpc: '2.0', id: 1, result: { network, version: 'fixture', chainHeight: 0 } }) })))
  const expected = typeof network === 'string' && network.trim() ? network : ''
  const remote = new RemoteNodeAdapter({ seedNodes: ['https://fixture.invalid/rpc'], useWebSocket: false, networkId: 'botho-testnet' })
  if (network === 'botho-testnet') {
    await remote.connect()
    expect(remote.getNodeInfo()?.networkId).toBe(network)
  } else {
    await expect(remote.connect()).rejects.toThrow()
    expect(remote.getNodeInfo()).toBeNull()
  }
  remote.disconnect()
  const local = new LocalNodeAdapter({ host: '127.0.0.1', scanPorts: [12345] })
  const nodes = await local.scanForNodes()
  expect(nodes.length).toBeGreaterThan(0)
  expect(nodes.every(node => node.networkId === expected)).toBe(true)
})

it.each(['botho-mainnet', 'botho-testnet'])('connects only to the explicitly expected %s', async network => {
  vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, json: async () => ({ jsonrpc: '2.0', id: 1, result: { network, version: 'fixture', chainHeight: 0 } }) })))
  const matched = new RemoteNodeAdapter({ seedNodes: ['https://fixture.invalid/rpc'], useWebSocket: false, networkId: network })
  await matched.connect()
  expect(matched.getNodeInfo()?.networkId).toBe(network)
  matched.disconnect()
  const mismatch = new RemoteNodeAdapter({ seedNodes: ['https://fixture.invalid/rpc'], useWebSocket: false, networkId: network === 'botho-mainnet' ? 'botho-testnet' : 'botho-mainnet' })
  await expect(mismatch.connect()).rejects.toThrow()
  expect(mismatch.getNodeInfo()).toBeNull()
})
