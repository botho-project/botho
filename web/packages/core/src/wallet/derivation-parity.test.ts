/**
 * Key-derivation parity test: TypeScript vs Rust.
 *
 * The browser wallet signs transactions with keys derived from the mnemonic in
 * TypeScript (`deriveKeypairs`). For a node to accept a wallet-built tx, those
 * keys MUST be byte-identical to the keys the Rust node derives from the same
 * mnemonic (`Mnemonic::derive_slip10_key(0)` -> `Account::from(&slip10_key)` in
 * `core/src/slip10/mod.rs`).
 *
 * The Rust derivation is:
 *   1. BIP39 seed (empty passphrase)
 *   2. SLIP-0010 Ed25519 derivation at m/44'/866'/account'
 *   3. HKDF-SHA512(salt = "botho-ristretto255-{view,spend}", ikm = slip10 key)
 *      -> 64-byte OKM
 *   4. Scalar::from_bytes_mod_order_wide(OKM) -> the private scalar
 *
 * The authoritative expected OKM bytes are the `view_hex`/`spend_hex` fields of
 * `core/tests/slip10_mnemonic.json` (the same vectors the Rust unit test
 * `mnemonic_into_account_key` asserts against). We reduce them mod the curve
 * order here exactly as `from_bytes_mod_order_wide` does, and require the TS
 * `deriveKeypairs` private scalars to match. If this passes, JS-derived keys
 * are guaranteed identical to the node's, so a wallet-signed tx will not be
 * rejected for a key mismatch.
 */

import { describe, it, expect } from 'vitest'
import { deriveKeypairs, deriveDefaultSubaddressPublicKeys } from './address'

// Ed25519 / Ristretto255 group order L = 2^252 + 27742317777372353535851937790883648493.
const CURVE_ORDER = BigInt(
  '7237005577332262213973186563042994240857116359379907606001950938285454250989',
)

/** Parse a hex string into bytes. */
function hexToBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2)
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16)
  }
  return out
}

/** Little-endian bytes -> BigInt (matches curve25519-dalek scalar layout). */
function bytesToBigIntLE(bytes: Uint8Array): bigint {
  let result = 0n
  for (let i = bytes.length - 1; i >= 0; i--) {
    result = (result << 8n) | BigInt(bytes[i])
  }
  return result
}

/** BigInt -> 32 little-endian bytes. */
function bigIntToBytesLE(n: bigint): Uint8Array {
  const bytes = new Uint8Array(32)
  let temp = n
  for (let i = 0; i < 32; i++) {
    bytes[i] = Number(temp & 0xffn)
    temp >>= 8n
  }
  return bytes
}

/**
 * Reduce a 64-byte wide value mod L, exactly as
 * `curve25519_dalek::Scalar::from_bytes_mod_order_wide`. The Rust JSON vectors
 * store the raw 64-byte HKDF OKM, so reducing here yields the canonical 32-byte
 * private scalar the node holds.
 */
function reduceWide(wideHex: string): Uint8Array {
  const wide = hexToBytes(wideHex)
  expect(wide.length).toBe(64)
  return bigIntToBytesLE(bytesToBigIntLE(wide) % CURVE_ORDER)
}

function toHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('')
}

// Vectors copied verbatim from core/tests/slip10_mnemonic.json (account_index 0).
// These are the same vectors the Rust `mnemonic_into_account_key` test asserts.
const RUST_VECTORS: ReadonlyArray<{
  phrase: string
  viewWideHex: string
  spendWideHex: string
}> = [
  {
    phrase:
      'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
    viewWideHex:
      'b731055a24cf3afe6e18f86161dfd4d2ae05ce9f8263f0525a33ad12d56d78afea6d68ad279efbbf8ea55a08fa62949ea05d414d94a11961f5531c4cd2a88988',
    spendWideHex:
      '44beb5967db5e179c36aa91105c385cfa8cf1611d7d951a821874ae8b6641f57ef311486819ca887b0fe296a948d65c9d86fac01491597dc7a2ed60ca72a1e2e',
  },
  {
    phrase:
      'legal winner thank year wave sausage worth useful legal winner thank yellow',
    viewWideHex:
      '06b808e57f7e8b1dbc9e3907b5daa88d683c2089c153d6c6a2697b8efc69ad4f5d898dc8c29cd4e2f18bf9694f8c12921a1017ad52e837a8c274f87af75f5f7a',
    spendWideHex:
      '40c318c9d849e22d29cf3793e01b2ffbe9f397952038452a10f37c9c157d57da801f5f0a05f90c68b7761c9188d5625f9af4f33ec4dc18afacac6a24c071dec9',
  },
  {
    phrase:
      'letter advice cage absurd amount doctor acoustic avoid letter advice cage above',
    viewWideHex:
      '9523526422933f967f11f27347b662f2345ad3c38612321c64717f30aa0f0493ac94eb97385b871506e07c3b4a1346ac573b59f6c88d8a4a7307b28604d79133',
    spendWideHex:
      '695325a1b3563e19ba94f6e666d9a4d90e106c7914c6dafc51cd40a665ecf2308afc57ef4353d285c34f02e4d8097ef010765e3a949d47b8812bb1981a285ed2',
  },
]

