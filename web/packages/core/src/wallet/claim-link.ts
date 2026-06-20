/**
 * Claimable payment links — bearer claim-link helpers.
 *
 * A claim-link is a *bearer instrument backed by a throwaway (ephemeral)
 * wallet*. The link secret IS an ephemeral BIP39 mnemonic; whoever holds it
 * owns the funds. This is 100% client-side: these helpers only generate /
 * encode / decode the ephemeral secret. Funding (a normal CLSAG send to the
 * ephemeral address) and sweeping (a normal CLSAG send FROM the ephemeral keys)
 * reuse the existing wasm-signer send path — no node/consensus/RPC change.
 *
 * Link format (issue #460, architect "Option A"):
 *
 *   https://wallet.botho.io/claim#v1.<base58(16-byte entropy)>[.<amount-hint-pc>]
 *
 * - `v1`           one-char-ish version tag so the format can evolve.
 * - base58 entropy the 12-word (128-bit) mnemonic's raw 16-byte entropy,
 *                  base58-encoded (~22 chars). Reconstructed to the mnemonic via
 *                  `entropyToMnemonic`. Keeping the entropy (not the words) makes
 *                  the link short and chat/QR friendly.
 * - amount-hint-pc OPTIONAL, non-secret cosmetic hint (picocredits) so the claim
 *                  page can show "X BTH" before the on-chain scan completes. The
 *                  scan is always authoritative; the hint adds no privacy loss
 *                  (the holder learns the amount from the scan anyway).
 *
 * Security / bearer model:
 * - 128-bit entropy from `@scure/bip39`'s CSPRNG (same generator as real
 *   wallets). Adequate for a transient bearer secret.
 * - The secret lives ONLY in the URL fragment, which browsers never transmit to
 *   a server. The claim page must strip it (`history.replaceState`) after
 *   reading and never log it.
 * - Anyone with the link can claim — treat it like cash.
 */

import { generateMnemonic, entropyToMnemonic, mnemonicToEntropy } from '@scure/bip39'
import { wordlist } from '@scure/bip39/wordlists/english.js'
import { base58 } from '@scure/base'

/** Current claim-link fragment version tag. */
export const CLAIM_LINK_VERSION = 'v1'

/** Length, in bytes, of the entropy backing a 12-word (128-bit) mnemonic. */
export const CLAIM_LINK_ENTROPY_BYTES = 16

/**
 * A parsed claim-link secret: the reconstructed ephemeral mnemonic plus the
 * optional, non-authoritative amount hint (picocredits) carried in the link.
 */
export interface ClaimLinkSecret {
  /** The ephemeral 12-word BIP39 mnemonic that owns the funded output(s). */
  mnemonic: string
  /**
   * Optional cosmetic amount hint in picocredits, if the link carried one.
   * NEVER trust this for the claimed value — always show the scanned amount.
   */
  amountHint?: bigint
}

/**
 * Generate a fresh ephemeral 12-word mnemonic (128-bit entropy) to back a new
 * claim link. Thin wrapper over the same CSPRNG used for real wallets.
 */
export function createClaimLinkMnemonic(): string {
  return generateMnemonic(wordlist, 128)
}

/**
 * Encode a claim-link fragment for a given ephemeral mnemonic.
 *
 * Returns the fragment WITHOUT the leading `#`, e.g.
 * `v1.<base58 entropy>` or `v1.<base58 entropy>.<amountHint>`.
 *
 * @param mnemonic  the ephemeral 12-word mnemonic (must be 128-bit / 12 words)
 * @param amountHint optional cosmetic picocredit hint to embed
 */
export function encodeClaimLinkFragment(mnemonic: string, amountHint?: bigint): string {
  const entropy = mnemonicToEntropy(mnemonic, wordlist)
  if (entropy.length !== CLAIM_LINK_ENTROPY_BYTES) {
    throw new Error(
      `Claim links require a 12-word (128-bit) mnemonic; got ${entropy.length}-byte entropy`,
    )
  }
  const encoded = base58.encode(entropy)
  let fragment = `${CLAIM_LINK_VERSION}.${encoded}`
  if (amountHint !== undefined) {
    if (amountHint < 0n) throw new Error('amountHint must be non-negative')
    fragment += `.${amountHint.toString()}`
  }
  return fragment
}

/**
 * Build the full shareable claim URL for an ephemeral mnemonic.
 *
 * @param origin      e.g. `https://wallet.botho.io` (no trailing slash needed)
 * @param mnemonic    the ephemeral 12-word mnemonic
 * @param amountHint  optional cosmetic picocredit hint
 */
export function buildClaimLink(origin: string, mnemonic: string, amountHint?: bigint): string {
  const base = origin.replace(/\/$/, '')
  return `${base}/claim#${encodeClaimLinkFragment(mnemonic, amountHint)}`
}

/**
 * Parse a claim-link fragment back into its ephemeral mnemonic (+ optional
 * amount hint). Accepts the fragment with or without a leading `#`, and accepts
 * a full URL (the part after `#` is used). Throws on a malformed/unsupported
 * fragment.
 */
export function parseClaimLinkFragment(fragment: string): ClaimLinkSecret {
  let raw = fragment.trim()
  // Allow passing a whole URL — take only the fragment portion.
  const hashIdx = raw.indexOf('#')
  if (hashIdx >= 0) raw = raw.slice(hashIdx + 1)
  if (raw.startsWith('#')) raw = raw.slice(1)
  if (!raw) throw new Error('Empty claim-link fragment')

  const parts = raw.split('.')
  if (parts[0] !== CLAIM_LINK_VERSION) {
    throw new Error(`Unsupported claim-link version: ${parts[0] || '(none)'}`)
  }
  const encoded = parts[1]
  if (!encoded) throw new Error('Claim-link fragment is missing its secret')

  let entropy: Uint8Array
  try {
    entropy = base58.decode(encoded)
  } catch {
    throw new Error('Claim-link secret is not valid base58')
  }
  if (entropy.length !== CLAIM_LINK_ENTROPY_BYTES) {
    throw new Error('Claim-link secret has the wrong length')
  }

  const mnemonic = entropyToMnemonic(entropy, wordlist)

  let amountHint: bigint | undefined
  if (parts[2] !== undefined && parts[2] !== '') {
    try {
      amountHint = BigInt(parts[2])
    } catch {
      // A malformed hint is cosmetic-only; ignore it rather than fail the claim.
      amountHint = undefined
    }
  }

  return { mnemonic, amountHint }
}
