import { generateMnemonic, validateMnemonic, mnemonicToSeedSync } from '@scure/bip39'
import { wordlist } from '@scure/bip39/wordlists/english.js'
import { sha256 } from '@noble/hashes/sha2.js'
import { bytesToHex, hexToBytes } from '@noble/hashes/utils.js'

export interface WalletConfig {
  /** Network to connect to */
  network: 'mainnet' | 'testnet'
  /** Storage key prefix */
  storagePrefix: string
}

export const DEFAULT_WALLET_CONFIG: WalletConfig = {
  network: 'mainnet',
  storagePrefix: 'botho-wallet',
}

// ============================================================================
// Encryption utilities (Web Crypto API)
// ============================================================================

const PBKDF2_ITERATIONS = 100_000
const SALT_LENGTH = 16
const IV_LENGTH = 12

/**
 * Derive an AES-GCM key from a password using PBKDF2
 */
async function deriveKey(password: string, salt: Uint8Array): Promise<CryptoKey> {
  const encoder = new TextEncoder()
  const keyMaterial = await crypto.subtle.importKey(
    'raw',
    encoder.encode(password),
    'PBKDF2',
    false,
    ['deriveKey']
  )

  return crypto.subtle.deriveKey(
    {
      name: 'PBKDF2',
      salt: salt as unknown as BufferSource,
      iterations: PBKDF2_ITERATIONS,
      hash: 'SHA-256',
    },
    keyMaterial,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt']
  )
}

/**
 * Encrypt plaintext with a password
 * Returns: salt (16 bytes) + iv (12 bytes) + ciphertext, all hex-encoded
 */
export async function encrypt(plaintext: string, password: string): Promise<string> {
  const encoder = new TextEncoder()
  const salt = crypto.getRandomValues(new Uint8Array(SALT_LENGTH))
  const iv = crypto.getRandomValues(new Uint8Array(IV_LENGTH))
  const key = await deriveKey(password, salt)

  const ciphertext = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv },
    key,
    encoder.encode(plaintext)
  )

  // Concatenate salt + iv + ciphertext
  const result = new Uint8Array(salt.length + iv.length + ciphertext.byteLength)
  result.set(salt, 0)
  result.set(iv, salt.length)
  result.set(new Uint8Array(ciphertext), salt.length + iv.length)

  return bytesToHex(result)
}

/**
 * Decrypt ciphertext with a password
 * Expects: salt (16 bytes) + iv (12 bytes) + ciphertext, all hex-encoded
 */
export async function decrypt(encryptedHex: string, password: string): Promise<string> {
  const data = hexToBytes(encryptedHex)
  const salt = data.slice(0, SALT_LENGTH)
  const iv = data.slice(SALT_LENGTH, SALT_LENGTH + IV_LENGTH)
  const ciphertext = data.slice(SALT_LENGTH + IV_LENGTH)

  const key = await deriveKey(password, salt)

  const plaintext = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv },
    key,
    ciphertext
  )

  return new TextDecoder().decode(plaintext)
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

/**
 * Derive a wallet address from a mnemonic
 * Uses SHA256 of the seed to create a deterministic address
 */
export function deriveAddress(mnemonic: string): string {
  if (!isValidMnemonic(mnemonic)) {
    throw new Error('Invalid mnemonic')
  }

  const seed = mnemonicToSeedSync(mnemonic)
  const hash = sha256(seed)
  // Take first 20 bytes for address (similar to Ethereum)
  return 'bth1' + bytesToHex(hash.slice(0, 20))
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

/**
 * Save wallet to localStorage, optionally encrypted with a password
 */
export async function saveWallet(mnemonic: string, password?: string): Promise<void> {
  const address = deriveAddress(mnemonic)

  if (password) {
    const encryptedMnemonic = await encrypt(mnemonic, password)
    localStorage.setItem(STORAGE_KEY_MNEMONIC, encryptedMnemonic)
    localStorage.setItem(STORAGE_KEY_ENCRYPTED, 'true')
  } else {
    localStorage.setItem(STORAGE_KEY_MNEMONIC, mnemonic)
    localStorage.setItem(STORAGE_KEY_ENCRYPTED, 'false')
  }

  localStorage.setItem(STORAGE_KEY_ADDRESS, address)
}

/**
 * Load wallet from localStorage
 * If encrypted, requires password to decrypt
 */
export async function loadWallet(password?: string): Promise<StoredWallet | null> {
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
    try {
      const mnemonic = await decrypt(storedMnemonic, password)
      return { mnemonic, address }
    } catch {
      throw new Error('Incorrect password')
    }
  }

  return { mnemonic: storedMnemonic, address }
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
