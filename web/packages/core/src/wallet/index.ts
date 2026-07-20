import { generateMnemonic, validateMnemonic } from '@scure/bip39'
import { wordlist } from '@scure/bip39/wordlists/english.js'
import {
  deriveKeypairs,
  deriveDefaultSubaddressPublicKeys,
  formatAddress,
  parseAddress,
  isValidAddress,
  shortenAddress,
  type BothoKeypairs,
  type ParsedAddress,
} from './address'

// Claim-link (bearer payment link) helpers
export {
  CLAIM_LINK_VERSION,
  CLAIM_LINK_ENTROPY_BYTES,
  CLAIM_LINK_MAX_AMOUNT_PICOCREDITS,
  createClaimLinkMnemonic,
  encodeClaimLinkFragment,
  buildClaimLink,
  parseClaimLinkFragment,
  isWithinClaimLinkCap,
  assertClaimLinkAmountWithinCap,
  type ClaimLinkSecret,
} from './claim-link'

// Payment-request (pull payment link) helpers (#470, promoted for the Snap #1108)
export {
  buildPaymentRequestFragment,
  buildPaymentRequestLink,
  parsePaymentRequestFragment,
  type PaymentRequest,
} from './payment-request'

// Re-export address utilities
export {
  deriveKeypairs,
  deriveDefaultSubaddressPublicKeys,
  formatAddress,
  parseAddress,
  isValidAddress,
  shortenAddress,
  type BothoKeypairs,
  type ParsedAddress,
}

export interface WalletConfig {
  /** Network to connect to */
  network: 'mainnet' | 'testnet'
  /** Storage key prefix */
  storagePrefix: string
}

export const DEFAULT_WALLET_CONFIG: WalletConfig = {
  network: 'testnet',
  storagePrefix: 'botho-wallet',
}

// ============================================================================
// Encryption utilities (Web Crypto API)
// ============================================================================
//
// The actual crypto lives in ./vault, a reusable password-derived encryption
// layer (AES-256-GCM + PBKDF2-SHA256, versioned blobs at 600k iterations). The
// wallet seed, claim-link secrets (#474) and address book (#476) all encrypt
// under the same VaultKey. The `encrypt`/`decrypt` helpers below are thin,
// back-compatible wrappers: `encrypt` always writes the NEW versioned format,
// while `decrypt` transparently reads BOTH the new format and LEGACY headerless
// blobs (pre-#475, 100k iterations) so existing wallets keep working.

export {
  VaultKey,
  VAULT_VERSION,
  PBKDF2_ITERATIONS,
  LEGACY_PBKDF2_ITERATIONS,
  isVaultBlob,
  needsRewrap,
  encryptWithPassword,
  decryptWithPassword,
} from './vault'

import {
  VaultKey,
  encryptWithPassword,
  decryptWithPassword,
  needsRewrap,
} from './vault'

/**
 * Encrypt plaintext with a password.
 *
 * Produces a VERSIONED vault blob (magic + version + KDF params + salt + iv +
 * ciphertext, hex-encoded) at the current PBKDF2 iteration count.
 */
export async function encrypt(plaintext: string, password: string): Promise<string> {
  return encryptWithPassword(plaintext, password)
}

/**
 * Decrypt ciphertext with a password.
 *
 * Accepts both the current versioned vault format and the LEGACY headerless
 * format used by pre-#475 wallets (salt|iv|ciphertext at 100k iterations).
 * Throws on wrong password / tampered data.
 */
export async function decrypt(encryptedHex: string, password: string): Promise<string> {
  return decryptWithPassword(encryptedHex, password)
}

/**
 * Generate a new BIP39 mnemonic with 256 bits of entropy (24 words)
 */
export function createMnemonic(): string {
  return generateMnemonic(wordlist, 256)
}

/**
 * Generate a 12-word mnemonic with 128 bits of entropy
 */
export function createMnemonic12(): string {
  return generateMnemonic(wordlist, 128)
}

/**
 * Validate a BIP39 mnemonic phrase
 */
export function isValidMnemonic(mnemonic: string): boolean {
  return validateMnemonic(mnemonic, wordlist)
}

const STORAGE_KEY_MNEMONIC = 'botho-wallet-mnemonic'
const STORAGE_KEY_ADDRESS = 'botho-wallet-address'
const STORAGE_KEY_ENCRYPTED = 'botho-wallet-encrypted'

export interface StoredWallet {
  mnemonic: string
  address: string
}

export interface WalletStorageInfo {
  exists: boolean
  isEncrypted: boolean
  address: string | null
}

/** Minimum allowed password length for NEW wallets (raised from 4 in #475). */
export const MIN_PASSWORD_LENGTH = 8

/** Qualitative password strength buckets for a simple UI hint. */
export type PasswordStrength = 'too-short' | 'weak' | 'fair' | 'strong'

/**
 * Rate a password for a lightweight strength meter. Not a security boundary —
 * just UX guidance. Length is the dominant factor; character-class variety
 * nudges the rating up.
 */
export function passwordStrength(password: string): PasswordStrength {
  if (password.length < MIN_PASSWORD_LENGTH) return 'too-short'
  let classes = 0
  if (/[a-z]/.test(password)) classes++
  if (/[A-Z]/.test(password)) classes++
  if (/[0-9]/.test(password)) classes++
  if (/[^a-zA-Z0-9]/.test(password)) classes++
  if (password.length >= 12 && classes >= 3) return 'strong'
  if (password.length >= 10 || classes >= 3) return 'fair'
  return 'weak'
}

