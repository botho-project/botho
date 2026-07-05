import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { RemoteNodeAdapter } from '../remote'

// ---------------------------------------------------------------------------
// Fixtures
//
// These JSON files are the ACTUAL JSON-RPC responses captured live from the EU
// seed node 3.77.150.19:17101 (protocol 4.0.0, node commit 4f944e0) on
// 2026-07-05 — the same node the curator probed for issue #634. They pin the
// wire format the adapter must handle: string-encoded u128 cluster wealth
// (#628) and JSON-number smooth fee factors (#626). `cluster-getAllWealth.json`
// is truncated to three representative entries (the live response had 202) to
// keep the fixture small; the retained cluster_id values are real and exceed the
// JS safe-integer range.
// ---------------------------------------------------------------------------

const fixturesDir = join(dirname(fileURLToPath(import.meta.url)), 'fixtures')

function loadFixture<T = { jsonrpc: string; result: Record<string, unknown>; id: number }>(
  name: string,
): T {
  return JSON.parse(readFileSync(join(fixturesDir, `${name}.json`), 'utf-8')) as T
}

const nodeStatus = loadFixture('node-getStatus')
const estimateFeeZero = loadFixture('estimateFee-zero-wealth')
const estimateFeeNonzero = loadFixture('estimateFee-nonzero-wealth')
const clusterByTargetKeys = loadFixture('cluster-getWealthByTargetKeys')
const clusterGetAllWealth = loadFixture('cluster-getAllWealth')

const ZERO_KEY = '0000000000000000000000000000000000000000000000000000000000000000'

/** A single captured JSON-RPC request, for asserting what the adapter sent. */
interface CapturedCall {
  method: string
  params: Record<string, unknown>
}

let captured: CapturedCall[] = []

/**
 * Install a `fetch` stub that routes JSON-RPC requests by method to the given
 * fixture responses, and records every request for later assertions. Unknown
 * methods return a JSON-RPC "method not found" error.
 */
function installFetch(routes: Record<string, unknown>): void {
  const mock = vi.fn(async (_url: string, init: RequestInit) => {
    const req = JSON.parse(init.body as string) as { method: string; params: Record<string, unknown>; id: number }
    captured.push({ method: req.method, params: req.params })
    const route = routes[req.method]
    const body =
      route === undefined
        ? { jsonrpc: '2.0', error: { code: -32601, message: 'Method not found' }, id: req.id }
        : route
    return { ok: true, json: async () => body } as unknown as Response
  })
  vi.stubGlobal('fetch', mock)
}

/** Connect a RemoteNodeAdapter against the given per-method fixture routes. */
async function connectedAdapter(routes: Record<string, unknown>): Promise<RemoteNodeAdapter> {
  installFetch({ node_getStatus: nodeStatus, ...routes })
  const adapter = new RemoteNodeAdapter({
    seedNodes: ['https://seed.test/rpc'],
    networkId: 'botho-testnet',
    useWebSocket: false,
  })
  await adapter.connect()
  return adapter
}

function lastCall(method: string): CapturedCall | undefined {
  return captured.filter((c) => c.method === method).at(-1)
}

