/**
 * Operator-signed quorum-curation action envelope — the BROWSER side of the
 * write path (#751, P4.4e of the #709 proposal). This is the security core: it
 * MUST byte-for-byte reproduce the node's Rust canonicalization
 * (`botho/src/operator_action.rs`) or the node's parse-after-verify signature
 * check fails (`docs/security/quorum-write-path.md` §3, §4; finding 1).
 *
 * ## Cross-language canonicalization contract (the #1 failure mode)
 *
 * The node signs and verifies over `DOMAIN_SEPARATOR || canonical_bytes`, where
 * `canonical_bytes` is the EXACT UTF-8 JSON string with:
 *   - keys sorted lexicographically at the top level (and `params` keys sorted
 *     too — every object is emitted sorted),
 *   - integers only (no floats, no `1.0`),
 *   - NO insignificant whitespace.
 *
 * The node NEVER re-serializes a parsed object: it verifies over the received
 * bytes and only then parses them. So the browser is the sole producer of the
 * canonical bytes — this module owns that production and never lets the caller
 * hand-edit the signed string. `action-envelope.test.ts` asserts byte-equality
 * against a committed fixture the Rust code produced (`fixtures/`), which is how
 * we guarantee the two languages agree.
 *
 * ## Finding 1 — dryRun is a SIGNED field
 *
 * A dry-run preview and the real apply are DIFFERENT signed byte strings (the
 * `dryRun` value AND a fresh nonce differ). The UI can never "flip a flag" on
 * already-signed bytes; it must sign a fresh envelope. {@link buildRealApply}
 * enforces the fresh nonce.
 *
 * No key material is exposed here: signing takes a {@link SignFn} the vault
 * provides (the raw secret never leaves the vault module).
 */

import { blake2b } from '@noble/hashes/blake2.js'
import { ed25519 } from '@noble/curves/ed25519.js'

/**
 * Domain separator prepended to the canonical bytes before signing / verifying
 * (§3). Prevents cross-protocol signature reuse. MUST equal the node's
 * `DOMAIN_SEPARATOR` (`botho/src/operator_action.rs`).
 */
export const DOMAIN_SEPARATOR = 'botho-operator-action-v1'

/** The only envelope version v1 verifiers accept (§3). */
export const ENVELOPE_VERSION = 1

/** Max envelope lifetime in seconds (`expiresAt - issuedAt <= 300`, §3/§4). */
export const MAX_ENVELOPE_LIFETIME_SECS = 300

/** Default lifetime the composer uses when building an envelope. */
export const DEFAULT_ENVELOPE_LIFETIME_SECS = 120

/** Upper bound on `max_auto_members` (§4 step 7; mirrors the node ceiling). */
export const MAX_AUTO_MEMBERS_CEILING = 64

/**
 * The v1 action allowlist (§3). This is a security invariant on the node
 * (mode/threshold are deliberately excluded to bound a compromised-dashboard
 * attack to recoverable liveness, §8.3); the composer only ever offers these.
 */
export type OperatorActionName =
  | 'quorum.pin_member'
  | 'quorum.unpin_member'
  | 'quorum.set_max_auto_members'

/** The `params` object for each v1 action (matches the node's `parse_action`). */
export type OperatorActionParams =
  | { peerId: string }
  | { value: number }

/**
 * The logical fields of a signed action envelope. This is the operator's
 * INTENT; {@link canonicalizeEnvelope} renders it to the exact signed bytes.
 */
export interface EnvelopeFields {
  /** Envelope version — always {@link ENVELOPE_VERSION}. */
  v: number
  /** The v1 action name. */
  action: OperatorActionName
  /** The action params object. */
  params: OperatorActionParams
  /** The target node's base58 PeerId (must equal that node's own PeerId). */
  targetNode: string
  /** The signer fingerprint (blake2b-256(pubkey)[..8] hex). */
  signerKeyId: string
  /** Fresh 128-bit nonce, lowercase hex (32 chars). */
  nonce: string
  /** Unix seconds the envelope was issued. */
  issuedAt: number
  /** Unix seconds the envelope expires (`<= issuedAt + 300`). */
  expiresAt: number
  /** Whether this is a dry-run preview (SIGNED field, finding 1). */
  dryRun: boolean
  /**
   * Only set (to `true`) when the operator acknowledges a resulting
   * degenerate (<4-node) quorum. Absent otherwise, matching the node's
   * optional field. Never emit `false` — absence and `false` must both mean
   * "not acknowledged" and the node treats absence as `false`.
   */
  acknowledgeDegenerate?: true
}

/** A fully-produced signed envelope: the exact bytes + detached signature. */
export interface SignedActionEnvelope {
  /** The canonical JSON string, verbatim, as signed (the node verifies THIS). */
  canonical: string
  /** Detached Ed25519 signature over `DOMAIN_SEPARATOR || canonical`, hex. */
  signature: string
  /** blake2b-256(canonical) hex — the node's `envelopeHash` (§6). */
  envelopeHash: string
  /** The logical fields, for display / diffing (NOT what is signed). */
  fields: EnvelopeFields
}

