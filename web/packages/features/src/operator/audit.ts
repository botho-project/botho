/**
 * `operator_getAuditLog` RPC client (#751) — fetches the node's persisted audit
 * entries (#750's `OperatorAuditEntry` shape, §6). The dashboard's audit view
 * renders EXCLUSIVELY from these stored entries (anti-#541) — it never
 * fabricates or infers history.
 *
 * The read is token-gated (the same operator read token as
 * `operator_getQuorumInfo`); the token is opaque to the client and verified
 * only by the node.
 */

import type { FleetNode } from '../network/types'
import type { OperatorFetchResult } from './types'

const OPERATOR_NOT_ENABLED = -32020
const OPERATOR_TOKEN_REJECTED = -32021

/**
 * One persisted audit entry, mirroring the node's `OperatorAuditEntry` (#750,
 * §6 wire shape). Every field is exactly what the node stored — the view
 * renders these verbatim.
 */
export interface AuditEntry {
  /** Unix seconds the action was processed. */
  ts: number
  /** Fingerprint of the signing key. */
  signerKeyId: string
  /** blake2b-256 hex of the canonical signed envelope bytes. */
  envelopeHash: string
  /** The attempted action (`quorum.pin_member`, ...). */
  action: string
  /** The attempted action params. */
  params: unknown
  /** Whether the action was a dry run. */
  dryRun: boolean
  /** `applied` | `gate_refused` | `verify_refused:<reason>`. */
  outcome: string
  /** Quorum posture BEFORE the edit (present when known). */
  prevQuorum?: unknown
  /** Quorum posture AFTER — present ONLY for `applied` (refusals have no new state). */
  newQuorum?: unknown
  /** Gate snapshot for the evaluation (absent for pre-gate refusals). */
  gate?: unknown
}

/** Parse one raw audit entry, keeping only the node-reported fields. */
function parseEntry(v: unknown): AuditEntry | null {
  if (!v || typeof v !== 'object') return null
  const o = v as Record<string, unknown>
  if (typeof o.action !== 'string' || typeof o.outcome !== 'string') return null
  return {
    ts: typeof o.ts === 'number' ? o.ts : 0,
    signerKeyId: typeof o.signerKeyId === 'string' ? o.signerKeyId : '',
    envelopeHash: typeof o.envelopeHash === 'string' ? o.envelopeHash : '',
    action: o.action,
    params: o.params,
    dryRun: o.dryRun === true,
    outcome: o.outcome,
    // Use `in` so an explicitly-null field stays distinct from an absent one.
    prevQuorum: 'prevQuorum' in o ? o.prevQuorum : undefined,
    newQuorum: 'newQuorum' in o ? o.newQuorum : undefined,
    gate: 'gate' in o ? o.gate : undefined,
  }
}

/**
 * Fetch one node's audit log (token-gated). Returns the node's stored entries
 * (newest first, as the node serves them) or a degraded state. Never fabricates
 * entries.
 */
export async function fetchAuditLog(
  node: FleetNode,
  token: string | null,
  limit = 200,
  timeoutMs = 8000,
): Promise<OperatorFetchResult<AuditEntry[]>> {
  const controller = new AbortController()
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const response = await fetch(node.rpcEndpoint, {
      method: 'POST',
      signal: controller.signal,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method: 'operator_getAuditLog',
        params: { token: token ?? '', limit },
        id: 1,
      }),
    })
    if (!response.ok) return { status: 'unreachable' }
    const json = (await response.json()) as {
      result?: { entries?: unknown }
      error?: { code?: number }
    }
    if (json.error) {
      if (json.error.code === OPERATOR_NOT_ENABLED) return { status: 'not-enabled' }
      if (json.error.code === OPERATOR_TOKEN_REJECTED) return { status: 'unauthorized' }
      return { status: 'unreachable' }
    }
    const raw = json.result?.entries
    if (!Array.isArray(raw)) return { status: 'unreachable' }
    const entries: AuditEntry[] = []
    for (const r of raw) {
      const e = parseEntry(r)
      if (e) entries.push(e)
    }
    return { status: 'ok', data: entries }
  } catch {
    return { status: 'unreachable' }
  } finally {
    clearTimeout(timeoutId)
  }
}