/**
 * Save wallet to localStorage.
 *
 * SECURITY (#475): a password is the default, REQUIRED path — when one is given
 * the seed is written as a versioned, encrypted vault blob (AES-256-GCM,
 * PBKDF2-SHA256 @ 600k). Passing no password is an explicit plaintext opt-out
 * (the UI strongly discourages it); it is preserved only for backward
 * compatibility and migration.
 *
 * `address` is the wallet's v2 (`botho://2/…`) address. It is supplied by the
 * caller rather than derived here because a v2 address requires the account's
 * post-quantum keys, which are derived in `@botho/wasm-signer` (see
 * `deriveV2Address`) — JavaScript never re-implements ML-KEM/ML-DSA keygen. When
 * omitted (e.g. a password change that only re-encrypts the seed) the previously
 * stored address is preserved.
 */
export async function saveWallet(
  mnemonic: string,
  password?: string,
  address?: string,
): Promise<void> {
  if (password) {
    if (password.length < MIN_PASSWORD_LENGTH) {
      throw new Error(`Password must be at least ${MIN_PASSWORD_LENGTH} characters`)
    }
    const encryptedMnemonic = await encrypt(mnemonic, password)
    localStorage.setItem(STORAGE_KEY_MNEMONIC, encryptedMnemonic)
    localStorage.setItem(STORAGE_KEY_ENCRYPTED, 'true')
  } else {
    localStorage.setItem(STORAGE_KEY_MNEMONIC, mnemonic)
    localStorage.setItem(STORAGE_KEY_ENCRYPTED, 'false')
  }

  // Only (re)write the stored address when the caller supplies one. A
  // password-change re-save omits it, preserving the existing v2 address.
  if (address !== undefined) {
    localStorage.setItem(STORAGE_KEY_ADDRESS, address)
  }
}

/**
 * Load wallet from localStorage. If encrypted, requires a password.
 *
 * MIGRATION (#475): when the stored blob is in a legacy format (pre-#475
 * headerless / 100k iterations) it is transparently re-wrapped to the current
 * versioned format on a successful unlock, so the next read is upgraded. The
 * decrypted result is identical either way.
 */
export async function loadWallet(password?: string): Promise<StoredWallet | null> {
  const result = await loadWalletWithKey(password)
  if (!result) return null
  return { mnemonic: result.mnemonic, address: result.address }
}

/** A loaded wallet plus the unlocked {@link VaultKey} for the session. */
export interface UnlockedWallet extends StoredWallet {
  /**
   * The unlocked vault key, present only for encrypted wallets. Hold this in
   * memory for the session so claim-link secrets (#474) and the address book
   * (#476) can be encrypted under the SAME key. Null for plaintext wallets.
   */
  vaultKey: VaultKey | null
}

/**
 * Like {@link loadWallet} but also returns the unlocked {@link VaultKey} so the
 * caller (wallet context) can reuse it to encrypt/decrypt sibling data during
 * the session. Performs the same legacy->current migration on unlock.
 */
export async function loadWalletWithKey(password?: string): Promise<UnlockedWallet | null> {
  const storedMnemonic = localStorage.getItem(STORAGE_KEY_MNEMONIC)
  const address = localStorage.getItem(STORAGE_KEY_ADDRESS)
  const isEncrypted = localStorage.getItem(STORAGE_KEY_ENCRYPTED) === 'true'

  if (!storedMnemonic || !address) {
    return null
  }

  if (isEncrypted) {
    if (!password) {
      throw new Error('Password required to unlock wallet')
    }
    let mnemonic: string
    let vaultKey: VaultKey
    try {
      vaultKey = await VaultKey.fromPasswordAndBlob(password, storedMnemonic)
      mnemonic = await vaultKey.decryptString(storedMnemonic)
    } catch {
      throw new Error('Incorrect password')
    }
    // Migrate a legacy/older blob to the current versioned format so the next
    // unlock is faster/stronger. Re-derive a fresh key (new salt @ 600k) and
    // re-wrap. Best-effort: a storage failure must not block unlock.
    if (needsRewrap(storedMnemonic)) {
      try {
        const freshKey = await VaultKey.fromPassword(password)
        const upgraded = await freshKey.encryptString(mnemonic)
        localStorage.setItem(STORAGE_KEY_MNEMONIC, upgraded)
        localStorage.setItem(STORAGE_KEY_ENCRYPTED, 'true')
        vaultKey = freshKey
      } catch {
        // Keep the old (working) blob; migration will retry next unlock.
      }
    }
    return { mnemonic, address, vaultKey }
  }

  return { mnemonic: storedMnemonic, address, vaultKey: null }
}

/**
 * Get wallet storage info without decrypting
 */
export function getWalletInfo(): WalletStorageInfo {
  const storedMnemonic = localStorage.getItem(STORAGE_KEY_MNEMONIC)
  const address = localStorage.getItem(STORAGE_KEY_ADDRESS)
  const isEncrypted = localStorage.getItem(STORAGE_KEY_ENCRYPTED) === 'true'

  return {
    exists: storedMnemonic !== null,
    isEncrypted,
    address,
  }
}

/**
 * Check if a wallet exists in localStorage
 */
export function hasStoredWallet(): boolean {
  return localStorage.getItem(STORAGE_KEY_MNEMONIC) !== null
}

/**
 * Check if stored wallet is encrypted
 */
export function isWalletEncrypted(): boolean {
  return localStorage.getItem(STORAGE_KEY_ENCRYPTED) === 'true'
}

/**
 * Clear wallet from localStorage
 */
export function clearWallet(): void {
  localStorage.removeItem(STORAGE_KEY_MNEMONIC)
  localStorage.removeItem(STORAGE_KEY_ADDRESS)
  localStorage.removeItem(STORAGE_KEY_ENCRYPTED)
}
