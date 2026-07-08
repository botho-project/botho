import { describe, expect, it, vi, afterEach } from 'vitest'
import { deriveTrustSummary, fetchNodeTrustStatus, fetchOperatorQuorumInfo } from './quorum'
import type { NodeTrustStatus } from './types'

const node = { id: 'seed', name: 'Seed (validator)', rpcEndpoint: 'https://seed.test/rpc' }

/**
 * Wire fixtures captured from the live testnet (seed.botho.io, 2026-07-07):
 * the exact camelCase field names `handle_node_status` and
 * `handle_get_peers` serialize (`botho/src/rpc/mod.rs`).
 */
const STATUS_FIXTURE = {
  chainHeight: 716,
  mempoolSize: 0,
  nodeVersion: '0.3.2',
  peerCount: 3,
  quorumAutoMembers: 3,
  quorumCuratedMembers: 0,
  quorumDegenerate: false,
  quorumFaultTolerant: true,
  quorumGateIntersectionRefused: false,
  quorumGateMaxAutoMembers: 8,
  quorumGateSuppressedPeers: 0,
  scpPeerCount: 3,
  synced: true,
}

const PEERS_FIXTURE = {
  peerCount: 3,
  peers: [
    {
      address: null,
      lastSeenSecs: 44,
      peerId: '12D3KooWJ5U2gk6Pe9ehZb6aHng2zu7RnUwAKzEYxHbaM6VRo592',
      protocolVersion: '4.0.0 (block v5)',
      versionWarning: false,
    },
    {
      address: null,
      lastSeenSecs: 0,
      peerId: '12D3KooWRubuvzRNxbxHH5BdzgxQNqMoWyQtdxKXUdNWJt5huTpk',
      protocolVersion: null,
      versionWarning: false,
    },
    {
      address: '/ip4/10.0.0.1/tcp/4001',
      lastSeenSecs: 1,
      peerId: '12D3KooWBDKQDQQkK5rLmSnke2hiWVaZ9qNGuuBhxFdxyqxaTvwK',
      protocolVersion: '4.0.0 (block v5)',
      versionWarning: true,
    },
  ],
}

/** Stub fetch with per-RPC-method responses. `Error` values throw. */
function stubRpc(handlers: Record<string, unknown>) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (_url: unknown, init?: { body?: string }) => {
      const method = (JSON.parse(init?.body ?? '{}') as { method?: string }).method ?? ''
      const h = handlers[method]
      if (h instanceof Error) throw h
      return { ok: true, json: async () => h }
    }),
  )
}

afterEach(() => vi.unstubAllGlobals())

describe('fetchNodeTrustStatus', () => {
  it('merges node_getStatus gate fields with the network_getPeers table', async () => {
    stubRpc({
      node_getStatus: { result: STATUS_FIXTURE },
      network_getPeers: { result: PEERS_FIXTURE },
    })
    const s = await fetchNodeTrustStatus(node)
    expect(s).toMatchObject({
      nodeId: 'seed',
      reachable: true,
      quorumFaultTolerant: true,
      quorumDegenerate: false,
      quorumCuratedMembers: 0,
      quorumAutoMembers: 3,
      quorumGateSuppressedPeers: 0,
      quorumGateMaxAutoMembers: 8,
      quorumGateIntersectionRefused: false,
      scpPeerCount: 3,
    })
    expect(s.peers).toHaveLength(3)
    expect(s.peers?.[0]).toEqual({
      peerId: '12D3KooWJ5U2gk6Pe9ehZb6aHng2zu7RnUwAKzEYxHbaM6VRo592',
      address: null,
      protocolVersion: '4.0.0 (block v5)',
      versionWarning: false,
      lastSeenSecs: 44,
    })
    // Null protocolVersion stays null (renders "—"), never a fabricated string.
    expect(s.peers?.[1].protocolVersion).toBeNull()
    expect(s.peers?.[2].versionWarning).toBe(true)
    expect(s.peers?.[2].address).toBe('/ip4/10.0.0.1/tcp/4001')
  })

  it('maps null gate fields (no gate evaluation yet) to absent — never zero', async () => {
    stubRpc({
      node_getStatus: {
        result: {
          ...STATUS_FIXTURE,
          quorumCuratedMembers: null,
          quorumAutoMembers: null,
          quorumGateSuppressedPeers: null,
          quorumGateMaxAutoMembers: null,
          quorumGateIntersectionRefused: null,
        },
      },
      network_getPeers: { result: { peers: [], peerCount: 0 } },
    })
    const s = await fetchNodeTrustStatus(node)
    expect(s.reachable).toBe(true)
    expect(s.quorumCuratedMembers).toBeUndefined()
    expect(s.quorumAutoMembers).toBeUndefined()
    expect(s.quorumGateSuppressedPeers).toBeUndefined()
    expect(s.quorumGateMaxAutoMembers).toBeUndefined()
    expect(s.quorumGateIntersectionRefused).toBeUndefined()
    // Genuinely-empty peer list is [], distinct from an unavailable one.
    expect(s.peers).toEqual([])
  })

  it('resolves unreachable when node_getStatus throws — never throws into the poller', async () => {
    stubRpc({
      node_getStatus: new Error('boom'),
      network_getPeers: { result: PEERS_FIXTURE },
    })
    const s = await fetchNodeTrustStatus(node)
    expect(s.reachable).toBe(false)
    expect(s.quorumCuratedMembers).toBeUndefined()
    expect(s.peers).toBeUndefined()
  })

  it('resolves unreachable on an RPC-level error object', async () => {
    stubRpc({
      node_getStatus: { error: { code: -32000 } },
      network_getPeers: { result: PEERS_FIXTURE },
    })
    expect((await fetchNodeTrustStatus(node)).reachable).toBe(false)
  })

  it('keeps the node reachable but marks peers unavailable when only network_getPeers fails', async () => {
    stubRpc({
      node_getStatus: { result: STATUS_FIXTURE },
      network_getPeers: new Error('boom'),
    })
    const s = await fetchNodeTrustStatus(node)
    expect(s.reachable).toBe(true)
    expect(s.quorumAutoMembers).toBe(3)
    // undefined = "call failed", NOT an empty list masquerading as no peers.
    expect(s.peers).toBeUndefined()
  })

  it('marks peers unavailable on an unrecognized peers shape', async () => {
    stubRpc({
      node_getStatus: { result: STATUS_FIXTURE },
      network_getPeers: { result: { peers: 'nope' } },
    })
    expect((await fetchNodeTrustStatus(node)).peers).toBeUndefined()
  })
})

