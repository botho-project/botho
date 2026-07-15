/**
 * Botho Address Derivation (address format v2, ADR 0008)
 *
 * Derives proper Botho addresses from BIP39 mnemonics following the protocol:
 *
 * 1. Mnemonic → BIP39 seed (64 bytes, empty passphrase)
 * 2. Seed → SLIP-10 Ed25519 derivation at m/44'/866'/0'
 * 3. SLIP-10 key → HKDF-SHA512 → view and spend Ristretto255 keys
 * 4. Default-subaddress (index 0) view/spend keys + the account's ML-KEM-768 /
 *    ML-DSA-65 public keys → base58 → `botho://2/<base58>` / `tbotho://2/…`.
 *
 * This module owns the **classical** half (SLIP-10 view/spend derivation,
 * pinned byte-identical to the node by `derivation-parity.test.ts`) and the v2
 * address-string **codec** (`formatAddress` / `parseAddress`), whose base58 body
 * layout `view(32)‖spend(32)‖kem(1184)‖dsa(1952)` is byte-identical to the
 * shared Rust codec (`address-codec`) — guarded by `address-v2.test.ts` against
 * the golden the node emits.
 *
 * The **post-quantum** half (deriving the ML-KEM / ML-DSA public keys from the
 * seed) and the canonical address *production* live in `@botho/wasm-signer`
 * (`deriveV2Address`), which reuses the node's exact `derive_pq_keys_from_seed`
 * and the shared codec via wasm — JavaScript never re-implements ML-KEM/ML-DSA
 * key generation. Because a v2 address requires those PQ keys, this module no
 * longer produces a full address from a mnemonic on its own.
 */

import { mnemonicToSeedSync } from '@scure/bip39'
import { hkdf } from '@noble/hashes/hkdf.js'
import { hmac } from '@noble/hashes/hmac.js'
import { sha512 } from '@noble/hashes/sha2.js'
import { blake2b } from '@noble/hashes/blake2.js'
import { base58 } from '@scure/base'
import { ristretto255 } from '@noble/curves/ed25519'

const encoder = new TextEncoder()

// BIP44 constants
const BIP44_PURPOSE = 44
const BOTHO_COIN_TYPE = 866

// Domain separators for key derivation (must match Rust implementation)
const VIEW_DOMAIN = 'botho-ristretto255-view'
const SPEND_DOMAIN = 'botho-ristretto255-spend'

// Subaddress derivation domain tag (must match the node's
// `SUBADDRESS_DOMAIN_TAG` in `core/src/consts.rs`).
const SUBADDRESS_DOMAIN_TAG = 'bth_subaddress'

// The default subaddress index. Outputs paid to a recipient are addressed to
// their default subaddress (index 0); the node's `TxOutput::belongs_to`
// (`transaction/clsag/src/lib.rs`) only recognizes outputs paid to the default
// (0) or change subaddress, NOT the account-root keys.
const DEFAULT_SUBADDRESS_INDEX = 0

// Address prefixes (address format v2, ADR 0008). The version bump from `1` to
// `2` makes old 64-byte classical-only addresses fail loudly on parse rather
// than silently mis-decode.
const TESTNET_PREFIX = 'tbotho://2/'
const MAINNET_PREFIX = 'botho://2/'

// Retired prefixes, kept only so `parseAddress` can reject them with a clear
// error instead of a confusing format failure.
const TESTNET_V1_PREFIX = 'tbotho://1/'
const MAINNET_V1_PREFIX = 'botho://1/'
const MAINNET_QUANTUM_PREFIX = 'botho://1q/'
const TESTNET_QUANTUM_PREFIX = 'tbotho://1q/'
const LEGACY_QUANTUM_PREFIX = 'botho-pq://1/'

// Raw post-quantum public-key byte lengths (must match the node:
// `ML_KEM_768_PUBLIC_KEY_LEN` / `ML_DSA_65_PUBLIC_KEY_LEN`).
const ML_KEM_768_PUBLIC_KEY_LEN = 1184
const ML_DSA_65_PUBLIC_KEY_LEN = 1952

// Byte offsets within the decoded v2 body: view(32)‖spend(32)‖kem(1184)‖dsa(1952).
const VIEW_LEN = 32
const SPEND_LEN = 32
const V2_BODY_LEN = VIEW_LEN + SPEND_LEN + ML_KEM_768_PUBLIC_KEY_LEN + ML_DSA_65_PUBLIC_KEY_LEN // 3200

