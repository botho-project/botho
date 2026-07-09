import { describe, expect, it } from 'vitest'
import { ed25519 } from '@noble/curves/ed25519.js'
import {
  buildDryRun,
  buildEnvelopeFields,
  buildRealApply,
  canonicalizeEnvelope,
  blake2b256Hex,
  DOMAIN_SEPARATOR,
  signerKeyIdFromPublicKey,
  signingMessage,
  verifyEnvelopeSignature,
  type ComposeActionRequest,
  type EnvelopeFields,
} from './action-envelope'
import fixtures from './fixtures/operator-action-fixtures.json'

/**
 * CROSS-LANGUAGE CANONICALIZATION FIXTURE TEST (issue #751, the #1 failure
 * mode). The fixture values are produced by the NODE's Rust code
 * (`botho/src/operator_action.rs`) for the deterministic key seed [1u8; 32].
 *
 * If the browser's canonical byte string, Ed25519 signature, signerKeyId, or
 * envelopeHash drift from these, the node's parse-after-verify signature check
 * fails. These asserts guarantee byte-for-byte agreement with the node.
 */

const SEED = hexToBytes(fixtures.signingKeySeed)

function hexToBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2)
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
  return out
}
function bytesToHex(b: Uint8Array): string {
  return [...b].map((x) => x.toString(16).padStart(2, '0')).join('')
}

/** A SignFn bound to the fixture seed key (matches the Rust signer). */
const fixtureSign = (msg: Uint8Array) => ed25519.sign(msg, SEED)

describe('cross-language canonicalization fixture (matches the node)', () => {
  it('derives the node public key + signerKeyId from the seed', () => {
    const pk = ed25519.getPublicKey(SEED)
    expect(bytesToHex(pk)).toBe(fixtures.publicKeyHex)
    expect(signerKeyIdFromPublicKey(pk)).toBe(fixtures.signerKeyId)
  })

  it('uses the same domain separator as the node', () => {
    expect(DOMAIN_SEPARATOR).toBe(fixtures.domainSeparator)
  })

  for (const fx of fixtures.envelopes) {
    describe(fx.name, () => {
      const fields = fx.fields as unknown as EnvelopeFields

      it('produces the EXACT canonical byte string the node produced', () => {
        expect(canonicalizeEnvelope(fields)).toBe(fx.canonical)
      })

      it('produces the EXACT Ed25519 signature the node accepts', () => {
        const sig = bytesToHex(fixtureSign(signingMessage(fx.canonical)))
        expect(sig).toBe(fx.signature)
      })

      it('the fixture signature verifies against the node public key', () => {
        const pk = ed25519.getPublicKey(SEED)
        expect(verifyEnvelopeSignature(fx.canonical, fx.signature, pk)).toBe(true)
      })

      if ('envelopeHash' in fx && fx.envelopeHash) {
        it('produces the EXACT envelopeHash the node stores (§6)', () => {
          expect(blake2b256Hex(new TextEncoder().encode(fx.canonical))).toBe(fx.envelopeHash)
        })
      }
    })
  }
})

describe('canonicalization rules', () => {
  it('sorts keys lexicographically with acknowledgeDegenerate first', () => {
    const fields: EnvelopeFields = {
      v: 1,
      action: 'quorum.unpin_member',
      params: { peerId: 'P' },
      targetNode: 'T',
      signerKeyId: 'S',
      nonce: 'N',
      issuedAt: 1,
      expiresAt: 2,
      dryRun: false,
      acknowledgeDegenerate: true,
    }
    const c = canonicalizeEnvelope(fields)
    expect(c.startsWith('{"acknowledgeDegenerate":true,"action":')).toBe(true)
    // Keys appear in sorted order, no whitespace.
    expect(c).not.toContain(' ')
    expect(c.endsWith('"v":1}')).toBe(true)
  })

  it('omits acknowledgeDegenerate entirely when not set (matches optional node field)', () => {
    const fields: EnvelopeFields = {
      v: 1,
      action: 'quorum.pin_member',
      params: { peerId: 'P' },
      targetNode: 'T',
      signerKeyId: 'S',
      nonce: 'N',
      issuedAt: 1,
      expiresAt: 2,
      dryRun: false,
    }
    expect(canonicalizeEnvelope(fields)).not.toContain('acknowledgeDegenerate')
  })

  it('rejects non-integer numbers (the node rejects floats)', () => {
    const fields = {
      v: 1,
      action: 'quorum.set_max_auto_members',
      params: { value: 8.5 },
      targetNode: 'T',
      signerKeyId: 'S',
      nonce: 'N',
      issuedAt: 1,
      expiresAt: 2,
      dryRun: false,
    } as unknown as EnvelopeFields
    expect(() => canonicalizeEnvelope(fields)).toThrow(/integers only/)
  })
})

describe('finding 1: dry-run and real apply are separately signed with fresh nonces', () => {
  const req: ComposeActionRequest = {
    action: 'quorum.set_max_auto_members',
    params: { value: 8 },
    targetNode: '12D3KooWTest',
    signerKeyId: 'abcd',
  }

  it('buildDryRun sets dryRun:true, buildRealApply sets dryRun:false', () => {
    const dry = buildDryRun(req, fixtureSign)
    const apply = buildRealApply(req, fixtureSign)
    expect(dry.fields.dryRun).toBe(true)
    expect(apply.fields.dryRun).toBe(false)
  })

  it('the real apply has a DIFFERENT nonce and DIFFERENT signed bytes than the dry-run', () => {
    const dry = buildDryRun(req, fixtureSign)
    const apply = buildRealApply(req, fixtureSign)
    expect(apply.fields.nonce).not.toBe(dry.fields.nonce)
    expect(apply.canonical).not.toBe(dry.canonical)
    expect(apply.signature).not.toBe(dry.signature)
  })

  it('two apply builds never reuse a nonce (fresh 128-bit each time)', () => {
    const seen = new Set<string>()
    for (let i = 0; i < 50; i++) {
      const a = buildRealApply(req, fixtureSign)
      expect(seen.has(a.fields.nonce)).toBe(false)
      expect(a.fields.nonce).toHaveLength(32)
      seen.add(a.fields.nonce)
    }
  })

  it('issuedAt/expiresAt are <= 300s apart', () => {
    const a = buildEnvelopeFields(req, false)
    expect(a.expiresAt - a.issuedAt).toBeLessThanOrEqual(300)
    expect(a.expiresAt).toBeGreaterThan(a.issuedAt)
  })
})

describe('compose validation (pre-sign, mirrors node shape checks)', () => {
  it('requires a targetNode', () => {
    expect(() =>
      buildEnvelopeFields(
        { action: 'quorum.pin_member', params: { peerId: 'P' }, targetNode: '', signerKeyId: 'S' },
        true,
      ),
    ).toThrow(/targetNode/)
  })

  it('requires a peerId for pin/unpin', () => {
    expect(() =>
      buildEnvelopeFields(
        {
          action: 'quorum.pin_member',
          params: { peerId: '' },
          targetNode: 'T',
          signerKeyId: 'S',
        },
        true,
      ),
    ).toThrow(/peerId/)
  })

  it('rejects a max_auto_members value over the ceiling', () => {
    expect(() =>
      buildEnvelopeFields(
        {
          action: 'quorum.set_max_auto_members',
          params: { value: 65 },
          targetNode: 'T',
          signerKeyId: 'S',
        },
        true,
      ),
    ).toThrow(/0\.\.=64/)
  })
})
