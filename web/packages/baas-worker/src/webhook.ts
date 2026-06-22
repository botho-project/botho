/**
 * Stripe webhook → provision / deprovision (P7.2 of #458, this is #506).
 *
 * This is the security-critical JOIN between billing and auto-provisioning:
 * a paid Stripe event is the ONLY thing that may launch (or tear down) a managed
 * rig. There is deliberately no public `/provision` route (#458 §5).
 *
 * Guarantees enforced here (#458 §2, §3, §5):
 *  1. **Signature verification first.** Every request must carry a valid
 *     `Stripe-Signature` header that is an HMAC-SHA256 over the *raw* request
 *     body using `STRIPE_WEBHOOK_SECRET`, within a timestamp tolerance to defeat
 *     replay. Unsigned / mismatched / stale requests are rejected with 400
 *     BEFORE any side effect (no parsing of the body into an event, no
 *     provisioner call).
 *  2. **Raw-body verification.** We verify the exact bytes Stripe signed; we do
 *     NOT JSON-parse before verifying (parse-then-reserialize would change the
 *     bytes and break the HMAC — and would also expose a parser to unverified
 *     input).
 *  3. **Provision triggers:** `checkout.session.completed`, `invoice.paid`
 *     → `provisionRig`.
 *  4. **Teardown triggers:** `customer.subscription.deleted`,
 *     `invoice.payment_failed` (past grace) → `teardownRig`.
 *  5. **Idempotency:** Stripe retries deliveries. We rely on the provisioner's
 *     own idempotency (it dedups by `subscription_id` via D1 + the EC2 tag), so a
 *     replayed delivery never double-provisions. Unknown event types are a
 *     graceful 2xx no-op.
 *  6. **Fast ACK:** the handler returns 2xx quickly. The provisioner core is
 *     structured for idempotent retry; we call it directly and ack. (If a Queue
 *     / Durable Object is introduced later per #458 §3, the enqueue replaces the
 *     direct call without changing this contract.)
 *
 * The signature crypto + event mapping are pure functions so they can be
 * unit-tested with a known secret and forged/tampered inputs, and the
 * provisioner is injected (fakes) so NO real AWS/Stripe/DNS call happens in a
 * test path.
 */

import type {
  ProvisionerDeps,
  ProvisionOutcome,
  ProvisionRequest,
} from './provisioner'
import { provisionRig, teardownRig } from './provisioner'

/** Worker env keys this module needs. The signing secret is a Worker secret. */
export interface WebhookEnv {
  /**
   * Stripe webhook signing secret (`whsec_...`). Worker secret, never the repo.
   * Used to verify the `Stripe-Signature` HMAC over the raw body (#458 §5).
   */
  STRIPE_WEBHOOK_SECRET?: string
}

/**
 * Maximum age (seconds) of a signed event before we reject it as stale, to
 * bound replay attacks. Matches Stripe's own default tolerance (5 minutes).
 */
export const DEFAULT_SIGNATURE_TOLERANCE_SECONDS = 300

/** Stripe event types that trigger a provision (#458 §2). */
export const PROVISION_EVENTS = [
  'checkout.session.completed',
  'invoice.paid',
] as const

/** Stripe event types that trigger a teardown (#458 §2, §5). */
export const TEARDOWN_EVENTS = [
  'customer.subscription.deleted',
  'invoice.payment_failed',
] as const

export type WebhookAction = 'provision' | 'teardown' | 'ignore'

/** Outcome of a signature-verification attempt. */
export type SignatureResult =
  | { ok: true }
  | { ok: false; reason: string }

/**
 * Verify a Stripe `Stripe-Signature` header against the raw request body.
 *
 * The header has the form `t=<unix-ts>,v1=<hex-hmac>[,v1=<hex-hmac>...]`. Stripe
 * computes the signature as `HMAC-SHA256(secret, "<t>.<rawBody>")`. We:
 *   - parse out `t` and all `v1` candidates,
 *   - reject if the timestamp is missing/old (replay window — #458 §5),
 *   - recompute the HMAC over `"<t>.<rawBody>"` and constant-time compare it
 *     against each provided `v1` (a single match passes).
 *
 * `rawBody` MUST be the exact bytes received (do not parse/reserialize first).
 */
