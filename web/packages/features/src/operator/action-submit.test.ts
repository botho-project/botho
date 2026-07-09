import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  isAppliedResult,
  isRefusedResult,
  parseOutcome,
  submitAction,
  submitToFleet,
  type FleetSubmitItem,
} from './action-submit'
import type { SignedActionEnvelope } from './action-envelope'
import type { FleetNode } from '../network/types'

const node = (id: string): FleetNode => ({
  id,
  name: `Node ${id}`,
  rpcEndpoint: `https://${id}.test/rpc`,
})

const signed: SignedActionEnvelope = {
  canonical: '{"v":1}',
  signature: 'ab'.repeat(32),
  envelopeHash: 'cd'.repeat(32),
  fields: {} as SignedActionEnvelope['fields'],
}

/** A node applied-outcome result body (matches the node's success shape). */
const appliedBody = {
  outcome: 'applied',
  dryRun: false,
  signerKeyId: 'c5e21ab1c9f6022d',
  action: 'quorum.pin_member',
  message: 'operator action applied',
  auditTag: 'applied',
  authenticated: true,
  resultingQuorum: { mode: 'recommended', members: ['P'], maxAutoMembers: 8 },
  gate: {
    intersectionRefused: false,
    curatedMembers: 1,
    autoMembers: 3,
    suppressedPeers: 0,
    maxAutoMembers: 8,
    faultTolerant: true,
    degenerate: false,
  },
}

/** A gate-refused outcome (returned as an RPC error whose data is the outcome). */
const refusedBody = {
  outcome: 'gate_refused',
  dryRun: false,
  signerKeyId: 'c5e21ab1c9f6022d',
  action: 'quorum.pin_member',
  message: 'gate refused',
  auditTag: 'gate_refused',
  authenticated: true,
  gate: {
    intersectionRefused: true,
    curatedMembers: 0,
    autoMembers: 0,
    suppressedPeers: 0,
    maxAutoMembers: 8,
    faultTolerant: false,
    degenerate: true,
  },
}

function mockFetchOnce(response: unknown, ok = true, status = 200) {
  return vi.fn().mockResolvedValue({
    ok,
    status,
    json: async () => response,
  })
}

afterEach(() => {
  vi.restoreAllMocks()
})

describe('parseOutcome (truthful outcome parsing, anti-#541)', () => {
  it('parses an applied outcome', () => {
    const o = parseOutcome(appliedBody)
    expect(o?.outcome).toBe('applied')
    expect(o?.authenticated).toBe(true)
    expect(o?.gate?.faultTolerant).toBe(true)
  })

  it('returns null for an unrecognized shape (never fabricates applied)', () => {
    expect(parseOutcome({ foo: 'bar' })).toBeNull()
    expect(parseOutcome(null)).toBeNull()
    expect(parseOutcome('applied')).toBeNull()
  })
})

describe('submitAction (single node)', () => {
  it('maps a success result to an applied outcome', async () => {
    vi.stubGlobal('fetch', mockFetchOnce({ result: appliedBody, id: 1 }))
    const r = await submitAction(node('a'), signed)
    expect(r.status).toBe('ok')
    expect(isAppliedResult(r)).toBe(true)
  })

  it('maps an RPC error whose data is the outcome to a refusal (NOT unreachable)', async () => {
    vi.stubGlobal(
      'fetch',
      mockFetchOnce({ error: { code: -32024, message: 'gate refused', data: refusedBody }, id: 1 }),
    )
    const r = await submitAction(node('a'), signed)
    expect(r.status).toBe('ok')
    expect(isRefusedResult(r)).toBe(true)
    expect(isAppliedResult(r)).toBe(false)
  })

  it('maps OPERATOR_NOT_ENABLED to not-enabled', async () => {
    vi.stubGlobal(
      'fetch',
      mockFetchOnce({ error: { code: -32020, message: 'not configured' }, id: 1 }),
    )
    const r = await submitAction(node('a'), signed)
    expect(r.status).toBe('not-enabled')
  })

  it('maps a transport failure to unreachable (NOT a refusal, NOT applied)', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('network down')))
    const r = await submitAction(node('a'), signed)
    expect(r.status).toBe('unreachable')
    expect(isAppliedResult(r)).toBe(false)
    expect(isRefusedResult(r)).toBe(false)
  })

  it('does NOT treat a dry-run applied outcome as a real apply', async () => {
    vi.stubGlobal('fetch', mockFetchOnce({ result: { ...appliedBody, dryRun: true }, id: 1 }))
    const r = await submitAction(node('a'), signed)
    expect(r.status).toBe('ok')
    expect(isAppliedResult(r)).toBe(false) // dryRun applied is a preview, not applied
  })

  it('sends EXACTLY {envelope, signature} — no sibling params (finding 1)', async () => {
    const fetchMock = mockFetchOnce({ result: appliedBody, id: 1 })
    vi.stubGlobal('fetch', fetchMock)
    await submitAction(node('a'), signed)
    const body = JSON.parse(fetchMock.mock.calls[0][1].body)
    expect(Object.keys(body.params).sort()).toEqual(['envelope', 'signature'])
    expect(body.params).not.toHaveProperty('dryRun')
    expect(body.method).toBe('operator_submitAction')
  })
})

describe('submitToFleet (partial failure is first-class, §7.3)', () => {
  it('classifies a mixed fleet: some applied, some refused, some unreachable', async () => {
    // Per-node fetch behavior keyed by endpoint.
    vi.stubGlobal(
      'fetch',
      vi.fn().mockImplementation((url: string) => {
        if (url.includes('a.test')) {
          return Promise.resolve({ ok: true, status: 200, json: async () => ({ result: appliedBody }) })
        }
        if (url.includes('b.test')) {
          return Promise.resolve({
            ok: true,
            status: 200,
            json: async () => ({ error: { code: -32024, data: refusedBody } }),
          })
        }
        return Promise.reject(new Error('down'))
      }),
    )
    const items: FleetSubmitItem[] = [
      { node: node('a'), signed },
      { node: node('b'), signed },
      { node: node('c'), signed },
    ]
    const outcome = await submitToFleet(items)
    expect(outcome.appliedNodeIds).toEqual(['a'])
    expect(outcome.refusedNodeIds).toEqual(['b'])
    expect(outcome.inconclusiveNodeIds).toEqual(['c'])
    expect(outcome.partial).toBe(true)
    expect(outcome.allApplied).toBe(false)
  })

  it('allApplied is true only when every node applied', async () => {
    vi.stubGlobal('fetch', mockFetchOnce({ result: appliedBody }))
    const outcome = await submitToFleet([
      { node: node('a'), signed },
      { node: node('b'), signed },
    ])
    expect(outcome.allApplied).toBe(true)
    expect(outcome.partial).toBe(false)
  })
})
