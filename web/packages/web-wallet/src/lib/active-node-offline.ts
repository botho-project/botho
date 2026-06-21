/**
 * Active-node-offline detection for the wallet's offline banner (#492).
 *
 * The wallet routes all RPC through ONE selected "ingress" node. The
 * NetworkSelector already polls per-node health (`node_getStatus`) on a fixed
 * cadence and stores a {@link NodeHealth} snapshot per ingress id. When the
 * user's CURRENTLY SELECTED ingress goes unreachable mid-use there is otherwise
 * no prominent prompt — the dashboard just silently stops updating.
 *
 * This module derives a single boolean "is the active node offline?" signal from
 * the existing health polling (no second poll loop) plus the wallet's
 * `isConnected` / `wsStatus`, and DEBOUNCES it so a single transient blip does
 * not flap the banner on/off.
 */
import type { NodeHealth } from '../config/networks'

/** Inputs to the raw (pre-debounce) offline check. */
export interface ActiveNodeOfflineInput {
  /** The selected ingress id (or 'custom'), from NetworkContext. */
  ingressId: string
  /** Per-ingress health snapshots, from NetworkContext's existing polling. */
  nodeHealth: Record<string, NodeHealth>
  /** Whether the wallet adapter believes it is connected to a node. */
  isConnected: boolean
}

/**
 * RAW (un-debounced) "is the active node offline?" decision.
 *
 * Truthy only when we are CONFIDENT the active ingress is unreachable, so a
 * not-yet-probed ('checking') node never trips the banner:
 *
 *   - Known ingress: trust the polled health snapshot. `offline` => offline;
 *     `online` => not offline; `checking`/absent => unknown, defer to the
 *     connection state below (so a never-probed node that also failed to connect
 *     still surfaces, but a freshly-mounted selector that has not polled yet does
 *     not flash the banner just because health is 'checking').
 *   - Custom endpoint ('custom'): there is no health entry, so fall back purely
 *     to the adapter connection state.
 */
export function isActiveNodeOfflineRaw({
  ingressId,
  nodeHealth,
  isConnected,
}: ActiveNodeOfflineInput): boolean {
  const health = nodeHealth[ingressId]

  // Known node with a definitive health verdict wins.
  if (health) {
    if (health.status === 'offline') return true
    if (health.status === 'online') return false
    // status === 'checking' -> fall through to connection state.
  }

  // No definitive health (custom endpoint, or not yet probed): the node is
  // offline only if the adapter has positively failed to connect.
  return !isConnected
}

/**
 * Number of consecutive RAW-offline observations required before the banner is
 * shown. With the 20s health poll cadence this is ~one extra poll cycle of
 * confirmation, so a single dropped probe does not flap the banner.
 */
export const OFFLINE_DEBOUNCE_TICKS = 2

/** Internal debounce accumulator. */
export interface OfflineDebounceState {
  /** Count of consecutive raw-offline observations (capped). */
  consecutiveOffline: number
  /** The debounced, user-facing "show the banner" verdict. */
  shown: boolean
}

/** Fresh debounce state: nothing observed, banner hidden. */
export function initialDebounceState(): OfflineDebounceState {
  return { consecutiveOffline: 0, shown: false }
}

/**
 * Advance the debounce state by one observation.
 *
 * - A raw-offline observation increments the streak; once it reaches
 *   {@link OFFLINE_DEBOUNCE_TICKS} the banner is shown.
 * - A raw-online observation immediately resets the streak AND hides the banner
 *   (recovery is not debounced — as soon as the node is reachable again we stop
 *   nagging).
 *
 * Pure: returns a NEW state; callers store it (e.g. in a ref/state) across ticks.
 */
export function advanceDebounce(
  prev: OfflineDebounceState,
  rawOffline: boolean,
  ticks: number = OFFLINE_DEBOUNCE_TICKS,
): OfflineDebounceState {
  if (!rawOffline) {
    return { consecutiveOffline: 0, shown: false }
  }
  const consecutiveOffline = Math.min(prev.consecutiveOffline + 1, ticks)
  return {
    consecutiveOffline,
    shown: prev.shown || consecutiveOffline >= ticks,
  }
}