export async function verifyStripeSignature(
  rawBody: string,
  signatureHeader: string | null,
  secret: string,
  opts: { toleranceSeconds?: number; nowSeconds?: number } = {},
): Promise<SignatureResult> {
  if (!secret) return { ok: false, reason: 'webhook secret not configured' }
  if (!signatureHeader) return { ok: false, reason: 'missing Stripe-Signature header' }

  const parsed = parseSignatureHeader(signatureHeader)
  if (parsed.timestamp === undefined) {
    return { ok: false, reason: 'malformed signature header: no timestamp' }
  }
  if (parsed.v1.length === 0) {
    return { ok: false, reason: 'malformed signature header: no v1 signature' }
  }

  // Replay / freshness window. Reject timestamps too far in the past OR future.
  const tolerance = opts.toleranceSeconds ?? DEFAULT_SIGNATURE_TOLERANCE_SECONDS
  const now = opts.nowSeconds ?? Math.floor(Date.now() / 1000)
  if (Math.abs(now - parsed.timestamp) > tolerance) {
    return { ok: false, reason: 'timestamp outside tolerance window' }
  }

  const expected = await hmacSha256Hex(secret, `${parsed.timestamp}.${rawBody}`)

  // Constant-time compare against every supplied candidate.
  for (const candidate of parsed.v1) {
    if (timingSafeEqualHex(expected, candidate)) {
      return { ok: true }
    }
  }
  return { ok: false, reason: 'signature mismatch' }
}

/** Parsed `Stripe-Signature` header fields. */
interface ParsedSignature {
  timestamp?: number
  v1: string[]
}

/** Parse `t=...,v1=...,v1=...` into a timestamp + list of v1 candidates. */
export function parseSignatureHeader(header: string): ParsedSignature {
  const result: ParsedSignature = { v1: [] }
  for (const part of header.split(',')) {
    const eq = part.indexOf('=')
    if (eq === -1) continue
    const key = part.slice(0, eq).trim()
    const value = part.slice(eq + 1).trim()
    if (key === 't') {
      const n = Number(value)
      if (Number.isFinite(n)) result.timestamp = n
    } else if (key === 'v1') {
      if (value) result.v1.push(value)
    }
  }
  return result
}

/** Decide what action a verified Stripe event type maps to (#458 §2). */
export function actionForEventType(type: string): WebhookAction {
  if ((PROVISION_EVENTS as readonly string[]).includes(type)) return 'provision'
  if ((TEARDOWN_EVENTS as readonly string[]).includes(type)) return 'teardown'
  return 'ignore'
}

/** Minimal shape of the Stripe event we read after verification. */
interface StripeEvent {
  id?: string
  type?: string
  data?: { object?: StripeEventObject }
}

/**
 * The union of fields we may read off the event object across the handful of
 * event types we act on. Stripe nests these differently per type, so we probe
 * several locations (see `extractProvisionRequest` / `extractSubscriptionId`).
 */
interface StripeEventObject {
  // checkout.session.completed
  subscription?: string | { id?: string }
  customer?: string | { id?: string }
  metadata?: Record<string, unknown>
  // invoice.paid / invoice.payment_failed
  subscription_details?: { metadata?: Record<string, unknown> }
  lines?: { data?: Array<{ metadata?: Record<string, unknown> }> }
  // customer.subscription.deleted (object IS the subscription)
  id?: string
}

/** Result of mapping a verified event into a provisioner call. */
export type WebhookHandled =
  | { action: 'provision'; outcome: ProvisionOutcome; subscriptionId: string }
  | { action: 'teardown'; result: { ok: boolean; error?: string }; subscriptionId: string }
  | { action: 'ignore'; reason: string }

/**
 * Coerce a Stripe id field that may be either a bare id string or an expanded
 * object `{ id }` into the id string (or undefined).
 */
function asId(v: string | { id?: string } | undefined): string | undefined {
  if (typeof v === 'string') return v || undefined
  if (v && typeof v === 'object' && typeof v.id === 'string') return v.id || undefined
  return undefined
}

