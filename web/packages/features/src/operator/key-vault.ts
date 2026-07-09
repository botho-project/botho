/**
 * Operator signing-key vault (#751, §2) — imports the operator Ed25519 key into
 * the browser ENCRYPTED under a mandatory passphrase, held in memory for the
 * session only.
 *
 * ## No plaintext-by-default (the #474/#475 lesson)
 *
 * The raw secret is NEVER stored in the clear and NEVER sent to a server (the
 * Pages host serves static assets and never sees the key, §2/§8.3). We reuse
 * the wallet's audited vault (`@botho/core` `VaultKey`: AES-256-GCM + PBKDF2)
 * to wrap the secret at rest, and expose signing through a {@link SessionSigner}
 * that keeps the decrypted secret in a closure for the session and can wipe it.
 *
 * A passphrase is MANDATORY: {@link importOperatorKey} refuses an empty /
 * whitespace-only passphrase, exactly as the node's `OperatorKeyFile::generate`
 * does. There is no unencrypted path.
 *
 * ## What the operator imports
 *
 * The node's `botho operator keygen` writes a JSON key file whose `ciphertext`
 * is the operator's own at-rest form. For the dashboard the operator provides
 * the 32-byte Ed25519 secret scalar as lowercase hex (64 chars) — the same
 * scalar `SigningKey::to_bytes()` produces. We derive the public key (and the
 * `signerKeyId` fingerprint) from it so the composer can address the node.
 */

import { VaultKey } from '@botho/core/wallet'
import { ed25519 } from '@noble/curves/ed25519.js'
import type { SignFn } from './action-envelope'
import { signerKeyIdFromPublicKey } from './action-envelope'

/** Length of the Ed25519 secret scalar in hex characters (32 bytes). */
const SECRET_HEX_LEN = 64

function bytesToHex(bytes: Uint8Array): string {
  let out = ''
  for (const b of bytes) out += b.toString(16).padStart(2, '0')
  return out
}

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.trim().toLowerCase()
  if (clean.length % 2 !== 0) throw new Error('secret key hex has odd length')
  const out = new Uint8Array(clean.length / 2)
  for (let i = 0; i < out.length; i++) {
    const b = parseInt(clean.slice(i * 2, i * 2 + 2), 16)
    if (Number.isNaN(b)) throw new Error('secret key is not valid hex')
    out[i] = b
  }
  return out
}

/**
 * An in-memory, session-held operator signer. The decrypted secret lives only
 * inside this object for the tab's lifetime; {@link wipe} zeroes it. The public
 * key + fingerprint are safe to expose (they identify the signer to the node).
 */
export class SessionSigner {
  private secret: Uint8Array | null

  private constructor(
    secret: Uint8Array,
    readonly publicKey: Uint8Array,
    readonly signerKeyId: string,
    /** The AES-256-GCM + PBKDF2 vault blob (encrypted secret) for re-derive. */
    readonly vaultBlob: string,
  ) {
    this.secret = secret
  }

  /** True while the secret is still held (not wiped). */
  get isUnlocked(): boolean {
    return this.secret !== null
  }

  /** The operator public key as lowercase hex (for display). */
  get publicKeyHex(): string {
    return bytesToHex(this.publicKey)
  }

  /**
   * A {@link SignFn} bound to this signer. Throws if the secret has been wiped.
   * The composer calls this to sign `DOMAIN_SEPARATOR || canonical`; the raw
   * secret never leaves this closure.
   */
  get sign(): SignFn {
    return (message: Uint8Array): Uint8Array => {
      if (this.secret === null) {
        throw new Error('operator signer is locked — re-import the key')
      }
      return ed25519.sign(message, this.secret)
    }
  }

  /**
   * Zero and drop the in-memory secret. After this the signer is locked and
   * signing throws. Call on sign-out / tab-hide. The vault blob remains so the
   * operator can re-derive with their passphrase if desired.
   */
  wipe(): void {
    if (this.secret) {
      this.secret.fill(0)
      this.secret = null
    }
  }

  /** @internal — build a signer from a decrypted secret. */
  static async fromSecret(secret: Uint8Array, vaultBlob: string): Promise<SessionSigner> {
    const publicKey = ed25519.getPublicKey(secret)
    const signerKeyId = signerKeyIdFromPublicKey(publicKey)
    return new SessionSigner(secret, publicKey, signerKeyId, vaultBlob)
  }
}

/**
 * Import an operator Ed25519 secret key (32-byte scalar, lowercase hex) under a
 * MANDATORY passphrase. The secret is wrapped in a `VaultKey` blob (AES-256-GCM
 * + PBKDF2, reusing `@botho/core`) and a {@link SessionSigner} holding the
 * decrypted secret is returned for the session.
 *
 * Refuses:
 *   - an empty / whitespace-only passphrase (no plaintext-by-default, §2),
 *   - a secret that is not exactly 64 lowercase hex chars.
 */
export async function importOperatorKey(
  secretKeyHex: string,
  passphrase: string,
): Promise<SessionSigner> {
  if (passphrase.trim() === '') {
    throw new Error(
      'a passphrase is required to encrypt the operator key in the browser — ' +
        'refusing to hold an unencrypted key (empty passphrase rejected)',
    )
  }
  const clean = secretKeyHex.trim().toLowerCase()
  if (clean.length !== SECRET_HEX_LEN || !/^[0-9a-f]+$/.test(clean)) {
    throw new Error(
      `operator secret key must be ${SECRET_HEX_LEN} lowercase hex characters ` +
        '(the 32-byte Ed25519 secret scalar)',
    )
  }
  const secret = hexToBytes(clean)
  // Validate it is a usable Ed25519 secret by deriving the public key.
  try {
    ed25519.getPublicKey(secret)
  } catch {
    throw new Error('the provided bytes are not a valid Ed25519 secret key')
  }

  // Wrap the secret at rest under the passphrase (AES-256-GCM + PBKDF2). The
  // resulting blob could be persisted to sessionStorage by the caller if they
  // choose; the raw secret is never persisted in the clear.
  const vaultKey = await VaultKey.fromPassword(passphrase)
  const vaultBlob = await vaultKey.encryptString(clean)

  return SessionSigner.fromSecret(secret, vaultBlob)
}

/**
 * Re-derive a session signer from a previously-produced vault blob + passphrase
 * (e.g. after a page reload where the operator kept the encrypted blob in
 * sessionStorage). Throws on wrong passphrase (AES-GCM auth failure).
 */
export async function unlockOperatorKey(
  vaultBlob: string,
  passphrase: string,
): Promise<SessionSigner> {
  if (passphrase.trim() === '') {
    throw new Error('a passphrase is required to unlock the operator key')
  }
  const vaultKey = await VaultKey.fromPasswordAndBlob(passphrase, vaultBlob)
  const secretHex = await vaultKey.decryptString(vaultBlob)
  const secret = hexToBytes(secretHex)
  return SessionSigner.fromSecret(secret, vaultBlob)
}