/**
 * SLIP-0010 master key for the Ed25519 curve.
 *
 * `I = HMAC-SHA512("ed25519 seed", seed)`; left 32 bytes are the key, right 32
 * bytes the chain code. (See SLIP-0010 §"Master key generation".)
 */
function slip10Ed25519Master(seed: Uint8Array): { key: Uint8Array; chainCode: Uint8Array } {
  const I = hmac(sha512, encoder.encode('ed25519 seed'), seed)
  return { key: I.slice(0, 32), chainCode: I.slice(32, 64) }
}

/** Serialize a 32-bit unsigned integer as 4 big-endian bytes. */
function ser32(index: number): Uint8Array {
  const out = new Uint8Array(4)
  out[0] = (index >>> 24) & 0xff
  out[1] = (index >>> 16) & 0xff
  out[2] = (index >>> 8) & 0xff
  out[3] = index & 0xff
  return out
}

/**
 * SLIP-0010 Ed25519 hardened child derivation.
 *
 * Ed25519 only supports hardened derivation: the high bit of the index is
 * always set, and the data hashed is `0x00 || key || ser32(index')`.
 * (See SLIP-0010 §"Private parent key -> private child key".)
 */
function slip10Ed25519Child(
  parent: { key: Uint8Array; chainCode: Uint8Array },
  index: number,
): { key: Uint8Array; chainCode: Uint8Array } {
  // Force hardened (>>> 0 keeps it an unsigned 32-bit value).
  const hardened = (index | 0x80000000) >>> 0
  const data = new Uint8Array(1 + 32 + 4)
  data[0] = 0x00
  data.set(parent.key, 1)
  data.set(ser32(hardened), 33)
  const I = hmac(sha512, parent.chainCode, data)
  return { key: I.slice(0, 32), chainCode: I.slice(32, 64) }
}

/**
 * Derive the SLIP-0010 Ed25519 private key from a BIP39 seed at the Botho
 * wallet path `m/44'/866'/account'`.
 *
 * This MUST byte-match the Rust node's derivation
 * (`slip10_ed25519::derive_ed25519_private_key` in `core/src/slip10/mod.rs`),
 * otherwise the keys this wallet signs with will differ from the node's view of
 * the account and any transaction it builds will be rejected. The parity is
 * pinned by `derivation-parity.test.ts` against the same vectors the Rust unit
 * tests assert.
 *
 * NOTE: an earlier implementation used `@scure/bip32`'s `HDKey`, which performs
 * BIP-32 *secp256k1* derivation, not SLIP-0010 *Ed25519* derivation, and so
 * produced keys incompatible with the node. This hand-rolled SLIP-0010 Ed25519
 * path (HMAC-SHA512 with the "ed25519 seed" key, hardened-only children)
 * matches the spec and the node.
 */