/** Read a string `region` out of any of the metadata locations Stripe uses. */
function regionFromMetadata(obj: StripeEventObject): string | undefined {
  const candidates: Array<Record<string, unknown> | undefined> = [
    obj.metadata,
    obj.subscription_details?.metadata,
    obj.lines?.data?.[0]?.metadata,
  ]
  for (const m of candidates) {
    const r = m?.region
    if (typeof r === 'string' && r.length > 0) return r
  }
  return undefined
}

/**
 * Extract the `subscription_id` to key teardown/idempotency on. For
 * `customer.subscription.deleted` the event object IS the subscription, so its
 * `id` is the subscription id; for invoices/checkout sessions it is the
 * `subscription` field.
 */
export function extractSubscriptionId(event: StripeEvent): string | undefined {
  const obj = event.data?.object
  if (!obj) return undefined
  if (event.type === 'customer.subscription.deleted') {
    return typeof obj.id === 'string' ? obj.id || undefined : undefined
  }
  return asId(obj.subscription)
}

/**
 * Extract the `ProvisionRequest` from a provision-type event. Returns undefined
 * if the event lacks the fields the provisioner needs (subscription/customer),
 * which the caller maps to a no-op 2xx (Stripe sends some unrelated events that
 * share a type filter).
 */
export function extractProvisionRequest(
  event: StripeEvent,
): ProvisionRequest | undefined {
  const obj = event.data?.object
  if (!obj) return undefined
  const subscriptionId = asId(obj.subscription)
  const customerId = asId(obj.customer)
  const region = regionFromMetadata(obj)
  if (!subscriptionId || !customerId || !region) return undefined
  return { subscriptionId, customerId, region }
}

/**
 * Map a verified Stripe event to the appropriate provisioner call. Pure w.r.t.
 * the injected `deps` (fakes in tests). Idempotency is the provisioner's job —
 * a replayed delivery flows through `provisionRig`/`teardownRig` which dedup by
 * subscription id (#458 §3, §5).
 */
export async function handleStripeEvent(
  event: StripeEvent,
  deps: ProvisionerDeps,
): Promise<WebhookHandled> {
  const type = event.type ?? ''
  const action = actionForEventType(type)

  if (action === 'provision') {
    const req = extractProvisionRequest(event)
    if (!req) {
      return { action: 'ignore', reason: `provision event "${type}" missing subscription/customer/region` }
    }
    const outcome = await provisionRig(req, deps)
    return { action: 'provision', outcome, subscriptionId: req.subscriptionId }
  }

  if (action === 'teardown') {
    const subscriptionId = extractSubscriptionId(event)
    if (!subscriptionId) {
      return { action: 'ignore', reason: `teardown event "${type}" missing subscription id` }
    }
    const result = await teardownRig(subscriptionId, deps)
    return { action: 'teardown', result, subscriptionId }
  }

  return { action: 'ignore', reason: `unhandled event type "${type}"` }
}

// ---------------------------------------------------------------------------
// crypto helpers (Web Crypto — works in workerd and Node's test runtime)
// ---------------------------------------------------------------------------

/** HMAC-SHA256(secret, message) returned as lowercase hex. */
export async function hmacSha256Hex(secret: string, message: string): Promise<string> {
  const enc = new TextEncoder()
  const key = await crypto.subtle.importKey(
    'raw',
    enc.encode(secret),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  )
  const sig = await crypto.subtle.sign('HMAC', key, enc.encode(message))
  return bytesToHex(new Uint8Array(sig))
}

function bytesToHex(bytes: Uint8Array): string {
  let hex = ''
  for (let i = 0; i < bytes.length; i++) {
    hex += bytes[i].toString(16).padStart(2, '0')
  }
  return hex
}

/**
 * Constant-time comparison of two hex strings. Returns false immediately for a
 * length mismatch (the length itself isn't secret), otherwise XOR-accumulates
 * over all chars so timing does not leak the first differing position.
 */
export function timingSafeEqualHex(a: string, b: string): boolean {
  if (a.length !== b.length) return false
  let diff = 0
  for (let i = 0; i < a.length; i++) {
    diff |= a.charCodeAt(i) ^ b.charCodeAt(i)
  }
  return diff === 0
}