describe('SLIP-10 key derivation parity (TS <-> Rust)', () => {
  for (const vec of RUST_VECTORS) {
    it(`matches Rust spend/view scalars for "${vec.phrase.split(' ').slice(0, 3).join(' ')} ..."`, () => {
      const kp = deriveKeypairs(vec.phrase, 0)

      const expectedView = reduceWide(vec.viewWideHex)
      const expectedSpend = reduceWide(vec.spendWideHex)

      expect(toHex(kp.viewPrivate)).toBe(toHex(expectedView))
      expect(toHex(kp.spendPrivate)).toBe(toHex(expectedSpend))
    })
  }
})

// Authoritative DEFAULT-SUBADDRESS (index 0) public keys, generated from the
// Rust node's subaddress derivation (`core/src/subaddress.rs`, the
// `(&RootViewPrivate, &RootSpendPrivate)` impl reached via
// `Account::subaddress(0)`). The wallet displays an address built from THESE
// keys, and the node's `TxOutput::belongs_to` scans for ownership against the
// default-subaddress spend key — so the TS subaddress derivation must produce
// these exact bytes or funds sent to the displayed address are undetectable by
// the recipient (the bug fixed by #383).
//
// To regenerate: add a temporary test in `core/src/slip10/mod.rs` that prints
// `hex::encode(account_key.subaddress(0).{view,spend}_public_key().to_bytes())`
// for each phrase and run `cargo test -p bth-core --features bip39 ... --nocapture`.
const DEFAULT_SUBADDRESS_PUBKEY_VECTORS: ReadonlyArray<{
  phrase: string
  viewPublicHex: string
  spendPublicHex: string
}> = [
  {
    phrase:
      'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
    viewPublicHex: '60eeebc23d5d4fa3b90621292da88f39c6df05114bd405319cf9adc905905773',
    spendPublicHex: '8e2cf7239559d62c6ca0c0718eac345da1fa9348aa741a94d6489025a05a917c',
  },
  {
    phrase:
      'legal winner thank year wave sausage worth useful legal winner thank yellow',
    viewPublicHex: 'fe8b11112542a664c5ce2954596fafd21a0766f5b02b3e4cbea5ee6d56b0d845',
    spendPublicHex: 'e22d1ee2eb8f99c3af59c615d11b7eb4560feff4be667e12172ce6e5ba119372',
  },
  {
    phrase:
      'letter advice cage absurd amount doctor acoustic avoid letter advice cage above',
    viewPublicHex: '9085161414daed6c8f503092940a2d13778cd581c16bbb9bc84b0dccf6cb8d33',
    spendPublicHex: 'eca8c52ed862f73c38d7322663819001a14da1dad96003e9f98849707fee8838',
  },
]

describe('Default-subaddress public-key parity (TS <-> Rust)', () => {
  for (const vec of DEFAULT_SUBADDRESS_PUBKEY_VECTORS) {
    it(`matches Rust default-subaddress keys for "${vec.phrase.split(' ').slice(0, 3).join(' ')} ..."`, () => {
      const { viewPublic, spendPublic } = deriveDefaultSubaddressPublicKeys(vec.phrase, 0)

      expect(toHex(viewPublic)).toBe(vec.viewPublicHex)
      expect(toHex(spendPublic)).toBe(vec.spendPublicHex)
    })
  }
})
