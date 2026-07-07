import type { FleetNode } from '@botho/features'
import { INGRESS_NODES } from './networks'

/**
 * Shared fleet-dashboard configuration for the `/network` and `/operator`
 * pages (#698, #706) — one source of truth so the two surfaces can never
 * drift apart on which nodes they watch.
 */

/** Metrics-daemon fleet API (#697), served via the faucet's nginx. */
export const METRICS_API_BASE = 'https://faucet.botho.io/metrics-api'

/**
 * The dashboards watch every ingress node plus region labels.
 *
 * Module-level constant on purpose: the fleet hooks take it as an effect
 * dependency, so it must be referentially stable.
 */
export const FLEET: FleetNode[] = INGRESS_NODES.map((n) => ({
  id: n.id,
  name: n.name,
  rpcEndpoint: n.rpcEndpoint,
}))