function deriveSlip10Key(seed: Uint8Array, accountIndex: number = 0): Uint8Array {
  let node = slip10Ed25519Master(seed)
  for (const component of [BIP44_PURPOSE, BOTHO_COIN_TYPE, accountIndex]) {
    node = slip10Ed25519Child(node, component)
  }
  return node.key
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
 * Reduce a scalar (given as a BigInt) modulo the curve order L.
 *
 * Matches Rust scalar arithmetic, where every `Scalar` operation is implicitly
 * reduced mod L.
 */
function modL(n: bigint): bigint {
  const r = n % CURVE_ORDER
  return r < 0n ? r + CURVE_ORDER : r
}

/**
 * Derive the DEFAULT-SUBADDRESS (index 0) private keys from the account-root
 * view/spend private scalars.
 *
 * This MUST byte-match the node's subaddress derivation in
 * `core/src/subaddress.rs` (the `(&RootViewPrivate, &RootSpendPrivate)`
 * implementation), which for index `n` computes:
 *
 *   a  = view_private (scalar)
 *   b  = spend_private (scalar)
 *   Hs = Scalar::from_hash(Blake2b512("bth_subaddress" || a.as_bytes() || n.as_bytes()))
 *   subaddress_spend_private = Hs + b
 *   subaddress_view_private  = a * (Hs + b)
 *
 * `Scalar::from_hash` reduces the 64-byte Blake2b512 output via
 * `from_bytes_mod_order_wide` (little-endian), and `a.as_bytes()` /
 * `n.as_bytes()` are the canonical 32-byte little-endian scalar encodings.
 *
 * The node addresses outputs to a recipient's default subaddress and scans for
 * ownership against the default/change subaddress (`TxOutput::belongs_to`), so
 * the address the wallet displays must pack THESE keys — not the account-root
 * keys — or funds sent to the displayed address are undetectable by the
 * recipient's scan.
 */
function deriveSubaddressPrivateScalars(
  viewPrivate: Uint8Array,
  spendPrivate: Uint8Array,
  index: number = DEFAULT_SUBADDRESS_INDEX,
): { viewSubPrivate: Uint8Array; spendSubPrivate: Uint8Array } {
  const a = bytesToBigInt(viewPrivate)
  const b = bytesToBigInt(spendPrivate)

  // `n = Scalar::from(index)` -> canonical 32-byte little-endian encoding.
  const nBytes = bigIntToBytes(BigInt(index))

  // Hs = from_bytes_mod_order_wide(Blake2b512(tag || a.as_bytes() || n.as_bytes()))
  const tag = encoder.encode(SUBADDRESS_DOMAIN_TAG)
  const digestInput = new Uint8Array(tag.length + 32 + 32)
  digestInput.set(tag, 0)
  digestInput.set(viewPrivate, tag.length) // a.as_bytes() (32-byte LE)
  digestInput.set(nBytes, tag.length + 32) // n.as_bytes() (32-byte LE)
  const wide = blake2b(digestInput, { dkLen: 64 })
  const Hs = bytesToBigInt(scalarFromWide(wide))

  // subaddress spend private = Hs + b; view private = a * (Hs + b)
  const spendSub = modL(Hs + b)
  const viewSub = modL(a * spendSub)

  return {
    viewSubPrivate: bigIntToBytes(viewSub),
    spendSubPrivate: bigIntToBytes(spendSub),
  }
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
 * Derive the account's DEFAULT-SUBADDRESS (index 0) public keys from a mnemonic.
 *
 * These are the keys a recipient's address must advertise: the node addresses
 * outputs to the default subaddress and scans for ownership against it
 * (`TxOutput::belongs_to`). The account-root public keys (`deriveKeypairs`'
 * `viewPublic`/`spendPublic`) are used for signing, NOT for receiving.
 */
export function deriveDefaultSubaddressPublicKeys(
  mnemonic: string,
  accountIndex: number = 0,
): { viewPublic: Uint8Array; spendPublic: Uint8Array } {
  const kp = deriveKeypairs(mnemonic, accountIndex)
  const { viewSubPrivate, spendSubPrivate } = deriveSubaddressPrivateScalars(
    kp.viewPrivate,
    kp.spendPrivate,
    DEFAULT_SUBADDRESS_INDEX,
  )
  return {
    viewPublic: derivePublicKey(viewSubPrivate),
    spendPublic: derivePublicKey(spendSubPrivate),
  }
}

/**
 * Format a v2 Botho address string from its four public-key components.
 *
 * Layout (address format v2, ADR 0008), base58 of the fixed concatenation:
 *
 *   view(32) ‖ spend(32) ‖ kem(1184) ‖ dsa(1952)   = 3200 bytes
 *
 * → `botho://2/<base58>` (mainnet) or `tbotho://2/<base58>` (testnet). This is
 * byte-identical to the shared Rust codec (`address-codec::encode_address`);
 * the equality is guarded by `address-v2.test.ts` against the node's golden.
 *
 * The ML-KEM-768 / ML-DSA-65 public keys are NOT derived here — they come from
 * `@botho/wasm-signer`'s node-identical seed derivation. This function only
 * packs already-derived keys, so a wallet's own address is produced via
 * `deriveV2Address` (wasm), which calls the shared codec directly.
 */
export function formatAddress(
  viewPublic: Uint8Array,
  spendPublic: Uint8Array,
  kemPublic: Uint8Array,
  dsaPublic: Uint8Array,
  network: 'mainnet' | 'testnet' = 'testnet',
): string {
  if (viewPublic.length !== VIEW_LEN || spendPublic.length !== SPEND_LEN) {
    throw new Error('Invalid address: view and spend keys must be 32 bytes each')
  }
  if (kemPublic.length !== ML_KEM_768_PUBLIC_KEY_LEN) {
    throw new Error(
      `Invalid address: ML-KEM key must be ${ML_KEM_768_PUBLIC_KEY_LEN} bytes, got ${kemPublic.length}`,
    )
  }
  if (dsaPublic.length !== ML_DSA_65_PUBLIC_KEY_LEN) {
    throw new Error(
      `Invalid address: ML-DSA key must be ${ML_DSA_65_PUBLIC_KEY_LEN} bytes, got ${dsaPublic.length}`,
    )
  }

  const body = new Uint8Array(V2_BODY_LEN)
  body.set(viewPublic, 0)
  body.set(spendPublic, VIEW_LEN)
  body.set(kemPublic, VIEW_LEN + SPEND_LEN)
  body.set(dsaPublic, VIEW_LEN + SPEND_LEN + ML_KEM_768_PUBLIC_KEY_LEN)

  const encoded = base58.encode(body)
  const prefix = network === 'testnet' ? TESTNET_PREFIX : MAINNET_PREFIX
  return prefix + encoded
}

/**
 * Parsed v2 Botho address components.
 */
export interface ParsedAddress {
  network: 'mainnet' | 'testnet'
  viewPublic: Uint8Array
  spendPublic: Uint8Array
  /** Raw ML-KEM-768 public key (1184 bytes). */
  kemPublic: Uint8Array
  /** Raw ML-DSA-65 public key (1952 bytes). */
  dsaPublic: Uint8Array
}

/**
 * Parse a v2 Botho address string into its components.
 *
 * Rejects retired formats loudly: the quantum-private prefixes (ADR 0006) and
 * old 64-byte v1 addresses (ADR 0008), which cannot receive on the v2 chain.
 * The base58 body layout matches the shared Rust codec exactly.
 */
export function parseAddress(address: string): ParsedAddress {
  const trimmed = address.trim()

  // Reject retired quantum-private addresses before anything else (ADR 0006).
  if (
    trimmed.startsWith(MAINNET_QUANTUM_PREFIX) ||
    trimmed.startsWith(TESTNET_QUANTUM_PREFIX) ||
    trimmed.startsWith(LEGACY_QUANTUM_PREFIX)
  ) {
    throw new Error(
      'Quantum addresses were retired (ADR 0006). Ask the recipient for a current botho://2/ address.',
    )
  }

  // Reject retired v1 (64-byte, classical-only) addresses loudly (ADR 0008).
  // Checked before the v2 prefix so `botho://1/…` never silently mis-parses.
  if (trimmed.startsWith(MAINNET_V1_PREFIX) || trimmed.startsWith(TESTNET_V1_PREFIX)) {
    throw new Error(
      'Address format v1 (botho://1/) was retired (ADR 0008): it carries no post-quantum keys and cannot receive on the v2 chain. Ask the recipient to regenerate a botho://2/ address.',
    )
  }

  let network: 'mainnet' | 'testnet'
  let encoded: string
  if (trimmed.startsWith(TESTNET_PREFIX)) {
    network = 'testnet'
    encoded = trimmed.slice(TESTNET_PREFIX.length)
  } else if (trimmed.startsWith(MAINNET_PREFIX)) {
    network = 'mainnet'
    encoded = trimmed.slice(MAINNET_PREFIX.length)
  } else {
    throw new Error('Invalid address format: expected botho://2/ or tbotho://2/ prefix')
  }

  const decoded = base58.decode(encoded)
  if (decoded.length !== V2_BODY_LEN) {
    throw new Error(`Invalid address: expected ${V2_BODY_LEN} bytes, got ${decoded.length}`)
  }

  const kemStart = VIEW_LEN + SPEND_LEN
  const dsaStart = kemStart + ML_KEM_768_PUBLIC_KEY_LEN
  return {
    network,
    viewPublic: decoded.slice(0, VIEW_LEN),
    spendPublic: decoded.slice(VIEW_LEN, kemStart),
    kemPublic: decoded.slice(kemStart, dsaStart),
    dsaPublic: decoded.slice(dsaStart, V2_BODY_LEN),
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
