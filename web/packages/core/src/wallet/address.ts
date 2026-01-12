/**
 * Botho Address Derivation
 *
 * Derives proper Botho addresses from BIP39 mnemonics following the protocol:
 *
 * 1. Mnemonic → BIP39 seed (64 bytes, empty passphrase)
 * 2. Seed → SLIP-10 Ed25519 derivation at m/44'/866'/0'
 * 3. SLIP-10 key → HKDF-SHA512 → view and spend Ristretto255 keys
 * 4. Public keys → base58 encoding → tbotho://1/<base58>
 */

import { mnemonicToSeedSync } from '@scure/bip39'
import { HDKey } from '@scure/bip32'
import { hkdf } from '@noble/hashes/hkdf.js'
import { sha512 } from '@noble/hashes/sha2.js'
import { base58 } from '@scure/base'
import { ristretto255 } from '@noble/curves/ed25519'

const encoder = new TextEncoder()

// BIP44 constants
const BIP44_PURPOSE = 44
const BOTHO_COIN_TYPE = 866

// Domain separators for key derivation (must match Rust implementation)
const VIEW_DOMAIN = 'botho-ristretto255-view'
const SPEND_DOMAIN = 'botho-ristretto255-spend'

// Address prefixes
const TESTNET_PREFIX = 'tbotho://1/'
const MAINNET_PREFIX = 'botho://1/'

/**
 * Derive SLIP-10 Ed25519 key from BIP39 seed
 *
 * Uses path m/44'/866'/account' for Botho
 */
function deriveSlip10Key(seed: Uint8Array, accountIndex: number = 0): Uint8Array {
  // SLIP-10 uses Ed25519 derivation from BIP32
  // We use @scure/bip32 which supports Ed25519 via HDKey
  const masterKey = HDKey.fromMasterSeed(seed)

  // Derive using hardened path: m/44'/866'/account'
  const path = `m/${BIP44_PURPOSE}'/${BOTHO_COIN_TYPE}'/${accountIndex}'`
  const derived = masterKey.derive(path)

  if (!derived.privateKey) {
    throw new Error('Failed to derive private key')
  }

  return derived.privateKey
}

/**
 * Derive Ristretto255 private key using HKDF-SHA512
 *
 * This matches the Rust implementation:
 * - HKDF with domain-separated salt
 * - 64-byte output
 * - Converted to scalar via from_bytes_mod_order_wide
 */
function deriveRistrettoPrivate(slip10Key: Uint8Array, domain: string): Uint8Array {
  // HKDF-SHA512 with domain as salt, slip10Key as IKM
  // Output 64 bytes for wide scalar reduction
  const salt = encoder.encode(domain)
  const okm = hkdf(sha512, slip10Key, salt, undefined, 64)
  return okm
}

// Ed25519 curve order L = 2^252 + 27742317777372353535851937790883648493
const CURVE_ORDER = BigInt('7237005577332262213973186563042994240857116359379907606001950938285454250989')

/**
 * Convert bytes to BigInt (little-endian)
 */
function bytesToBigInt(bytes: Uint8Array): bigint {
  let result = BigInt(0)
  for (let i = bytes.length - 1; i >= 0; i--) {
    result = (result << BigInt(8)) | BigInt(bytes[i])
  }
  return result
}

/**
 * Convert BigInt to 32-byte Uint8Array (little-endian)
 */
function bigIntToBytes(n: bigint): Uint8Array {
  const bytes = new Uint8Array(32)
  let temp = n
  for (let i = 0; i < 32; i++) {
    bytes[i] = Number(temp & BigInt(0xff))
    temp >>= BigInt(8)
  }
  return bytes
}

/**
 * Convert 64-byte wide scalar to Ristretto255 scalar (mod L)
 *
 * This matches Rust's Scalar::from_bytes_mod_order_wide
 * The ed25519 curve order L = 2^252 + 27742317777372353535851937790883648493
 */
function scalarFromWide(wide: Uint8Array): Uint8Array {
  if (wide.length !== 64) {
    throw new Error('Wide scalar must be 64 bytes')
  }

  // Convert 64 bytes to BigInt (little-endian, matching Rust)
  const wideInt = bytesToBigInt(wide)

  // Reduce mod the curve order L
  const reduced = wideInt % CURVE_ORDER

  // Convert back to 32 bytes (little-endian)
  return bigIntToBytes(reduced)
}

/**
 * Derive Ristretto255 public key from private scalar
 *
 * This is scalar multiplication: public = scalar * G
 * where G is the Ristretto255 base point
 *
 * Uses ristretto255 encoding (NOT Ed25519) to match Rust's RistrettoPublic
 */
