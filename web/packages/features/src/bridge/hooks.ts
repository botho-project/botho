import { useEffect, useMemo, useState } from 'react'
import { ACTIVE_BRIDGE_NETWORK, getVenues } from './venues'
import type {
  BridgeNetwork,
  BridgeStats,
  BridgeStatsClientLike,
  BridgeStatsState,
  Venue,
} from './types'

/**
 * Bridge feature hooks (#1030), mirroring `network/hooks.ts`.
 *
 * Tier 0 has no live data plane of its own — peg health is reused from the
 * network module's `useReserveProof`. This hook just resolves the venue
 * directory for the active network, memoized so the reference is stable across
 * renders (venues.ts is static config today; the memo keeps callers honest if
 * the source becomes dynamic in a later tier).
 */
export interface UseBridgeVenuesResult {
  /** Venues to render for the requested network. */
  venues: Venue[]
  /** The network these venues belong to. */
  network: BridgeNetwork
}

export function useBridgeVenues(
  network: BridgeNetwork = ACTIVE_BRIDGE_NETWORK,
): UseBridgeVenuesResult {
  const venues = useMemo(() => getVenues(network), [network])
  return { venues, network }
}

export interface UseBridgeStatsOptions {
  /** Stats refresh cadence in ms. The server caches the aggregate for ~30 s,
   * so polling faster than that only re-reads the cache. */
  refreshMs?: number
}

export interface UseBridgeStatsResult {
  /** Latest aggregate, or null until the first successful poll. */
  stats: BridgeStats | null
  /** Discriminated fetch outcome (see {@link BridgeStatsState}). */
  state: BridgeStatsState
}

/**
 * Poll aggregate wrap/unwrap activity from the public bridge order API
 * (#1054), mirroring `network/hooks.ts`'s `useReserveProof` poll +
 * `cancelled` cleanup structure.
 *
 * `client === null` means no public bridge API is configured
 * (`VITE_BRIDGE_API_BASE` unset — it is disabled by default on nodes); that
 * maps to `absent`, the "hide the card" case. A configured-but-unreachable
 * endpoint (network error / non-OK) maps to `unavailable` — the card renders
 * a grayed placeholder and never fabricates values (#541 lesson).
 */
export function useBridgeStats(
  client: BridgeStatsClientLike | null,
  { refreshMs = 60_000 }: UseBridgeStatsOptions = {},
): UseBridgeStatsResult {
  const [stats, setStats] = useState<BridgeStats | null>(null)
  const [state, setState] = useState<BridgeStatsState>(
    client === null ? 'absent' : 'unavailable',
  )

  useEffect(() => {
    if (client === null) {
      setStats(null)
      setState('absent')
      return
    }
    let cancelled = false
    const load = async () => {
      try {
        const data = await client.getBridgeStats()
        if (cancelled) return
        setStats(data)
        setState('ok')
      } catch {
        if (!cancelled) setState('unavailable')
      }
    }
    load()
    const id = setInterval(load, refreshMs)
    return () => {
      cancelled = true
      clearInterval(id)
    }
  }, [client, refreshMs])

  return { stats, state }
}
