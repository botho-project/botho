import { afterEach, describe, expect, it, vi } from 'vitest'
import { fetchAuditLog } from './audit'
import type { FleetNode } from '../network/types'

const node: FleetNode = { id: 'a', name: 'Node A', rpcEndpoint: 'https://a.test/rpc' }

const storedEntry = {
  ts: 1_800_000_000,
  signerKeyId: 'c5e21ab1c9f6022d',
  envelopeHash: 'deadbeef'.repeat(8),
  action: 'quorum.pin_member',
  params: { peerId: '12D3KooWfake' },
  dryRun: false,
  outcome: 'applied',
  prevQuorum: { mode: 'recommended', members: [], maxAutoMembers: 8 },
  newQuorum: { mode: 'recommended', members: ['12D3KooWfake'], maxAutoMembers: 8 },
  gate: { intersectionRefused: false },
}

function mockFetch(response: unknown, ok = true, status = 200) {
  return vi.fn().mockResolvedValue({ ok, status, json: async () => response })
}

afterEach(() => vi.restoreAllMocks())

describe('fetchAuditLog (renders stored entries only, anti-#541)', () => {
  it('returns the node stored entries verbatim', async () => {
    vi.stubGlobal('fetch', mockFetch({ result: { entries: [storedEntry], count: 1 } }))
    const r = await fetchAuditLog(node, 'tok')
    expect(r.status).toBe('ok')
    if (r.status === 'ok') {
      expect(r.data).toHaveLength(1)
      expect(r.data[0].outcome).toBe('applied')
      expect(r.data[0].newQuorum).toEqual(storedEntry.newQuorum)
    }
  })

  it('returns an empty list when the node has no entries (not a fabricated one)', async () => {
    vi.stubGlobal('fetch', mockFetch({ result: { entries: [], count: 0 } }))
    const r = await fetchAuditLog(node, 'tok')
    expect(r.status).toBe('ok')
    if (r.status === 'ok') expect(r.data).toEqual([])
  })

  it('maps not-enabled / unauthorized / unreachable distinctly', async () => {
    vi.stubGlobal('fetch', mockFetch({ error: { code: -32020 } }))
    expect((await fetchAuditLog(node, 'tok')).status).toBe('not-enabled')

    vi.stubGlobal('fetch', mockFetch({ error: { code: -32021 } }))
    expect((await fetchAuditLog(node, 'bad')).status).toBe('unauthorized')

    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('down')))
    expect((await fetchAuditLog(node, 'tok')).status).toBe('unreachable')
  })

  it('drops malformed entries rather than surfacing garbage', async () => {
    vi.stubGlobal(
      'fetch',
      mockFetch({ result: { entries: [storedEntry, { nonsense: true }, null] } }),
    )
    const r = await fetchAuditLog(node, 'tok')
    expect(r.status).toBe('ok')
    if (r.status === 'ok') expect(r.data).toHaveLength(1)
  })
})