describe('deriveTrustSummary', () => {
  function status(overrides: Partial<NodeTrustStatus>): NodeTrustStatus {
    return { nodeId: 'n', reachable: true, polledAt: 0, ...overrides }
  }

  it('collects refused/degenerate node ids and counts posture', () => {
    const s = deriveTrustSummary([
      status({ nodeId: 'a', quorumFaultTolerant: true }),
      status({ nodeId: 'b', quorumGateIntersectionRefused: true, quorumFaultTolerant: true }),
      status({ nodeId: 'c', quorumDegenerate: true, quorumFaultTolerant: false }),
      status({ nodeId: 'd', reachable: false }),
    ])
    expect(s.nodesReachable).toBe(3)
    expect(s.nodesTotal).toBe(4)
    expect(s.intersectionRefusedNodeIds).toEqual(['b'])
    expect(s.degenerateNodeIds).toEqual(['c'])
    expect(s.faultTolerantCount).toBe(2)
  })

  it('never counts an unreachable node as warning OR healthy', () => {
    const s = deriveTrustSummary([
      status({
        nodeId: 'dead',
        reachable: false,
        quorumGateIntersectionRefused: true,
        quorumDegenerate: true,
        quorumFaultTolerant: true,
      }),
    ])
    expect(s.intersectionRefusedNodeIds).toEqual([])
    expect(s.degenerateNodeIds).toEqual([])
    expect(s.faultTolerantCount).toBe(0)
  })

  it('treats absent gate fields as no signal', () => {
    const s = deriveTrustSummary([status({ nodeId: 'relay' })])
    expect(s.intersectionRefusedNodeIds).toEqual([])
    expect(s.degenerateNodeIds).toEqual([])
    expect(s.faultTolerantCount).toBe(0)
  })
})

describe('fetchOperatorQuorumInfo (#707)', () => {
  const QUORUM_OK = {
    quorum: {
      mode: 'recommended',
      faultModel: 'crash',
      threshold: 2,
      members: ['12D3KooWCurated'],
      minPeers: 1,
      maxAutoMembers: 8,
    },
    perPeer: {
      curated: ['12D3KooWCurated'],
      auto: ['12D3KooWAuto'],
      suppressed: ['12D3KooWSuppressed'],
    },
  }

  it('parses a successful response including per-peer classification', async () => {
    stubRpc({ operator_getQuorumInfo: { result: QUORUM_OK } })
    const r = await fetchOperatorQuorumInfo(node, 'op.9999999999.sig')
    expect(r.status).toBe('ok')
    if (r.status === 'ok') {
      expect(r.data.mode).toBe('recommended')
      expect(r.data.members).toEqual(['12D3KooWCurated'])
      expect(r.data.perPeer).toEqual({
        curated: ['12D3KooWCurated'],
        auto: ['12D3KooWAuto'],
        suppressed: ['12D3KooWSuppressed'],
      })
    }
  })

  it('maps perPeer:null (no gate evaluation yet) to undefined — never fabricated', async () => {
    stubRpc({
      operator_getQuorumInfo: { result: { ...QUORUM_OK, perPeer: null } },
    })
    const r = await fetchOperatorQuorumInfo(node, 'tok')
    expect(r.status).toBe('ok')
    if (r.status === 'ok') expect(r.data.perPeer).toBeUndefined()
  })

  it('reports not-enabled on the -32020 error code', async () => {
    stubRpc({ operator_getQuorumInfo: { error: { code: -32020 } } })
    expect((await fetchOperatorQuorumInfo(node, 'tok')).status).toBe('not-enabled')
  })

  it('reports unauthorized on the -32021 error code (missing/expired/tampered)', async () => {
    stubRpc({ operator_getQuorumInfo: { error: { code: -32021 } } })
    expect((await fetchOperatorQuorumInfo(node, 'tok')).status).toBe('unauthorized')
    // A null token still calls the node and surfaces its rejection.
    expect((await fetchOperatorQuorumInfo(node, null)).status).toBe('unauthorized')
  })

  it('reports unreachable when the call throws — never throws into the poller', async () => {
    stubRpc({ operator_getQuorumInfo: new Error('boom') })
    expect((await fetchOperatorQuorumInfo(node, 'tok')).status).toBe('unreachable')
  })
})