beforeEach(() => {
  captured = []
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('RemoteNodeAdapter.estimateFee', () => {
  it('returns a bigint fee even though the node sends fees as JSON numbers', async () => {
    const adapter = await connectedAdapter({ estimateFee: estimateFeeZero })
    const { fee } = await adapter.estimateFee(0)
    expect(typeof fee).toBe('bigint')
    // recommendedFee in the live fixture is the JSON number 16000.
    expect(fee).toBe(16000n)
    expect(fee).toBe(BigInt(estimateFeeZero.result.recommendedFee as number))
  })

  it('returns the node-computed clusterFactorDisplay (base rate)', async () => {
    const adapter = await connectedAdapter({ estimateFee: estimateFeeZero })
    const { clusterFactorDisplay } = await adapter.estimateFee(0)
    expect(clusterFactorDisplay).toBe('1.00x')
  })

  it('omits cluster_wealth from the request when none is supplied', async () => {
    const adapter = await connectedAdapter({ estimateFee: estimateFeeZero })
    await adapter.estimateFee(0)
    const call = lastCall('estimateFee')
    expect(call).toBeDefined()
    expect(call?.params.cluster_wealth).toBeUndefined()
  })

  it('forwards clusterWealth as a decimal STRING and returns the higher fee + factor display', async () => {
    const adapter = await connectedAdapter({ estimateFee: estimateFeeNonzero })
    // 1000 BTH = 1e15 picocredits; live node returns the 1.26x factor for this.
    const { fee, clusterFactorDisplay } = await adapter.estimateFee(0, 1_000_000_000_000_000n)
    expect(fee).toBe(20240n)
    expect(clusterFactorDisplay).toBe('1.26x')
    const call = lastCall('estimateFee')
    expect(call?.params.cluster_wealth).toBe('1000000000000000')
    expect(typeof call?.params.cluster_wealth).toBe('string')
  })

  it('forwards a u128-range clusterWealth as a string without precision loss', async () => {
    const adapter = await connectedAdapter({ estimateFee: estimateFeeZero })
    // Exceeds Number.MAX_SAFE_INTEGER and u64 max — must survive as a string.
    await adapter.estimateFee(0, 99_999_999_999_999_999_999n)
    const call = lastCall('estimateFee')
    expect(call?.params.cluster_wealth).toBe('99999999999999999999')
  })

  it('never coerces the response clusterWealth string through Number()', async () => {
    // The estimateFee response carries clusterWealth as a JSON string ("0").
    // The adapter must return a fee derived solely from recommendedFee via
    // BigInt — the clusterWealth string must not be parsed at all here.
    expect(typeof estimateFeeZero.result.clusterWealth).toBe('string')
    const adapter = await connectedAdapter({ estimateFee: estimateFeeZero })
    const { fee } = await adapter.estimateFee(0)
    expect(fee).toBe(BigInt(String(estimateFeeZero.result.recommendedFee)))
  })
})

describe('RemoteNodeAdapter.getClusterWealth', () => {
  it('returns 0n and makes no network call for an empty target-key list', async () => {
    const adapter = await connectedAdapter({})
    const wealth = await adapter.getClusterWealth([])
    expect(wealth).toBe(0n)
    expect(lastCall('cluster_getWealthByTargetKeys')).toBeUndefined()
  })

  it('parses max_cluster_wealth via BigInt and forwards target_keys', async () => {
    const adapter = await connectedAdapter({ cluster_getWealthByTargetKeys: clusterByTargetKeys })
    const wealth = await adapter.getClusterWealth([ZERO_KEY])
    expect(typeof wealth).toBe('bigint')
    expect(wealth).toBe(0n) // live fixture: max_cluster_wealth "0"
    const call = lastCall('cluster_getWealthByTargetKeys')
    expect(call?.params.target_keys).toEqual([ZERO_KEY])
  })

  it('handles a u128-range max_cluster_wealth (> u64 max) without precision loss', async () => {
    const big = '99999999999999999999' // > 2^64, > 2^53
    const response = {
      jsonrpc: '2.0',
      result: { ...clusterByTargetKeys.result, max_cluster_wealth: big, total_value: 5 },
      id: 1,
    }
    const adapter = await connectedAdapter({ cluster_getWealthByTargetKeys: response })
    const wealth = await adapter.getClusterWealth([ZERO_KEY])
    expect(wealth).toBe(99_999_999_999_999_999_999n)
    expect(wealth.toString()).toBe(big)
  })
})

describe('u128 / u64 wire-format round-trips', () => {
  it('round-trips BigInt("99999999999999999999") (> u64 max) exactly', () => {
    const big = '99999999999999999999'
    expect(BigInt(big).toString()).toBe(big)
    // Number() would silently corrupt it — this is exactly what the adapter avoids.
    expect(Number(big).toString()).not.toBe(big)
  })

  it('round-trips boundary and u128-max values exactly', () => {
    // JS safe-integer boundary.
    expect(BigInt('9007199254740991').toString()).toBe('9007199254740991')
    // u64 max + 1.
    expect(BigInt('18446744073709551616').toString()).toBe('18446744073709551616')
    // u128 max.
    const u128Max = '340282366920938463463374607431768211455'
    expect(BigInt(u128Max).toString()).toBe(u128Max)
  })

  it('cluster_getAllWealth cluster_id exceeds JS safe-integer range (never parseInt)', () => {
    const first = (clusterGetAllWealth.result.clusters as Array<{ cluster_id: string }>)[0]
    const id = first.cluster_id
    // Real captured value: a u64 well beyond Number.MAX_SAFE_INTEGER.
    expect(BigInt(id) > BigInt(Number.MAX_SAFE_INTEGER)).toBe(true)
    expect(BigInt(id).toString()).toBe(id)
    // parseInt / Number would corrupt the low digits.
    expect(String(parseInt(id, 10))).not.toBe(id)
  })
})