function derivePublicKey(privateScalar: Uint8Array): Uint8Array {
  // Scalar multiply with the base point using ristretto255
  // ristretto255.Point.BASE is the ristretto255 generator
  const scalar = bytesToBigInt(privateScalar)
  const publicPoint = ristretto255.Point.BASE.multiply(scalar)
  // Encode as ristretto255 compressed point (32 bytes)
  return publicPoint.toBytes()
}

/**
 * Derive view and spend keypairs from a mnemonic
 */
export interface BothoKeypairs {
  viewPrivate: Uint8Array
  viewPublic: Uint8Array
  spendPrivate: Uint8Array
  spendPublic: Uint8Array
}

export function deriveKeypairs(mnemonic: string, accountIndex: number = 0): BothoKeypairs {
  // 1. Mnemonic → BIP39 seed (64 bytes, empty passphrase)
  const seed = mnemonicToSeedSync(mnemonic, '')

  // 2. Seed → SLIP-10 key at m/44'/866'/account'
  const slip10Key = deriveSlip10Key(seed, accountIndex)

  // 3. SLIP-10 key → HKDF-SHA512 → view and spend private keys
  const viewWide = deriveRistrettoPrivate(slip10Key, VIEW_DOMAIN)
  const spendWide = deriveRistrettoPrivate(slip10Key, SPEND_DOMAIN)

  // 4. Wide scalars → reduced scalars (mod L)
  const viewPrivate = scalarFromWide(viewWide)
  const spendPrivate = scalarFromWide(spendWide)

  // 5. Private keys → public keys
  const viewPublic = derivePublicKey(viewPrivate)
  const spendPublic = derivePublicKey(spendPrivate)

  return {
    viewPrivate,
    viewPublic,
    spendPrivate,
    spendPublic,
  }
}

/**
 * Format a Botho address from view and spend public keys
 *
 * Classical address format: tbotho://1/<base58(view || spend)>
 */
export function formatAddress(
  viewPublic: Uint8Array,
  spendPublic: Uint8Array,
  network: 'mainnet' | 'testnet' = 'testnet'
): string {
  // Concatenate view (32 bytes) || spend (32 bytes)
  const combined = new Uint8Array(64)
  combined.set(viewPublic, 0)
  combined.set(spendPublic, 32)

  // Encode as base58
  const encoded = base58.encode(combined)

  // Add network prefix
  const prefix = network === 'testnet' ? TESTNET_PREFIX : MAINNET_PREFIX
  return prefix + encoded
}

/**
 * Derive a complete Botho address from a mnemonic
 */
export function deriveAddressFromMnemonic(
  mnemonic: string,
  network: 'mainnet' | 'testnet' = 'testnet',
  accountIndex: number = 0
): string {
  const keypairs = deriveKeypairs(mnemonic, accountIndex)
  return formatAddress(keypairs.viewPublic, keypairs.spendPublic, network)
}

/**
 * Parse a Botho address string into its components
 */
export interface ParsedAddress {
  network: 'mainnet' | 'testnet'
  viewPublic: Uint8Array
  spendPublic: Uint8Array
}

export function parseAddress(address: string): ParsedAddress {
  const trimmed = address.trim()

  let network: 'mainnet' | 'testnet'
  let encoded: string

  if (trimmed.startsWith(TESTNET_PREFIX)) {
    network = 'testnet'
    encoded = trimmed.slice(TESTNET_PREFIX.length)
  } else if (trimmed.startsWith(MAINNET_PREFIX)) {
    network = 'mainnet'
    encoded = trimmed.slice(MAINNET_PREFIX.length)
  } else {
    throw new Error('Invalid address format: missing tbotho:// or botho:// prefix')
  }

  // Decode base58
  const decoded = base58.decode(encoded)

  if (decoded.length !== 64) {
    throw new Error(`Invalid address: expected 64 bytes, got ${decoded.length}`)
  }

  return {
    network,
    viewPublic: decoded.slice(0, 32),
    spendPublic: decoded.slice(32, 64),
  }
}

/**
 * Validate a Botho address format
 */
export function isValidAddress(address: string): boolean {
  try {
    parseAddress(address)
    return true
  } catch {
    return false
  }
}

/**
 * Get a shortened display version of an address
 */
export function shortenAddress(address: string, prefixLen: number = 12, suffixLen: number = 8): string {
  if (address.length <= prefixLen + suffixLen + 3) {
    return address
  }
  return address.slice(0, prefixLen) + '...' + address.slice(-suffixLen)
}
