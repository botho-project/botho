// ============================================================================
// Vault: password-derived encryption layer (AES-256-GCM + PBKDF2-SHA256)
// ============================================================================
//
// This module is the FOUNDATION of the wallet's at-rest security. It provides a
// single "vault key" abstraction so that ALL sensitive in-browser data can be
// encrypted under one password-derived key while the wallet is unlocked:
//
//   - the seed mnemonic (this issue, #475)
//   - claim-link bearer secrets (#474)
//   - the address book (#476)
//
// Design goals:
//   - Non-custodial: keys are derived and used in-browser only; nothing is ever
//     sent to a server.
//   - VERSIONED blobs: every encrypted value carries a self-describing header
//     (magic + version + KDF params) so the format can evolve and so legacy
//     blobs can be detected and re-wrapped on next unlock.
//   - Reusable: an unlocked `VaultKey` exposes `encryptString`/`decryptString`
//     and `encryptJSON`/`decryptJSON` that downstream features (#474/#476) call
//     directly. The same `VaultKey` instance can be held in memory by the wallet
//     context for the session.
//   - Sound AES-GCM: a fresh random 16-byte salt and 12-byte IV per encryption.
//
// Blob layout (hex-encoded):
//
//   "bv01" (magic, 4 ASCII bytes) | version (1 byte) | iterations (4 bytes, BE)
//        | salt (16 bytes) | iv (12 bytes) | ciphertext (incl. GCM tag)
//
// A header lets us read the KDF parameters that were used to produce a given
// blob WITHOUT guessing, so wallets written at 100k iterations still decrypt
// while new writes use the current (stronger) parameters.

const SALT_LENGTH = 16
const IV_LENGTH = 12

/** ASCII "bv" + 0x00 0x01 — identifies a versioned vault blob. */
const VAULT_MAGIC = new Uint8Array([0x62, 0x76, 0x00, 0x01]) // "bv\0\1"

/**
 * Current vault blob version. Bump when the on-disk layout or KDF algorithm
 * changes in a way that requires migration. v1 = PBKDF2-SHA256 + AES-256-GCM.
 */
export const VAULT_VERSION = 1

/**
 * Current PBKDF2-SHA256 iteration count for NEW encryptions.
 *
 * 600,000 follows OWASP's 2023 guidance for PBKDF2-SHA256. The iteration count
 * actually used to produce a blob is stored in that blob's header, so older
 * blobs written at a lower count still decrypt; they are transparently upgraded
 * to this value the next time they are re-encrypted (e.g. on unlock + save).
 */
export const PBKDF2_ITERATIONS = 600_000

/**
 * Legacy iteration count used by pre-#475 wallets (no header). Used only to
 * detect/migrate old blobs.
 */
export const LEGACY_PBKDF2_ITERATIONS = 100_000

const HEADER_LENGTH = VAULT_MAGIC.length + 1 /* version */ + 4 /* iterations */

// ---------------------------------------------------------------------------
// hex helpers (no external dependency so the vault is self-contained)
// ---------------------------------------------------------------------------

function bytesToHex(bytes: Uint8Array): string {
  let out = ''
  for (const b of bytes) out += b.toString(16).padStart(2, '0')
  return out
}

function hexToBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) throw new Error('Invalid hex string')
  const out = new Uint8Array(hex.length / 2)
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
  }
  return out
}

// ---------------------------------------------------------------------------
// Key derivation
// ---------------------------------------------------------------------------

/**
 * Derive a non-extractable AES-GCM CryptoKey from a password + salt at a given
 * PBKDF2-SHA256 iteration count.
 */
async function deriveAesKey(
  password: string,
  salt: Uint8Array,
  iterations: number,
): Promise<CryptoKey> {
  const keyMaterial = await crypto.subtle.importKey(
    'raw',
    new TextEncoder().encode(password),
    'PBKDF2',
    false,
    ['deriveKey'],
  )

  return crypto.subtle.deriveKey(
    {
      name: 'PBKDF2',
      salt: salt as unknown as BufferSource,
      iterations,
      hash: 'SHA-256',
    },
    keyMaterial,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  )
}

// ---------------------------------------------------------------------------
// Header (de)serialization
// ---------------------------------------------------------------------------

interface VaultHeader {
  version: number
  iterations: number
}

function writeHeader(version: number, iterations: number): Uint8Array {
  const header = new Uint8Array(HEADER_LENGTH)
  header.set(VAULT_MAGIC, 0)
  header[VAULT_MAGIC.length] = version & 0xff
  const view = new DataView(header.buffer)
  view.setUint32(VAULT_MAGIC.length + 1, iterations, false /* big-endian */)
  return header
}

