/**
 * @vitest-environment jsdom
 *
 * The polling/history hooks extracted from the `/network` page (#706) so
 * `/network` and `/operator` share one implementation. Contract under test:
 * the first poll populates per-node snapshots (failures as explicit
 * `reachable: false`), and a missing metrics backend degrades history to
 * `unavailable` without throwing.
 */
import { describe, expect, it, vi, afterEach } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import { useFleetHistory, useFleetStatus } from './hooks'
import type { FleetNode } from './types'

const NODES: FleetNode[] = [
  { id: 'seed', name: 'Seed', rpcEndpoint: 'https://seed.test/rpc' },
  { id: 'eu', name: 'EU', rpcEndpoint: 'https://eu.test/rpc' },
]

afterEach(() => vi.unstubAllGlobals())

describe('useFleetStatus', () => {
  it('populates statuses from the first poll and derives the consensus height', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: unknown, init?: { body?: string }) => {
        const method = (JSON.parse(init?.body ?? '{}') as { method?: string }).method
        if (method === 'node_getStatus') {
          const height = String(url).includes('seed.test') ? 221 : 219
          return { ok: true, json: async () => ({ result: { chainHeight: height } }) }
        }
        // getBlockByHeight (block-spacing derivation): 20s per block.
        const { height } = JSON.parse(init?.body ?? '{}').params as { height: number }
        return {
          ok: true,
          json: async () => ({ result: { height, timestamp: height * 20 } }),
        }
      }),
    )

    const { result, unmount } = renderHook(() => useFleetStatus(NODES))
    await waitFor(() => expect(result.current.consensusHeight).toBe(221))
    expect(result.current.statuses.seed).toMatchObject({ reachable: true, chainHeight: 221 })
    expect(result.current.statuses.eu).toMatchObject({ reachable: true, chainHeight: 219 })
    await waitFor(() => expect(result.current.avgBlockSeconds).toBe(20))
    unmount()
  })

  it('resolves failed polls to explicit unreachable snapshots', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => { throw new Error('down') }))
    const { result, unmount } = renderHook(() => useFleetStatus(NODES))
    await waitFor(() => expect(result.current.statuses.seed?.reachable).toBe(false))
    expect(result.current.statuses.eu?.reachable).toBe(false)
    expect(result.current.consensusHeight).toBeNull()
    unmount()
  })
})

describe('useFleetHistory', () => {
  it('degrades to unavailable when the metrics backend is absent', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: false, status: 502 })))
    const { result, unmount } = renderHook(() =>
      useFleetHistory(NODES, 'https://metrics.test'),
    )
    await waitFor(() => expect(result.current.historyState).toBe('unavailable'))
    unmount()
  })

  it('loads per-node samples and reports ok', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: true,
        json: async () => [{ timestamp: 1, height: 2, peerCount: 3, mempoolSize: 0 }],
      })),
    )
    const { result, unmount } = renderHook(() =>
      useFleetHistory(NODES, 'https://metrics.test'),
    )
    await waitFor(() => expect(result.current.historyState).toBe('ok'))
    expect(result.current.history.seed).toHaveLength(1)
    unmount()
  })
})
