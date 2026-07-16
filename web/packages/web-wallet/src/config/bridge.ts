/**
 * Bridge order-API configuration for the `/trade` integrated export (#1031).
 *
 * The wallet opens BTH→wBTH mint orders against a CORS-enabled, public bridge
 * order endpoint. That endpoint does NOT exist on the bridge service yet — the
 * service (`bridge/service/src/api.rs`) only exposes operational endpoints and
 * binds loopback by design (it co-hosts an unauthenticated kill switch). Wiring
 * a public, rate-limited, security-reviewed order surface is a tracked
 * fast-follow (see the PR for #1031).
 *
 * Until then this is empty, and the `/trade` page passes a `null` bridge client
 * so the export panel renders an explicit "endpoint not wired yet" state rather
 * than a broken form. Set `VITE_BRIDGE_API_BASE` to point at the endpoint the
 * moment it is stood up — no code change required.
 */
export const BRIDGE_API_BASE: string =
  (import.meta.env.VITE_BRIDGE_API_BASE as string | undefined) ?? ''
