/**
 * Destination-address validation for the integrated export flow (#1031).
 *
 * The Botho wallet does the BTH side ONLY — it never touches the counterparty
 * chain. But it must let the user type the EVM/SVM address wBTH will be minted
 * to, and reject obviously malformed input before an order is opened. These are
 * intentionally lightweight FORMAT checks (no web3/EVM/Solana client libraries,
 * per epic #1029): an EVM address is `0x` + 40 hex; a Solana address is a
 * base58 string that decodes to 32 bytes (length 32–44 in base58). We do not
 * verify EIP-55 checksums or on-curve-ness — that is the user's own wallet's
 * job, and the bridge validates the address again server-side.
 */
import type { DestinationChain } from './types'

/** `0x` followed by exactly 40 hex nibbles (20 bytes). Case-insensitive. */
const EVM_ADDRESS_RE = /^0x[0-9a-fA-F]{40}$/

/**
 * Base58 alphabet (Bitcoin/Solana), i.e. no `0`, `O`, `I`, `l`. A Solana
 * address is 32 bytes base58-encoded, which is 32–44 characters. We bound the
 * length and alphabet without pulling in a base58 decoder.
 */
const SOLANA_ADDRESS_RE = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/

/**
 * Validate a destination address for the given chain. Trims surrounding
 * whitespace; returns `false` for empty input.
 */
export function isValidDestinationAddress(
  chain: DestinationChain,
  address: string,
): boolean {
  const a = address.trim()
  if (a.length === 0) return false
  switch (chain) {
    case 'ethereum':
      return EVM_ADDRESS_RE.test(a)
    case 'solana':
      return SOLANA_ADDRESS_RE.test(a)
    default:
      return false
  }
}