/** A signing function: sign 32-... arbitrary bytes, return the 64-byte sig. */
export type SignFn = (message: Uint8Array) => Uint8Array

// ---------------------------------------------------------------------------
// hex helpers (self-contained; mirrors vault.ts style)
// ---------------------------------------------------------------------------

function bytesToHex(bytes: Uint8Array): string {
  let out = ''
  for (const b of bytes) out += b.toString(16).padStart(2, '0')
  return out
}

// ---------------------------------------------------------------------------
// Canonical JSON (MUST byte-match the node)
// ---------------------------------------------------------------------------

/**
 * Serialize one JSON value in the node's canonical form: object keys sorted
 * lexicographically (by UTF-16 code unit, which matches Rust's `BTreeMap`
 * ordering for the ASCII field names used here), integers only, no whitespace.
 *
 * Throws on any non-integer number or non-finite value — the node rejects
 * floats (`serde_json` `as_u64` returns None for `5.0`), so producing one here
 * would guarantee a signature the node cannot accept. Fail loud at sign time.
 */
function canonicalizeValue(value: unknown): string {
  if (value === null) return 'null'
  if (typeof value === 'boolean') return value ? 'true' : 'false'
  if (typeof value === 'number') {
    if (!Number.isInteger(value)) {
      throw new Error(
        `canonical envelope requires integers only; got non-integer ${value}`,
      )
    }
    // Number.isInteger already excludes NaN/Infinity. Emit the plain decimal.
    return String(value)
  }
  if (typeof value === 'string') return JSON.stringify(value)
  if (Array.isArray(value)) {
    return `[${value.map(canonicalizeValue).join(',')}]`
  }
  if (typeof value === 'object') {
    const obj = value as Record<string, unknown>
    const keys = Object.keys(obj).sort()
    const parts = keys.map((k) => `${JSON.stringify(k)}:${canonicalizeValue(obj[k])}`)
    return `{${parts.join(',')}}`
  }
  throw new Error(`cannot canonicalize value of type ${typeof value}`)
}

/**
 * Render {@link EnvelopeFields} to the canonical signed byte string.
 *
 * We build a plain object with exactly the node's known-key set and let
 * {@link canonicalizeValue} sort + serialize it. `acknowledgeDegenerate` is
 * included ONLY when truthy (matching the node's optional field). This is the
 * ONE place the signed bytes are produced; nothing else constructs them.
 */
export function canonicalizeEnvelope(fields: EnvelopeFields): string {
  const obj: Record<string, unknown> = {
    v: fields.v,
    action: fields.action,
    params: fields.params,
    targetNode: fields.targetNode,
    signerKeyId: fields.signerKeyId,
    nonce: fields.nonce,
    issuedAt: fields.issuedAt,
    expiresAt: fields.expiresAt,
    dryRun: fields.dryRun,
  }
  if (fields.acknowledgeDegenerate === true) {
    obj.acknowledgeDegenerate = true
  }
  return canonicalizeValue(obj)
}

/** blake2b-256 hex of arbitrary bytes — the node's `blake2b_256_hex` (§6). */
export function blake2b256Hex(bytes: Uint8Array): string {
  return bytesToHex(blake2b(bytes, { dkLen: 32 }))
}

/**
 * Compute the signer fingerprint (`signerKeyId`): blake2b-256(pubkey) truncated
 * to the first 8 bytes, lowercase hex. MUST match the node's `fingerprint_hex`
 * (`botho/src/operator_key.rs`, `FINGERPRINT_BYTES = 8`).
 */
export function signerKeyIdFromPublicKey(publicKey: Uint8Array): string {
  return bytesToHex(blake2b(publicKey, { dkLen: 32 }).slice(0, 8))
}

/** The exact bytes that get signed: `DOMAIN_SEPARATOR || canonical`. */
export function signingMessage(canonical: string): Uint8Array {
  const domain = new TextEncoder().encode(DOMAIN_SEPARATOR)
  const body = new TextEncoder().encode(canonical)
  const out = new Uint8Array(domain.length + body.length)
  out.set(domain, 0)
  out.set(body, domain.length)
  return out
}

/**
 * Verify a detached signature over an envelope's canonical bytes with the
 * domain separator, against a public key. Used by tests and as a defensive
 * self-check before submitting (a bad self-signature means something is wrong
 * locally — never send it).
 */
export function verifyEnvelopeSignature(
  canonical: string,
  signatureHex: string,
  publicKey: Uint8Array,
): boolean {
  try {
    const sig = hexToBytesStrict(signatureHex)
    return ed25519.verify(sig, signingMessage(canonical), publicKey)
  } catch {
    return false
  }
}

