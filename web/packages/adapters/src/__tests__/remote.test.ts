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

describe('RemoteNodeAdapter.getAllClusterWealth', () => {
  it('parses the captured live fixture: string wealth -> bigint, missing factor -> 1000 floor', async () => {
    // The fixture predates #700 (no per-cluster factor) — the adapter must
    // default to the 1000 floor for old nodes, never re-derive the curve.
    const adapter = await connectedAdapter({ cluster_getAllWealth: clusterGetAllWealth })
    const clusters = await adapter.getAllClusterWealth()
    expect(clusters).toHaveLength(3)
    expect(clusters[0]).toEqual({
      clusterId: '7244222622098737154',
      wealth: 50_000_000_000_000n,
      factor: 1000,
    })
    expect(typeof clusters[0].wealth).toBe('bigint')
    // cluster_id stays a string — the real captured ids exceed 2^53.
    expect(typeof clusters[0].clusterId).toBe('string')
  })

  it('parses enriched #700 responses: u128 wealth exactly + node-supplied factor', async () => {
    const response = {
      jsonrpc: '2.0',
      result: {
        count: 2,
        total_tracked_wealth: '340282366920938463463374607431768211455',
        clusters: [
          { cluster_id: '1', wealth: '1', factor: 1000 },
          { cluster_id: '2', wealth: '340282366920938463463374607431768211454', factor: 6000 },
        ],
      },
      id: 1,
    }
    const adapter = await connectedAdapter({ cluster_getAllWealth: response })
    const clusters = await adapter.getAllClusterWealth()
    expect(clusters[0]).toEqual({ clusterId: '1', wealth: 1n, factor: 1000 })
    expect(clusters[1].wealth).toBe(340282366920938463463374607431768211454n)
    expect(clusters[1].wealth.toString()).toBe('340282366920938463463374607431768211454')
    expect(clusters[1].factor).toBe(6000)
  })

  it('returns [] when the node sends no clusters array', async () => {
    const response = {
      jsonrpc: '2.0',
      result: { count: 0, total_tracked_wealth: '0' },
      id: 1,
    }
    const adapter = await connectedAdapter({ cluster_getAllWealth: response })
    expect(await adapter.getAllClusterWealth()).toEqual([])
  })
})

describe('RemoteNodeAdapter.getBlock enriched fields (#700)', () => {
  /** Base pre-#700 block shape, byte-for-byte what old nodes serve. */
  const legacyBlock = {
    height: 42,
    hash: 'f'.repeat(64),
    prevHash: 'e'.repeat(64),
    timestamp: 1751840000,
    difficulty: 12345,
    nonce: 987,
    txCount: 1,
    mintingReward: 4800000000000,
  }

  function rpc(result: Record<string, unknown>) {
    return { jsonrpc: '2.0', result, id: 1 }
  }

  it('maps the enriched #700 shape: per-tx summaries, totalFees, lottery — all bigint amounts', async () => {
    const adapter = await connectedAdapter({
      getBlockByHeight: rpc({
        ...legacyBlock,
        transactions: [{ hash: 'c0de'.repeat(16), fee: 250, ringSize: 20 }],
        totalFees: 250,
        lottery: {
          totalFees: 250,
          poolDistributed: 200,
          amountBurned: 50,
          lotterySeed: '09'.repeat(32),
          payoutCount: 2,
          payoutTotal: 100,
        },
      }),
    })
    const block = await adapter.getBlock(42)
    expect(block).not.toBeNull()
    expect(block!.transactions).toEqual([
      { hash: 'c0de'.repeat(16), fee: 250n, ringSize: 20 },
    ])
    expect(block!.totalFees).toBe(250n)
    expect(block!.lottery).toEqual({
      totalFees: 250n,
      poolDistributed: 200n,
      amountBurned: 50n,
      lotterySeed: '09'.repeat(32),
      payoutCount: 2,
      payoutTotal: 100n,
    })
    // Pre-existing fields are unchanged by the enrichment.
    expect(block!.height).toBe(42)
    expect(block!.reward).toBe(4800000000000n)
    expect(block!.previousHash).toBe('e'.repeat(64))
  })

  it('leaves enriched fields undefined for old nodes (additive contract — no break)', async () => {
    const adapter = await connectedAdapter({ getBlockByHeight: rpc(legacyBlock) })
    const block = await adapter.getBlock(42)
    expect(block).not.toBeNull()
    expect(block!.transactions).toBeUndefined()
    expect(block!.totalFees).toBeUndefined()
    expect(block!.lottery).toBeUndefined()
    expect(block!.transactionCount).toBe(1)
  })

  it('maps the same enrichment on the getBlockByHash path', async () => {
    const adapter = await connectedAdapter({
      getBlockByHash: rpc({ ...legacyBlock, totalFees: 99, transactions: [] }),
    })
    const block = await adapter.getBlock('f'.repeat(64))
    expect(block!.totalFees).toBe(99n)
    expect(block!.transactions).toEqual([])
    expect(lastCall('getBlockByHash')?.params.hash).toBe('f'.repeat(64))
  })
})

