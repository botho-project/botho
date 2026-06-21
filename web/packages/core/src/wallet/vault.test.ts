import { describe, it, expect } from 'vitest'
import {
  VaultKey,
  VAULT_VERSION,
  PBKDF2_ITERATIONS,
  LEGACY_PBKDF2_ITERATIONS,
  isVaultBlob,
  needsRewrap,
  encryptWithPassword,
  decryptWithPassword,
} from './vault'

/**
 * Reproduce the LEGACY (pre-#475) headerless blob format: PBKDF2-SHA256 @ 100k
 * iterations, layout salt(16)|iv(12)|ciphertext, hex-encoded — NO version
 * header. Used to assert the migration/back-compat path.
 */
async function legacyEncrypt(plaintext: string, password: string): Promise<string> {
  const salt = crypto.getRandomValues(new Uint8Array(16))
  const iv = crypto.getRandomValues(new Uint8Array(12))
  const keyMaterial = await crypto.subtle.importKey(
    'raw',
    new TextEncoder().encode(password),
    'PBKDF2',
    false,
    ['deriveKey'],
  )
  const key = await crypto.subtle.deriveKey(
    { name: 'PBKDF2', salt, iterations: LEGACY_PBKDF2_ITERATIONS, hash: 'SHA-256' },
    keyMaterial,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  )
  const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode(plaintext))
  const ctBytes = new Uint8Array(ct)
  const out = new Uint8Array(16 + 12 + ctBytes.length)
  out.set(salt, 0)
  out.set(iv, 16)
  out.set(ctBytes, 28)
  return Array.from(out).map((b) => b.toString(16).padStart(2, '0')).join('')
}

const SECRET = 'gravity gravity gravity gravity orbit orbit'
const PASSWORD = 'correct horse battery'

describe('vault: versioned KDF header', () => {
  it('current encryptWithPassword produces a versioned vault blob', async () => {
    const blob = await encryptWithPassword(SECRET, PASSWORD)
    expect(isVaultBlob(blob)).toBe(true)
  })

  it('header round-trips: version byte and iteration count are recorded', async () => {
    const blob = await encryptWithPassword(SECRET, PASSWORD)
    const bytes = Uint8Array.from(blob.match(/.{2}/g)!.map((h) => parseInt(h, 16)))
    // magic "bv\0\1"
    expect(bytes[0]).toBe(0x62)
    expect(bytes[1]).toBe(0x76)
    // version
    expect(bytes[4]).toBe(VAULT_VERSION)
    // iterations (big-endian u32 at offset 5)
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    expect(view.getUint32(5, false)).toBe(PBKDF2_ITERATIONS)
  })

  it('round-trips a value through encrypt/decrypt with password', async () => {
    const blob = await encryptWithPassword(SECRET, PASSWORD)
    expect(await decryptWithPassword(blob, PASSWORD)).toBe(SECRET)
  })

  it('uses a fresh random salt+iv per encryption (non-deterministic ciphertext)', async () => {
    const a = await encryptWithPassword(SECRET, PASSWORD)
    const b = await encryptWithPassword(SECRET, PASSWORD)
    expect(a).not.toBe(b)
  })

  it('rejects the wrong password', async () => {
    const blob = await encryptWithPassword(SECRET, PASSWORD)
    await expect(decryptWithPassword(blob, 'wrong password')).rejects.toThrow()
  })
})

describe('vault: legacy migration / back-compat', () => {
  it('detects a legacy headerless blob as NOT a vault blob', async () => {
    const legacy = await legacyEncrypt(SECRET, PASSWORD)
    expect(isVaultBlob(legacy)).toBe(false)
  })

  it('decrypts a legacy 100k-iter blob with the right password', async () => {
    const legacy = await legacyEncrypt(SECRET, PASSWORD)
    expect(await decryptWithPassword(legacy, PASSWORD)).toBe(SECRET)
  })

  it('flags legacy blobs as needing re-wrap, current blobs as not', async () => {
    const legacy = await legacyEncrypt(SECRET, PASSWORD)
    const current = await encryptWithPassword(SECRET, PASSWORD)
    expect(needsRewrap(legacy)).toBe(true)
    expect(needsRewrap(current)).toBe(false)
  })
})

describe('VaultKey: reusable session key (for #474/#476)', () => {
  it('encryptString/decryptString round-trip under one key', async () => {
    const key = await VaultKey.fromPassword(PASSWORD)
    const blob = await key.encryptString('claim-link-secret')
    expect(isVaultBlob(blob)).toBe(true)
    expect(await key.decryptString(blob)).toBe('claim-link-secret')
  })

  it('encryptJSON/decryptJSON round-trip structured data', async () => {
    const key = await VaultKey.fromPassword(PASSWORD)
    const value = { contacts: [{ name: 'Ada', address: 'tbotho://1/abc' }], n: 2 }
    const blob = await key.encryptJSON(value)
    expect(await key.decryptJSON(blob)).toEqual(value)
  })

  it('uses a fresh IV per encryptString call', async () => {
    const key = await VaultKey.fromPassword(PASSWORD)
    const a = await key.encryptString('x')
    const b = await key.encryptString('x')
    expect(a).not.toBe(b)
  })

  it('fromPasswordAndBlob re-derives a key that decrypts that blob', async () => {
    const seedBlob = await encryptWithPassword(SECRET, PASSWORD)
    const key = await VaultKey.fromPasswordAndBlob(PASSWORD, seedBlob)
    // The session key can decrypt the seed blob...
    expect(await key.decryptString(seedBlob)).toBe(SECRET)
    // ...and encrypt sibling data the same key can read back.
    const sibling = await key.encryptString('address-book')
    expect(await key.decryptString(sibling)).toBe('address-book')
  })

  it('a key derived from one blob can still decrypt a sibling blob with a different salt', async () => {
    // Seed blob (salt A) + an independently-encrypted sibling blob (salt B).
    const seedBlob = await encryptWithPassword(SECRET, PASSWORD)
    const siblingBlob = await encryptWithPassword('other-data', PASSWORD)
    const key = await VaultKey.fromPasswordAndBlob(PASSWORD, seedBlob)
    expect(await key.decryptString(siblingBlob)).toBe('other-data')
  })

  it('rejects a blob encrypted under a different password', async () => {
    const key = await VaultKey.fromPassword(PASSWORD)
    const foreign = await encryptWithPassword('secret', 'a-different-password')
    await expect(key.decryptString(foreign)).rejects.toThrow()
  })
})
