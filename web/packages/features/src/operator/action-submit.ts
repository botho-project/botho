/**
 * `operator_submitAction` RPC client (#751) — submits a signed envelope to ONE
 * node and parses the node's structured outcome (§6, anti-#541).
 *
 * TRUTHFUL OUTCOMES: applied / refused state comes EXCLUSIVELY from the node's
 * response snapshot. This module never infers "applied" — it maps the node's
 * `OperatorActionOutcome` (`botho/src/operator_action.rs`) 1:1. A refusal is a
 * JSON-RPC error whose `data` carries the same outcome shape; a transport
 * failure is a distinct `unreachable` result (NOT a refusal, NOT a success).
 *
 * Fleet submit (`submitToFleet`) fans one envelope-per-node out concurrently
 * and reports per-node outcomes, so "some nodes refused" is a first-class
 * partial-failure result (§7.3) rather than a single boolean.
 */

import type { FleetNode } from '../network/types'
import type { SignedActionEnvelope } from './action-envelope'

/** Node RPC error codes for the operator write surface (`botho/src/rpc/mod.rs`). */
const OPERATOR_NOT_ENABLED = -32020
const OPERATOR_ACTION_REJECTED = -32024

/** The terminal outcome class the node reports (`OutcomeClass`, snake_case). */
export type OutcomeClass = 'applied' | 'gate_refused' | 'verify_refused'

/** A compact quorum posture the node returns (`QuorumPosture`, §6). */
export interface QuorumPosture {
  mode: string
  members: string[]
  maxAutoMembers: number
}

/** The gate verdict the node returns (`GateVerdict`, §6). */
export interface GateVerdict {
  intersectionRefused: boolean
  curatedMembers: number
  autoMembers: number
  suppressedPeers: number
  maxAutoMembers: number
  faultTolerant: boolean
  degenerate: boolean
}

/**
 * The node's structured action outcome (`OperatorActionOutcome`, camelCase on
 * the wire). Every field is exactly what the node reported — the dashboard
 * renders from THIS, never from inference.
 */
export interface OperatorActionOutcome {
  outcome: OutcomeClass
  dryRun: boolean
  signerKeyId?: string
  action?: string
  message: string
  auditTag: string
  authenticated: boolean
  prevQuorum?: QuorumPosture
  resultingQuorum?: QuorumPosture
  gate?: GateVerdict
}

/**
 * The result of submitting to ONE node. Distinguishes:
 *   - `ok`: the node returned a structured outcome (applied OR refused — check
 *     `outcome.outcome`). This is the ONLY source of applied/refused truth.
 *   - `not-enabled`: the node has no operator write surface configured.
 *   - `unreachable`: transport/HTTP/parse failure — NOT a refusal, NOT applied.
 */
export type SubmitResult =
  | { status: 'ok'; outcome: OperatorActionOutcome }
  | { status: 'not-enabled'; message: string }
  | { status: 'unreachable'; message: string }

/** True when a result is a node-confirmed APPLIED (real, not dry-run) outcome. */
export function isAppliedResult(r: SubmitResult): boolean {
  return r.status === 'ok' && r.outcome.outcome === 'applied' && !r.outcome.dryRun
}

/** True when a result is a node-confirmed refusal (gate or verify). */
export function isRefusedResult(r: SubmitResult): boolean {
  return (
    r.status === 'ok' &&
    (r.outcome.outcome === 'gate_refused' || r.outcome.outcome === 'verify_refused')
  )
}

function parseQuorumPosture(v: unknown): QuorumPosture | undefined {
  if (!v || typeof v !== 'object') return undefined
  const o = v as Record<string, unknown>
  const members = Array.isArray(o.members)
    ? o.members.filter((x): x is string => typeof x === 'string')
    : []
  return {
    mode: typeof o.mode === 'string' ? o.mode : 'unknown',
    members,
    maxAutoMembers: typeof o.maxAutoMembers === 'number' ? o.maxAutoMembers : 0,
  }
}

function parseGate(v: unknown): GateVerdict | undefined {
  if (!v || typeof v !== 'object') return undefined
  const o = v as Record<string, unknown>
  const num = (x: unknown): number => (typeof x === 'number' ? x : 0)
  const bool = (x: unknown): boolean => x === true
  return {
    intersectionRefused: bool(o.intersectionRefused),
    curatedMembers: num(o.curatedMembers),
    autoMembers: num(o.autoMembers),
    suppressedPeers: num(o.suppressedPeers),
    maxAutoMembers: num(o.maxAutoMembers),
    faultTolerant: bool(o.faultTolerant),
    degenerate: bool(o.degenerate),
  }
}

/**
 * Parse a node outcome object (from a success `result` or an error `data`) into
 * a typed {@link OperatorActionOutcome}. Returns `null` if the shape is
 * unrecognizable (which the caller treats as `unreachable`, never as applied).
 */
export function parseOutcome(v: unknown): OperatorActionOutcome | null {
  if (!v || typeof v !== 'object') return null
  const o = v as Record<string, unknown>
  const cls = o.outcome
  if (cls !== 'applied' && cls !== 'gate_refused' && cls !== 'verify_refused') {
    return null
  }
  return {
    outcome: cls,
    dryRun: o.dryRun === true,
    signerKeyId: typeof o.signerKeyId === 'string' ? o.signerKeyId : undefined,
    action: typeof o.action === 'string' ? o.action : undefined,
    message: typeof o.message === 'string' ? o.message : '',
    auditTag: typeof o.auditTag === 'string' ? o.auditTag : '',
    authenticated: o.authenticated === true,
    prevQuorum: parseQuorumPosture(o.prevQuorum),
    resultingQuorum: parseQuorumPosture(o.resultingQuorum),
    gate: parseGate(o.gate),
  }
}

