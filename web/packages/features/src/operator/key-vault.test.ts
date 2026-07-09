import { describe, expect, it } from 'vitest'
import { ed25519 } from '@noble/curves/ed25519.js'
import { importOperatorKey, unlockOperatorKey } from './key-vault'
import { signingMessage, verifyEnvelopeSignature } from './action-envelope'
import fixtures from './fixtures/operator-action-fixtures.json'

const SEED_HEX = fixtures.signingKeySeed

function bytesToHex(b: Uint8Array): string {
  return [...b].map((x) => x.toString(16).padStart(2, '0')).join('')
}

describe('operator key vault (§2, #474/#475 no-plaintext-by-default)', () => {
  it('imports a valid Ed25519 secret under a passphrase and derives the node signerKeyId', async () => {
    const signer = await importOperatorKey(SEED_HEX, 'correct horse battery staple')
    expect(signer.signerKeyId).toBe(fixtures.signerKeyId)
    expect(signer.publicKeyHex).toBe(fixtures.publicKeyHex)
    expect(signer.isUnlocked).toBe(true)
  })

  it('REFUSES an empty passphrase (no plaintext-by-default)', async () => {
    await expect(importOperatorKey(SEED_HEX, '')).rejects.toThrow(/passphrase is required/)
    await expect(importOperatorKey(SEED_HEX, '   ')).rejects.toThrow(/passphrase is required/)
  })

  it('REFUSES a malformed secret key', async () => {
    await expect(importOperatorKey('not-hex', 'pw')).rejects.toThrow(/hex/)
    await expect(importOperatorKey('ab', 'pw')).rejects.toThrow(/64 lowercase hex/)
  })

  it('the vault blob is encrypted at rest — the raw secret does NOT appear in it', async () => {
    const signer = await importOperatorKey(SEED_HEX, 'pw-123')
    expect(signer.vaultBlob).not.toContain(SEED_HEX)
    // Blob is a versioned vault blob (starts with the "bv" magic in hex: 6276).
    expect(signer.vaultBlob.startsWith('6276')).toBe(true)
  })

  it('the session signer signs bytes the node would accept', async () => {
    const signer = await importOperatorKey(SEED_HEX, 'pw')
    const canonical = fixtures.envelopes[0].canonical
    const sig = signer.sign(signingMessage(canonical))
    const pk = ed25519.getPublicKey(new Uint8Array([...SEED_HEX.matchAll(/../g)].map((m) => parseInt(m[0], 16))))
    expect(verifyEnvelopeSignature(canonical, bytesToHex(sig), pk)).toBe(true)
    // And it matches the committed node fixture signature exactly.
    expect(bytesToHex(sig)).toBe(fixtures.envelopes[0].signature)
  })

  it('wipe() locks the signer so it can no longer sign', async () => {
    const signer = await importOperatorKey(SEED_HEX, 'pw')
    signer.wipe()
    expect(signer.isUnlocked).toBe(false)
    expect(() => signer.sign(new Uint8Array([1, 2, 3]))).toThrow(/locked/)
  })

  it('unlockOperatorKey re-derives the signer from the vault blob + passphrase', async () => {
    const first = await importOperatorKey(SEED_HEX, 'pw-abc')
    const reopened = await unlockOperatorKey(first.vaultBlob, 'pw-abc')
    expect(reopened.signerKeyId).toBe(first.signerKeyId)
    expect(reopened.publicKeyHex).toBe(first.publicKeyHex)
  })

  it('unlockOperatorKey rejects the wrong passphrase (AES-GCM auth failure)', async () => {
    const first = await importOperatorKey(SEED_HEX, 'right-pw')
    await expect(unlockOperatorKey(first.vaultBlob, 'wrong-pw')).rejects.toThrow()
  })
})