function hexToBytesStrict(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) throw new Error('Invalid hex string')
  const out = new Uint8Array(hex.length / 2)
  for (let i = 0; i < out.length; i++) {
    const b = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
    if (Number.isNaN(b)) throw new Error('Invalid hex string')
    out[i] = b
  }
  return out
}

// ---------------------------------------------------------------------------
// Nonce + envelope construction
// ---------------------------------------------------------------------------

/** A fresh 128-bit nonce, lowercase hex (32 chars). */
export function freshNonce(): string {
  return bytesToHex(crypto.getRandomValues(new Uint8Array(16)))
}

/** Current Unix seconds. */
function nowSecs(): number {
  return Math.floor(Date.now() / 1000)
}

/** The composer's request (before nonce/time/dryRun are stamped on). */
export interface ComposeActionRequest {
  action: OperatorActionName
  params: OperatorActionParams
  /** The node's base58 PeerId (resolved live from `node_getStatus`). */
  targetNode: string
  /** The signer fingerprint. */
  signerKeyId: string
  /** Set `true` when the operator acknowledges a degenerate result. */
  acknowledgeDegenerate?: boolean
  /** Envelope lifetime; defaults to {@link DEFAULT_ENVELOPE_LIFETIME_SECS}. */
  lifetimeSecs?: number
}

/**
 * Validate a compose request's payload (mirrors the node's shape checks so the
 * operator sees an error BEFORE signing, not after a node rejection). Throws
 * with a human message on the first problem.
 */
export function validateComposeRequest(req: ComposeActionRequest): void {
  if (!req.targetNode || req.targetNode.trim() === '') {
    throw new Error('targetNode (a node PeerId) is required')
  }
  if (!req.signerKeyId || req.signerKeyId.trim() === '') {
    throw new Error('signerKeyId is required (import the operator key first)')
  }
  switch (req.action) {
    case 'quorum.pin_member':
    case 'quorum.unpin_member': {
      const p = req.params as { peerId?: unknown }
      if (typeof p.peerId !== 'string' || p.peerId.trim() === '') {
        throw new Error('a peerId is required for pin/unpin')
      }
      break
    }
    case 'quorum.set_max_auto_members': {
      const p = req.params as { value?: unknown }
      if (typeof p.value !== 'number' || !Number.isInteger(p.value)) {
        throw new Error('value must be an integer for set_max_auto_members')
      }
      if (p.value < 0 || p.value > MAX_AUTO_MEMBERS_CEILING) {
        throw new Error(`value must be in 0..=${MAX_AUTO_MEMBERS_CEILING}`)
      }
      break
    }
    default:
      throw new Error(`unsupported action ${String(req.action)}`)
  }
}

/**
 * Build the envelope fields for a request, stamping a fresh nonce + issuedAt /
 * expiresAt and the given `dryRun`. Every call produces a DISTINCT nonce, so a
 * dry-run and its real apply are always different signed strings (finding 1).
 */
export function buildEnvelopeFields(
  req: ComposeActionRequest,
  dryRun: boolean,
  now: number = nowSecs(),
): EnvelopeFields {
  validateComposeRequest(req)
  const lifetime = Math.min(
    req.lifetimeSecs ?? DEFAULT_ENVELOPE_LIFETIME_SECS,
    MAX_ENVELOPE_LIFETIME_SECS,
  )
  const fields: EnvelopeFields = {
    v: ENVELOPE_VERSION,
    action: req.action,
    params: req.params,
    targetNode: req.targetNode,
    signerKeyId: req.signerKeyId,
    nonce: freshNonce(),
    issuedAt: now,
    expiresAt: now + lifetime,
    dryRun,
  }
  if (req.acknowledgeDegenerate === true) {
    fields.acknowledgeDegenerate = true
  }
  return fields
}

/**
 * Canonicalize + sign an envelope. The signature is over
 * `DOMAIN_SEPARATOR || canonical`. Signing is delegated to {@link SignFn} so
 * the raw secret never enters this module.
 */
export function signEnvelope(fields: EnvelopeFields, sign: SignFn): SignedActionEnvelope {
  const canonical = canonicalizeEnvelope(fields)
  const signature = bytesToHex(sign(signingMessage(canonical)))
  return {
    canonical,
    signature,
    envelopeHash: blake2b256Hex(new TextEncoder().encode(canonical)),
    fields,
  }
}

/** Compose + sign a `dryRun: true` preview envelope. */
export function buildDryRun(
  req: ComposeActionRequest,
  sign: SignFn,
  now?: number,
): SignedActionEnvelope {
  return signEnvelope(buildEnvelopeFields(req, true, now), sign)
}

/**
 * Compose + sign the REAL `dryRun: false` apply envelope with a FRESH nonce
 * (finding 1: the UI can never reuse the dry-run bytes — this produces a new
 * envelope with its own nonce and its own signature).
 */
export function buildRealApply(
  req: ComposeActionRequest,
  sign: SignFn,
  now?: number,
): SignedActionEnvelope {
  return signEnvelope(buildEnvelopeFields(req, false, now), sign)
}
