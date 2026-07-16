/**
 * Browser wallet address production (address format v2, ADR 0008 / issue #965).
 *
 * A `botho://2/…` address carries, besides the two classical Ristretto keys, the
 * account's raw ML-KEM-768 (1184 B) and ML-DSA-65 (1952 B) public keys. Those
 * post-quantum keys MUST be derived exactly as the node derives them
 * (`derive_pq_keys_from_seed`) or an output built to the address would be
 * unreceivable. Re-implementing ML-KEM/ML-DSA key generation in JavaScript is a
 * correctness minefield, so this module derives them in wasm (the same Rust the
 * node runs) and encodes the address through the shared codec — JavaScript only
 * computes the classical half (pinned byte-identical to the node) and the BIP39
 * seed.
 *
 * This is the canonical way the browser wallet produces its OWN shareable
 * address. Parsing an address someone else pasted (to extract recipient keys) is
 * the pure-, synchronous-JS `parseAddress` in `@botho/core` — its base58 body
 * layout matches this codec byte-for-byte.
 */

import { mnemonicToSeedSync } from '@scure/bip39'
import { deriveDefaultSubaddressPublicKeys } from '@botho/core'
import { loadSigner, type DecodedV2Address } from './index'

/** Lowercase-hex encode a byte array. */
function toHex(bytes: Uint8Array): string {
  let out = ''
  for (const b of bytes) out += b.toString(16).padStart(2, '0')
  return out
}

/**
 * Derive the wallet's full v2 address string from its BIP39 mnemonic.
 *
 * Mirrors the node's `WalletKeys::public_address_string`:
 *   1. classical default-subaddress (index 0) view/spend keys — derived in JS
 *      (`deriveDefaultSubaddressPublicKeys`, pinned to the node by
 *      `derivation-parity.test.ts`);
 *   2. ML-KEM-768 / ML-DSA-65 public keys — derived in wasm from the same BIP39
 *      seed via the node-identical `derive_pq_keys_from_seed`;
 *   3. encoded through the shared address codec (also in wasm).
 *
 * The result is a `botho://2/…` (mainnet) / `tbotho://2/…` (testnet) address the
 * node accepts and can receive to.
 */
export async function deriveV2Address(
  mnemonic: string,
  network: 'mainnet' | 'testnet' = 'testnet',
  accountIndex = 0,
): Promise<string> {
  const { viewPublic, spendPublic } = deriveDefaultSubaddressPublicKeys(mnemonic, accountIndex)
  const seed = mnemonicToSeedSync(mnemonic, '')
  const signer = await loadSigner()
  if (!signer.deriveAddressFromSeed) {
    throw new Error('wasm-signer: deriveAddressFromSeed unavailable (stale wasm build?)')
  }
  return signer.deriveAddressFromSeed(
    toHex(seed),
    toHex(viewPublic),
    toHex(spendPublic),
    network === 'testnet',
  )
}

/**
 * Derive the wallet's raw ML-KEM-768 public key (hex) from its BIP39 mnemonic,
 * via the node-identical `derive_pq_keys_from_seed` in wasm.
 *
 * This is the sender's OWN ML-KEM public key — the one published in its
 * `botho://2/` address. The send path needs it to encapsulate the change output
 * back to the sender (a hybrid self-send), so the sender can later recover its
 * change under the post-quantum scheme (issue #978).
 */
export async function deriveKemPublicKey(mnemonic: string): Promise<string> {
  const seed = mnemonicToSeedSync(mnemonic, '')
  const signer = await loadSigner()
  if (!signer.derivePqPublicKeysFromSeed) {
    throw new Error('wasm-signer: derivePqPublicKeysFromSeed unavailable (stale wasm build?)')
  }
  return signer.derivePqPublicKeysFromSeed(toHex(seed)).kemPublicKey
}

/**
 * Decode a v2 address string into its hex components via the shared wasm codec.
 *
 * Use this when a byte-identical-to-the-node decode is required (e.g. to
 * validate that a produced address round-trips). For the hot synchronous send
 * path, `@botho/core`'s `parseAddress` decodes the same layout without loading
 * wasm.
 */
export async function decodeV2Address(address: string): Promise<DecodedV2Address> {
  const signer = await loadSigner()
  if (!signer.decodeAddress) {
    throw new Error('wasm-signer: decodeAddress unavailable (stale wasm build?)')
  }
  return signer.decodeAddress(address)
}
