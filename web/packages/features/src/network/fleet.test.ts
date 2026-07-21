import { describe, expect, it, vi, afterEach } from 'vitest'
import { averageBlockSeconds, deriveFleetSummary, fetchFleetNodeStatus } from './fleet'
import type { FleetNodeStatus } from './types'

function status(overrides: Partial<FleetNodeStatus>): FleetNodeStatus {
  return { nodeId: 'n', reachable: true, polledAt: 0, ...overrides }
}

describe('deriveFleetSummary', () => {
  it('takes the max reachable height as consensus and counts in-sync within 1 block', () => {
    const s = deriveFleetSummary([
      status({ nodeId: 'a', chainHeight: 220, mempoolSize: 2 }),
      status({ nodeId: 'b', chainHeight: 219, mempoolSize: 1 }),
      status({ nodeId: 'c', chainHeight: 210 }), // lagging
      status({ nodeId: 'd', reachable: false }),
    ])
    expect(s.consensusHeight).toBe(220)
    expect(s.nodesInSync).toBe(2)
    expect(s.nodesReachable).toBe(3)
    expect(s.nodesTotal).toBe(4)
    expect(s.totalMempool).toBe(3)
  })

  it('excludes peer-isolated nodes from consensus so a stale fork cannot poison the tip', () => {
    // The live eu/ap relay drift: two isolated relays stuck on the old
    // pre-reset chain at height 3233, three connected validators at 202.
    const s = deriveFleetSummary([
      status({ nodeId: 'seed', chainHeight: 202, peerCount: 2 }),
      status({ nodeId: 'seed2', chainHeight: 202, peerCount: 2 }),
      status({ nodeId: 'faucet', chainHeight: 202, peerCount: 2 }),
      status({ nodeId: 'eu', chainHeight: 3233, peerCount: 0 }),
      status({ nodeId: 'ap', chainHeight: 3233, peerCount: 0 }),
    ])
    expect(s.consensusHeight).toBe(202) // NOT 3233
    expect(s.nodesInSync).toBe(3) // the validators, not the isolated relays
    expect(s.nodesReachable).toBe(5)
    expect(s.nodesIsolated).toBe(2)
  })

  it('falls back to reachable heights when no node has peers (lone dev node)', () => {
    const s = deriveFleetSummary([status({ chainHeight: 10, peerCount: 0 })])
    expect(s.consensusHeight).toBe(10)
    expect(s.nodesIsolated).toBe(1)
  })

  it('treats an undefined peerCount as participating (back-compat with older nodes)', () => {
    const s = deriveFleetSummary([
      status({ chainHeight: 220 }), // no peerCount field
      status({ chainHeight: 219 }),
    ])
    expect(s.consensusHeight).toBe(220)
    expect(s.nodesInSync).toBe(2)
    expect(s.nodesIsolated).toBe(0)
  })

  it('reports null consensus height when nothing is reachable', () => {
    const s = deriveFleetSummary([status({ reachable: false }), status({ reachable: false })])
    expect(s.consensusHeight).toBeNull()
    expect(s.nodesInSync).toBe(0)
  })

  it('flags a stalled SCP slot anywhere in the fleet', () => {
    expect(
      deriveFleetSummary([status({ chainHeight: 5 }), status({ chainHeight: 5, slotStalled: true })])
        .anySlotStalled,
    ).toBe(true)
  })

  it('never counts an unreachable node toward mempool or stall', () => {
    const s = deriveFleetSummary([
      status({ reachable: false, mempoolSize: 99, slotStalled: true }),
      status({ chainHeight: 1 }),
    ])
    expect(s.totalMempool).toBe(0)
    expect(s.anySlotStalled).toBe(false)
  })
})

describe('averageBlockSeconds', () => {
  it('computes seconds per block over the window', () => {
    expect(averageBlockSeconds({ height: 200, timestamp: 1000 }, { height: 220, timestamp: 1400 })).toBe(20)
  })

  it('returns null for degenerate windows instead of fabricating a rate', () => {
    expect(averageBlockSeconds({ height: 220, timestamp: 1000 }, { height: 220, timestamp: 1400 })).toBeNull()
    expect(averageBlockSeconds({ height: 200, timestamp: 1400 }, { height: 220, timestamp: 1000 })).toBeNull()
  })
})

describe('fetchFleetNodeStatus', () => {
  afterEach(() => vi.unstubAllGlobals())

  const node = { id: 'seed', name: 'Seed', rpcEndpoint: 'https://seed.test/rpc' }

  it('maps a node_getStatus result to a reachable snapshot', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: true,
        json: async () => ({
          result: {
            chainHeight: 221,
            peerCount: 4,
            scpPeerCount: 4,
            mempoolSize: 0,
            mintingActive: false,
            nodeVersion: '0.3.1',
            slotStalled: false,
          },
        }),
      })),
    )
    const s = await fetchFleetNodeStatus(node)
    expect(s).toMatchObject({
      nodeId: 'seed',
      reachable: true,
      chainHeight: 221,
      peerCount: 4,
      nodeVersion: '0.3.1',
      slotStalled: false,
    })
  })

  it('resolves unreachable on fetch failure — never throws into the poller', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => { throw new Error('boom') }))
    const s = await fetchFleetNodeStatus(node)
    expect(s.reachable).toBe(false)
    expect(s.chainHeight).toBeUndefined()
  })

  it('resolves unreachable on an RPC-level error object', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true, json: async () => ({ error: { code: -32000 } }) })),
    )
    expect((await fetchFleetNodeStatus(node)).reachable).toBe(false)
  })
})