/**
 * Parse a vault header if present. Returns null when the bytes do not begin with
 * the vault magic (i.e. a legacy headerless blob), so callers can fall back to
 * the legacy format.
 */
function readHeader(bytes: Uint8Array): VaultHeader | null {
  if (bytes.length < HEADER_LENGTH) return null
  for (let i = 0; i < VAULT_MAGIC.length; i++) {
    if (bytes[i] !== VAULT_MAGIC[i]) return null
  }
  const version = bytes[VAULT_MAGIC.length]
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const iterations = view.getUint32(VAULT_MAGIC.length + 1, false)
  return { version, iterations }
}

/**
 * True if the hex blob carries a versioned vault header. A `false` result means
 * the blob is a legacy headerless ciphertext (salt|iv|ct at 100k iterations) and
 * should be migrated on next write.
 */
export function isVaultBlob(encryptedHex: string): boolean {
  try {
    return readHeader(hexToBytes(encryptedHex)) !== null
  } catch {
    return false
  }
}

// ---------------------------------------------------------------------------
// Low-level encrypt/decrypt with explicit password (one-shot)
// ---------------------------------------------------------------------------

/**
 * Encrypt a string under a password, producing a versioned vault blob.
 *
 * Always uses a fresh random salt + IV and the CURRENT KDF parameters.
 */
export async function encryptWithPassword(plaintext: string, password: string): Promise<string> {
  const salt = crypto.getRandomValues(new Uint8Array(SALT_LENGTH))
  const iv = crypto.getRandomValues(new Uint8Array(IV_LENGTH))
  const key = await deriveAesKey(password, salt, PBKDF2_ITERATIONS)

  const ciphertext = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv },
    key,
    new TextEncoder().encode(plaintext),
  )

  const header = writeHeader(VAULT_VERSION, PBKDF2_ITERATIONS)
  const ct = new Uint8Array(ciphertext)
  const out = new Uint8Array(header.length + SALT_LENGTH + IV_LENGTH + ct.length)
  out.set(header, 0)
  out.set(salt, header.length)
  out.set(iv, header.length + SALT_LENGTH)
  out.set(ct, header.length + SALT_LENGTH + IV_LENGTH)

  return bytesToHex(out)
}

/**
 * Decrypt a vault blob (or a LEGACY headerless blob) under a password.
 *
 * - Versioned blobs read their KDF params from the header.
 * - Legacy headerless blobs are decrypted with {@link LEGACY_PBKDF2_ITERATIONS}
 *   (the layout used by pre-#475 wallets: salt|iv|ct).
 *
 * Throws on wrong password / tampered ciphertext (AES-GCM auth failure).
 */
export async function decryptWithPassword(encryptedHex: string, password: string): Promise<string> {
  const data = hexToBytes(encryptedHex)
  const header = readHeader(data)

  let iterations: number
  let body: Uint8Array
  if (header) {
    if (header.version !== VAULT_VERSION) {
      throw new Error(`Unsupported vault version ${header.version}`)
    }
    iterations = header.iterations
    body = data.subarray(HEADER_LENGTH)
  } else {
    // Legacy headerless blob: salt|iv|ct at 100k iterations.
    iterations = LEGACY_PBKDF2_ITERATIONS
    body = data
  }

  const salt = body.subarray(0, SALT_LENGTH)
  const iv = body.subarray(SALT_LENGTH, SALT_LENGTH + IV_LENGTH)
  const ciphertext = body.subarray(SALT_LENGTH + IV_LENGTH)

  const key = await deriveAesKey(password, salt, iterations)
  const plaintext = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv: iv as unknown as BufferSource },
    key,
    ciphertext as unknown as BufferSource,
  )
  return new TextDecoder().decode(plaintext)
}

/**
 * True if a stored blob was written with an OLDER format/KDF than the current
 * one and should be re-wrapped on next save. This is the migration signal: a
 * legacy headerless blob, or a versioned blob whose iteration count is below the
 * current target.
 */
export function needsRewrap(encryptedHex: string): boolean {
  let data: Uint8Array
  try {
    data = hexToBytes(encryptedHex)
  } catch {
    return false
  }
  const header = readHeader(data)
  if (!header) return true // legacy headerless => migrate
  return header.version !== VAULT_VERSION || header.iterations < PBKDF2_ITERATIONS
}

// ---------------------------------------------------------------------------
// VaultKey: a reusable, session-held unlocked key
// ---------------------------------------------------------------------------

