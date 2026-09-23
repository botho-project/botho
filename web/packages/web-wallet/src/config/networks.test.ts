/**
 * @vitest-environment jsdom
 *
 * Unit coverage for the custom-RPC persistence + validation path (#806).
 *
 * The deep-link + trust-gate flow is covered in `contexts/network-deep-link.test.tsx`;
 * this suite exercises the lower-level config helpers the manual-entry picker
 * relies on: the localStorage persistence round-trip, and the HTTPS-shape /
 * reachability / network-match gates in `validateRpcEndpointForNetwork`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { RemoteNodeAdapter } from '@botho/adapters'
import {
  EXPECTED_NETWORK_ID,
  createCustomNetwork,
  fetchNodeHealth,
  saveSelectedNetwork,
  loadSelectedNetwork,
  saveSelectedIngress,
  loadSelectedIngress,
  validateRpcEndpointForNetwork,
  DEFAULT_INGRESS_ID,
  DEFAULT_NETWORK_ID,
} from './networks'

// jsdom serves an opaque origin, so localStorage is unavailable — shim it.
const localStorageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => {
      store[key] = value
    },
    removeItem: (key: string) => {
      delete store[key]
    },
    clear: () => {
      store = {}
    },
  }
})()
Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock })

/** Build a fetch stub whose node_getStatus returns the given result object. */
function stubStatus(result: Record<string, unknown> | null, ok = true) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => ({
      ok,
      json: async () => (result === null ? { error: 'boom' } : { jsonrpc: '2.0', id: 1, result }),
    })),
  )
}

describe('custom-endpoint persistence round-trip', () => {
  beforeEach(() => localStorage.clear())
  afterEach(() => localStorage.clear())

  it('saves and restores a custom endpoint via saveSelectedNetwork/loadSelectedNetwork', () => {
    saveSelectedNetwork('custom', 'https://node-x.testnet.botho.io/rpc')
    const loaded = loadSelectedNetwork()
    expect(loaded.networkId).toBe('custom')
    expect(loaded.customEndpoint).toBe('https://node-x.testnet.botho.io/rpc')
  })

  it('clears the custom endpoint when a built-in ingress is selected', () => {
    saveSelectedNetwork('custom', 'https://node-x.testnet.botho.io/rpc')
    saveSelectedIngress('seed2')
    const loaded = loadSelectedNetwork()
    // Selecting an ingress resets the network key and drops the custom endpoint.
    expect(loaded.networkId).toBe(DEFAULT_NETWORK_ID)
    expect(loaded.customEndpoint).toBeUndefined()
    expect(loadSelectedIngress()).toBe('seed2')
  })

  it('defaults to the built-in ingress when nothing is persisted', () => {
    expect(loadSelectedNetwork().networkId).toBe(DEFAULT_NETWORK_ID)
    expect(loadSelectedIngress()).toBe(DEFAULT_INGRESS_ID)
  })
})

describe('validateRpcEndpointForNetwork', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('rejects a plain-http non-loopback endpoint before any network call', async () => {
    const fetchSpy = vi.fn()
    vi.stubGlobal('fetch', fetchSpy)
    const result = await validateRpcEndpointForNetwork('http://node.example.com/rpc')
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toMatch(/https/i)
    // Shape check short-circuits — no fetch is made.
    expect(fetchSpy).not.toHaveBeenCalled()
  })

  it('rejects a non-URL string', async () => {
    const fetchSpy = vi.fn()
    vi.stubGlobal('fetch', fetchSpy)
    const result = await validateRpcEndpointForNetwork('not a url')
    expect(result.ok).toBe(false)
    expect(fetchSpy).not.toHaveBeenCalled()
  })

  it('rejects an https endpoint that is unreachable', async () => {
    stubStatus(null, false)
    const result = await validateRpcEndpointForNetwork('https://down.botho.io/rpc')
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toMatch(/connect/i)
  })

  it('rejects an https endpoint on a different network', async () => {
    stubStatus({ chainHeight: 5, synced: true, network: 'botho-mainnet' })
    const result = await validateRpcEndpointForNetwork('https://node-x.botho.io/rpc')
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain('botho-mainnet')
  })

  it('accepts an https endpoint on the expected network', async () => {
    stubStatus({ chainHeight: 5, synced: true, network: EXPECTED_NETWORK_ID })
    const result = await validateRpcEndpointForNetwork('https://node-x.testnet.botho.io/rpc')
    expect(result.ok).toBe(true)
  })

  it('exempts loopback hosts from the network-match check (dev workflow)', async () => {
    stubStatus({ chainHeight: 1, synced: false, network: 'botho-dev' })
    const result = await validateRpcEndpointForNetwork('http://localhost:17101/rpc')
    expect(result.ok).toBe(true)
  })

  it.each([undefined, null, '', ' ', 42, {}, ['botho-testnet'], ' botho-testnet '])(
    'rejects an https endpoint with invalid network identity %j', async (network) => {
      stubStatus({ chainHeight: 5, synced: true, network })
      const result = await validateRpcEndpointForNetwork('https://node-x.testnet.botho.io/rpc')
      expect(result).toEqual({ ok: false, error: 'This node did not report a valid network identity' })
    },
  )

  it('requires an identity even from loopback', async () => {
    stubStatus({ chainHeight: 5, synced: true })
    const result = await validateRpcEndpointForNetwork('http://localhost:17101/rpc')
    expect(result.ok).toBe(false)
  })

  it('configures custom endpoints with the same expected chain as validation', () => {
    expect(createCustomNetwork('https://node.example/rpc').networkId).toBe(EXPECTED_NETWORK_ID)
  })
})

describe('saved custom endpoint connection', () => {
  afterEach(() => {
    localStorage.clear()
    vi.unstubAllGlobals()
  })

  it.each([undefined, 'botho-mainnet', EXPECTED_NETWORK_ID])(
    'checks network %j again when restoring a previously accepted endpoint', async (network) => {
      const endpoint = 'https://node-x.testnet.botho.io/rpc'
      stubStatus({ chainHeight: 5, network: EXPECTED_NETWORK_ID })
      expect((await validateRpcEndpointForNetwork(endpoint)).ok).toBe(true)
      saveSelectedNetwork('custom', endpoint)

      const saved = loadSelectedNetwork()
      expect(saved.networkId).toBe('custom')
      const config = createCustomNetwork(saved.customEndpoint!)
      const adapter = new RemoteNodeAdapter({
        seedNodes: [config.rpcEndpoint], networkId: config.networkId, useWebSocket: false,
      })
      // The endpoint can change chains between being saved and being restored.
      stubStatus({ chainHeight: 5, network })
      if (network === EXPECTED_NETWORK_ID) {
        await adapter.connect()
        expect(adapter.getNodeInfo()?.networkId).toBe(EXPECTED_NETWORK_ID)
        adapter.disconnect()
      } else {
        await expect(adapter.connect()).rejects.toThrow('Failed to connect')
        expect(adapter.isConnected()).toBe(false)
        expect(adapter.getNodeInfo()).toBeNull()
      }
    },
  )
})

describe('fetchNodeHealth captures the reported network', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('surfaces the network field from node_getStatus', async () => {
    stubStatus({ chainHeight: 9, synced: true, network: 'botho-testnet' })
    const health = await fetchNodeHealth('https://node-x.testnet.botho.io/rpc')
    expect(health.status).toBe('online')
    expect(health.network).toBe('botho-testnet')
  })
})
