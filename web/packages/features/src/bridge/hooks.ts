import { useMemo } from 'react'
import { ACTIVE_BRIDGE_NETWORK, getVenues } from './venues'
import type { BridgeNetwork, Venue } from './types'

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