describe('RemoteNodeAdapter.getNetworkStats', () => {
  function rpc(result: Record<string, unknown>) {
    return { jsonrpc: '2.0', result, id: 1 }
  }

  it('reports hashRate as null (not a fabricated "0") — the read RPC has no hash rate (#913)', async () => {
    const adapter = await connectedAdapter({
      getChainInfo: rpc({ difficulty: 12345 }),
    })
    const stats = await adapter.getNetworkStats()
    expect(stats.hashRate).toBeNull()
    // The public fields the RPC does expose are still populated.
    expect(typeof stats.blockHeight).toBe('number')
    expect(stats.difficulty).toBe(12345n)
  })
})

describe('RemoteNodeAdapter.getTransaction', () => {
  function rpc(result: Record<string, unknown>) {
    return { jsonrpc: '2.0', result, id: 1 }
  }

  const txHash = 'ab'.repeat(32)

  it('does not fabricate amount or type — the node exposes neither (#913, D1)', async () => {
    const adapter = await connectedAdapter({
      getTransaction: rpc({
        txHash,
        status: 'confirmed',
        blockHeight: null,
        confirmations: 3,
        inMempool: false,
        fee: 250,
      }),
    })
    const tx = await adapter.getTransaction(txHash)
    expect(tx).not.toBeNull()
    // Neither amount nor direction is asserted.
    expect(tx!.amount).toBeUndefined()
    expect(tx!.type).toBeUndefined()
    // The public fee is preserved as a bigint.
    expect(tx!.fee).toBe(250n)
  })

  it('leaves timestamp undefined (not Date.now()) when the tx is not in a block (#913)', async () => {
    const adapter = await connectedAdapter({
      getTransaction: rpc({
        txHash,
        status: 'pending',
        blockHeight: null,
        confirmations: 0,
        inMempool: true,
        fee: 100,
      }),
    })
    const tx = await adapter.getTransaction(txHash)
    expect(tx!.timestamp).toBeUndefined()
  })

  it('resolves the real block timestamp when the tx is in a block (#913)', async () => {
    const adapter = await connectedAdapter({
      getTransaction: rpc({
        txHash,
        status: 'confirmed',
        blockHeight: 42,
        confirmations: 5,
        inMempool: false,
        fee: 250,
      }),
      getBlockByHeight: rpc({
        height: 42,
        hash: 'f'.repeat(64),
        prevHash: 'e'.repeat(64),
        timestamp: 1751840000,
        difficulty: 1,
        txCount: 1,
        mintingReward: 0,
      }),
    })
    const tx = await adapter.getTransaction(txHash)
    expect(tx!.timestamp).toBe(1751840000)
    expect(tx!.blockHeight).toBe(42)
  })

  it('leaves timestamp undefined when the block lookup fails (no fabrication)', async () => {
    const adapter = await connectedAdapter({
      getTransaction: rpc({
        txHash,
        status: 'confirmed',
        blockHeight: 42,
        confirmations: 5,
        inMempool: false,
        fee: 250,
      }),
      // getBlockByHeight is intentionally unrouted -> RPC error -> getBlock returns null.
    })
    const tx = await adapter.getTransaction(txHash)
    expect(tx!.timestamp).toBeUndefined()
    expect(tx!.blockHeight).toBe(42)
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
