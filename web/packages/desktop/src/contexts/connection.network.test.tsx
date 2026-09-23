// @vitest-environment jsdom
import { act, cleanup, render } from '@testing-library/react'
import { beforeEach, afterEach, expect, it, vi } from 'vitest'
import { ConnectionProvider, useConnection } from './connection'
const mock = vi.hoisted(() => ({ clients: [] as Array<{ endpoint: string; finish: () => void; disconnect: ReturnType<typeof vi.fn<() => void>> }> }))
vi.mock('@botho/adapters', () => ({
  LocalNodeAdapter: class { async scanForNodes() { return [{ id: 'local', host: '127.0.0.1', port: 1, networkId: 'botho-testnet' }] } },
  RemoteNodeAdapter: class {
    entry: typeof mock.clients[number]
    constructor(config: { seedNodes: string[] }) {
      this.entry = { endpoint: config.seedNodes[0], finish: () => {}, disconnect: vi.fn() }
      mock.clients.push(this.entry)
    }
    connect() { return new Promise<void>(resolve => { this.entry.finish = resolve }) }
    disconnect() { this.entry.disconnect() }
    getNodeInfo() { return { id: 'adapter-id', host: 'fixture.invalid', port: 443, networkId: this.entry.endpoint.includes('first') ? 'botho-testnet' : 'botho-mainnet' } }
  },
}))
let connection: ReturnType<typeof useConnection>
function Capture() { connection = useConnection(); return null }
beforeEach(() => {
  mock.clients.length = 0
  const data = new Map<string, string>()
  vi.stubGlobal('localStorage', { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => data.set(k, v), removeItem: (k: string) => data.delete(k) })
})
afterEach(() => { cleanup(); vi.unstubAllGlobals() })
it('retains the exact selected URL and rejects a late previous connection', async () => {
  render(<ConnectionProvider><Capture /></ConnectionProvider>)
  let first!: Promise<void>, second!: Promise<void>
  act(() => { first = connection.connectToNode({ id: 'https://first.invalid/a/rpc', host: 'ignored', port: 443, networkId: '', status: 'online' }) })
  act(() => { second = connection.connectToNode({ id: 'http://second.invalid:123/b/rpc', host: 'ignored', port: 1, networkId: '', status: 'online' }) })
  await act(async () => { mock.clients[1].finish(); await second })
  await act(async () => { mock.clients[0].finish(); await first })
  expect(connection.endpoint).toBe('http://second.invalid:123/b/rpc')
  expect(connection.connectedNode?.networkId).toBe('botho-mainnet')
  expect(mock.clients[0].disconnect).toHaveBeenCalled()
})