/**
 * An unlocked vault key bound to a password + a per-vault salt.
 *
 * Hold one of these in memory (e.g. in the wallet context) for the session so
 * that ALL sensitive data — seed, claim-link secrets (#474), address book
 * (#476) — can be encrypted/decrypted under the same key without re-prompting
 * for the password.
 *
 * Each `encrypt*` call still uses a fresh random IV; the salt is fixed for the
 * lifetime of the key (it identifies the derived key), and is embedded in every
 * blob's header so blobs remain self-describing and independently decryptable
 * with the password alone.
 */
export class VaultKey {
  private constructor(
    private readonly password: string,
    private readonly key: CryptoKey,
    private readonly salt: Uint8Array,
    private readonly iterations: number,
  ) {}

  /**
   * Derive a fresh vault key from a password. Generates a new random salt at the
   * current iteration count. Use this when creating/importing a wallet.
   */
  static async fromPassword(password: string): Promise<VaultKey> {
    const salt = crypto.getRandomValues(new Uint8Array(SALT_LENGTH))
    const iterations = PBKDF2_ITERATIONS
    const key = await deriveAesKey(password, salt, iterations)
    return new VaultKey(password, key, salt, iterations)
  }

  /**
   * Re-derive a vault key from a password and the salt of an EXISTING blob, so
   * the resulting key matches that blob. Use this when unlocking: the wallet's
   * stored blob carries the salt/iterations, so the same key can then decrypt
   * the seed AND any sibling data (claim links / address book) written under it.
   */
  static async fromPasswordAndBlob(password: string, encryptedHex: string): Promise<VaultKey> {
    const data = hexToBytes(encryptedHex)
    const header = readHeader(data)
    const iterations = header ? header.iterations : LEGACY_PBKDF2_ITERATIONS
    const body = header ? data.subarray(HEADER_LENGTH) : data
    const salt = body.subarray(0, SALT_LENGTH)
    // Copy the salt out of the shared buffer so the key is independent of the blob.
    const saltCopy = new Uint8Array(salt)
    const key = await deriveAesKey(password, saltCopy, iterations)
    return new VaultKey(password, key, saltCopy, iterations)
  }

  /**
   * Encrypt a string into a versioned vault blob bound to this key's salt.
   * Always uses a fresh random IV.
   */
  async encryptString(plaintext: string): Promise<string> {
    const iv = crypto.getRandomValues(new Uint8Array(IV_LENGTH))
    const ciphertext = await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv },
      this.key,
      new TextEncoder().encode(plaintext),
    )
    const header = writeHeader(VAULT_VERSION, this.iterations)
    const ct = new Uint8Array(ciphertext)
    const out = new Uint8Array(header.length + SALT_LENGTH + IV_LENGTH + ct.length)
    out.set(header, 0)
    out.set(this.salt, header.length)
    out.set(iv, header.length + SALT_LENGTH)
    out.set(ct, header.length + SALT_LENGTH + IV_LENGTH)
    return bytesToHex(out)
  }

  /**
   * Decrypt a vault blob. If the blob's salt differs from this key's salt (e.g.
   * a legacy seed blob written before the shared-key design), this transparently
   * re-derives the key from the stored password + that blob's salt so unlock
   * still works. New blobs share this key's salt and decrypt directly.
   */
  async decryptString(encryptedHex: string): Promise<string> {
    const data = hexToBytes(encryptedHex)
    const header = readHeader(data)
    const body = header ? data.subarray(HEADER_LENGTH) : data
    const salt = body.subarray(0, SALT_LENGTH)

    let key = this.key
    let sameSalt = salt.length === this.salt.length
    if (sameSalt) {
      for (let i = 0; i < salt.length; i++) {
        if (salt[i] !== this.salt[i]) { sameSalt = false; break }
      }
    }
    if (!sameSalt) {
      const iterations = header ? header.iterations : LEGACY_PBKDF2_ITERATIONS
      key = await deriveAesKey(this.password, new Uint8Array(salt), iterations)
    }

    const iv = body.subarray(SALT_LENGTH, SALT_LENGTH + IV_LENGTH)
    const ciphertext = body.subarray(SALT_LENGTH + IV_LENGTH)
    const plaintext = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: iv as unknown as BufferSource },
      key,
      ciphertext as unknown as BufferSource,
    )
    return new TextDecoder().decode(plaintext)
  }

  /** Encrypt a JSON-serializable value. */
  async encryptJSON(value: unknown): Promise<string> {
    return this.encryptString(JSON.stringify(value))
  }

  /** Decrypt a vault blob produced by {@link encryptJSON}. */
  async decryptJSON<T = unknown>(encryptedHex: string): Promise<T> {
    return JSON.parse(await this.decryptString(encryptedHex)) as T
  }
}