/**
 * Submit a signed envelope to ONE node's `operator_submitAction`. The RPC takes
 * EXACTLY the `{ envelope, signature }` argument (finding 1: no sibling param
 * such as `dryRun` is sent — `dryRun` lives INSIDE the signed envelope). The
 * outcome is read exclusively from the node's response.
 */
export async function submitAction(
  node: FleetNode,
  signed: SignedActionEnvelope,
  timeoutMs = 8000,
): Promise<SubmitResult> {
  const controller = new AbortController()
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const response = await fetch(node.rpcEndpoint, {
      method: 'POST',
      signal: controller.signal,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0',
        method: 'operator_submitAction',
        // EXACTLY these two fields — nothing else influences processing.
        params: { envelope: signed.canonical, signature: signed.signature },
        id: 1,
      }),
    })
    if (!response.ok) {
      return { status: 'unreachable', message: `HTTP ${response.status}` }
    }
    const json = (await response.json()) as {
      result?: unknown
      error?: { code?: number; message?: string; data?: unknown }
    }

    if (json.error) {
      // A refusal is an error whose `data` carries the structured outcome.
      if (json.error.code === OPERATOR_NOT_ENABLED) {
        return {
          status: 'not-enabled',
          message: json.error.message ?? 'operator actions not configured',
        }
      }
      const outcome = parseOutcome(json.error.data)
      if (outcome) return { status: 'ok', outcome }
      if (json.error.code === OPERATOR_ACTION_REJECTED) {
        // Rejected but no parseable outcome payload — surface the message.
        return { status: 'unreachable', message: json.error.message ?? 'rejected' }
      }
      return { status: 'unreachable', message: json.error.message ?? 'rpc error' }
    }

    const outcome = parseOutcome(json.result)
    if (!outcome) return { status: 'unreachable', message: 'unrecognized node response' }
    return { status: 'ok', outcome }
  } catch (e) {
    return { status: 'unreachable', message: e instanceof Error ? e.message : 'network error' }
  } finally {
    clearTimeout(timeoutId)
  }
}

/**
 * Resolve a node's own base58 PeerId from `node_getStatus` (the `peerId`
 * field), which is EXACTLY the value the node's `targetNode` binding check
 * compares against (`botho/src/rpc/mod.rs`). The composer uses this to address
 * each envelope automatically — the operator never types a PeerId, so a
 * fleet-wide action produces one correctly-targeted envelope per node.
 *
 * Returns `null` on any failure (transport / missing field) — the caller must
 * NOT sign an envelope for a node whose PeerId it could not confirm.
 */
export async function fetchNodePeerId(
  node: FleetNode,
  timeoutMs = 5000,
): Promise<string | null> {
  const controller = new AbortController()
  const timeoutId = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const response = await fetch(node.rpcEndpoint, {
      method: 'POST',
      signal: controller.signal,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', method: 'node_getStatus', params: {}, id: 1 }),
    })
    if (!response.ok) return null
    const json = (await response.json()) as { result?: { peerId?: unknown } }
    const peerId = json.result?.peerId
    return typeof peerId === 'string' && peerId.length > 0 ? peerId : null
  } catch {
    return null
  } finally {
    clearTimeout(timeoutId)
  }
}

/** One node's slot in a fleet submit: the node + its per-node signed envelope. */
export interface FleetSubmitItem {
  node: FleetNode
  signed: SignedActionEnvelope
}

/** Per-node result of a fleet submit. */
export interface FleetSubmitEntry {
  nodeId: string
  nodeName: string
  result: SubmitResult
}

/**
 * A fleet submit outcome (§7.3): per-node results PLUS the aggregate partial-
 * failure classification. "Some nodes refused" is a first-class state, not a
 * boolean — the UI must show exactly which nodes did what.
 */
export interface FleetSubmitOutcome {
  entries: FleetSubmitEntry[]
  /** Nodes that confirmed APPLIED (real apply). */
  appliedNodeIds: string[]
  /** Nodes that REFUSED (gate or verify). */
  refusedNodeIds: string[]
  /** Nodes that were unreachable / not-enabled (no verdict). */
  inconclusiveNodeIds: string[]
  /** True when every node applied. */
  allApplied: boolean
  /** True when at least one node applied AND at least one did not — partial. */
  partial: boolean
}

/**
 * Submit a per-node signed envelope to each node concurrently and classify the
 * aggregate. Each node gets its OWN envelope (its own `targetNode` PeerId + its
 * own fresh nonce) — the composer builds N envelopes for a fleet action.
 */
export async function submitToFleet(items: FleetSubmitItem[]): Promise<FleetSubmitOutcome> {
  const entries: FleetSubmitEntry[] = await Promise.all(
    items.map(async ({ node, signed }) => ({
      nodeId: node.id,
      nodeName: node.name,
      result: await submitAction(node, signed),
    })),
  )

  const appliedNodeIds = entries.filter((e) => isAppliedResult(e.result)).map((e) => e.nodeId)
  const refusedNodeIds = entries.filter((e) => isRefusedResult(e.result)).map((e) => e.nodeId)
  const inconclusiveNodeIds = entries
    .filter((e) => !isAppliedResult(e.result) && !isRefusedResult(e.result))
    .map((e) => e.nodeId)

  const allApplied = entries.length > 0 && appliedNodeIds.length === entries.length
  const partial = appliedNodeIds.length > 0 && appliedNodeIds.length < entries.length

  return {
    entries,
    appliedNodeIds,
    refusedNodeIds,
    inconclusiveNodeIds,
    allApplied,
    partial,
  }
}
