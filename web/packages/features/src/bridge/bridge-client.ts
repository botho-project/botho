/**
 * Typed client for the user-facing bridge order API (#1031, epic #1029).
 *
 * The wallet opens a **mint order** (BTH → wBTH) and then polls its status. The
 * wallet itself never touches the counterparty chain — it only builds/signs the
 * BTH deposit and watches the order walk its state machine.
 *
 * ─── Backend reachability (READ THIS) ──────────────────────────────────────
 * As of this change the bridge service (`bridge/service/src/api.rs`) exposes
 * ONLY operational endpoints (`/health`, `/metrics`, `/api/status`,
 * `/api/breaker`, `/api/reserve/proof`). It exposes NO order-create/status
 * endpoint, has NO CORS layer, and binds loopback by default *by design* —
 * that router co-hosts `POST /api/breaker`, an unauthenticated kill switch, so
 * it must not be published to browsers. A user-facing order surface therefore
 * needs a SEPARATE, CORS-enabled, rate-limited public bind with input
 * validation and its own security review (it is a minting surface, i.e. money),
 * which is out of scope for this web-wallet PR and tracked as a fast-follow.
 *
 * This client codifies the contract that fast-follow must honor so the UI is
 * complete and correct the moment the endpoint lands:
 *   - `POST {base}/api/bridge/orders`               → 200 {@link MintOrder}
 *   - `GET  {base}/api/bridge/orders/{id}`          → 200 {@link MintOrder}
 *   - `POST {base}/api/bridge/release-orders`       → 200 {@link ReleaseOrder}
 *   - `GET  {base}/api/bridge/release-orders/{id}`  → 200 {@link ReleaseOrder}
 *
 * The release-order endpoints (Unwrap, #1032) share the same #1036 backend
 * fast-follow — the burn happens in the user's OWN counterparty wallet, and the
 * order only registers the intent (source chain + BTH release address + amount)
 * so the bridge can correlate the burn and track the release to `released`.
 *
 * Until a base URL is configured (`VITE_BRIDGE_API_BASE`), the wallet passes a
 * `null` client and the panels render an explicit "endpoint not wired yet"
 * state — they never silently pretend to work.
 */
import type {
  CreateMintOrderRequest,
  CreateReleaseOrderRequest,
  MintOrder,
  ReleaseOrder,
} from './types'

/** Error thrown by {@link BridgeClient} calls that reach the network. */
export class BridgeApiError extends Error {
  /** HTTP status, when the failure came back as a response. */
  readonly status?: number
  constructor(message: string, status?: number) {
    super(message)
    this.name = 'BridgeApiError'
    this.status = status
  }
}

/** The user-facing bridge order API. */
export interface BridgeClient {
  /** Open a mint order; returns the deposit address + memo to send BTH to. */
  createMintOrder(req: CreateMintOrderRequest): Promise<MintOrder>
  /** Fetch the latest state of a mint order by id. */
  getOrderStatus(id: string): Promise<MintOrder>
  /** Open a release order (Unwrap, #1032); registers the burn intent. */
  createReleaseOrder(req: CreateReleaseOrderRequest): Promise<ReleaseOrder>
  /** Fetch the latest state of a release order by id. */
  getReleaseOrderStatus(id: string): Promise<ReleaseOrder>
}

/** Strip a single trailing slash so `${base}/api/...` never doubles up. */
function normalizeBase(baseUrl: string): string {
  return baseUrl.replace(/\/+$/, '')
}

async function parseJson(res: Response): Promise<unknown> {
  const text = await res.text()
  if (!res.ok) {
    // Surface a server-provided `{ "error": "..." }` when present, else the
    // status line — the panel shows this verbatim (e.g. "amount below fee").
    let detail = text
    try {
      const body = JSON.parse(text) as { error?: string }
      if (body && typeof body.error === 'string') detail = body.error
    } catch {
      /* non-JSON error body; fall back to raw text */
    }
    throw new BridgeApiError(
      detail || `bridge API ${res.status}`,
      res.status,
    )
  }
  try {
    return JSON.parse(text)
  } catch {
    throw new BridgeApiError('bridge API returned a non-JSON body')
  }
}

/**
 * Build a {@link BridgeClient} bound to `baseUrl` (e.g. the CORS-enabled bridge
 * order endpoint served alongside the metrics/seed infra). `fetchImpl` is
 * injectable for tests; it defaults to the global `fetch`.
 */
export function createBridgeClient(
  baseUrl: string,
  fetchImpl: typeof fetch = fetch,
): BridgeClient {
  const base = normalizeBase(baseUrl)

  return {
    async createMintOrder(req: CreateMintOrderRequest): Promise<MintOrder> {
      const res = await fetchImpl(`${base}/api/bridge/orders`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(req),
      })
      return (await parseJson(res)) as MintOrder
    },

    async getOrderStatus(id: string): Promise<MintOrder> {
      const res = await fetchImpl(
        `${base}/api/bridge/orders/${encodeURIComponent(id)}`,
      )
      return (await parseJson(res)) as MintOrder
    },

    async createReleaseOrder(
      req: CreateReleaseOrderRequest,
    ): Promise<ReleaseOrder> {
      const res = await fetchImpl(`${base}/api/bridge/release-orders`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(req),
      })
      return (await parseJson(res)) as ReleaseOrder
    },

    async getReleaseOrderStatus(id: string): Promise<ReleaseOrder> {
      const res = await fetchImpl(
        `${base}/api/bridge/release-orders/${encodeURIComponent(id)}`,
      )
      return (await parseJson(res)) as ReleaseOrder
    },
  }
}
