import { describe, it, expect } from 'vitest'
import {
  CLAIM_LINK_VERSION,
  CLAIM_LINK_ENTROPY_BYTES,
  createClaimLinkMnemonic,
  encodeClaimLinkFragment,
  buildClaimLink,
  parseClaimLinkFragment,
} from './claim-link'
import { isValidMnemonic, deriveAddress, createMnemonic } from './index'

describe('claim-link helpers', () => {
  describe('createClaimLinkMnemonic', () => {
    it('produces a valid 12-word BIP39 mnemonic', () => {
      const m = createClaimLinkMnemonic()
      expect(m.split(' ')).toHaveLength(12)
      expect(isValidMnemonic(m)).toBe(true)
    })

    it('produces unique mnemonics', () => {
      expect(createClaimLinkMnemonic()).not.toBe(createClaimLinkMnemonic())
    })
  })

  describe('encode / parse round-trip', () => {
    it('round-trips an ephemeral mnemonic through the fragment', () => {
      const m = createClaimLinkMnemonic()
      const fragment = encodeClaimLinkFragment(m)
      const parsed = parseClaimLinkFragment(fragment)
      expect(parsed.mnemonic).toBe(m)
      expect(parsed.amountHint).toBeUndefined()
    })

    it('embeds and recovers an amount hint', () => {
      const m = createClaimLinkMnemonic()
      const hint = 5_000_000_000_000n
      const fragment = encodeClaimLinkFragment(m, hint)
      const parsed = parseClaimLinkFragment(fragment)
      expect(parsed.mnemonic).toBe(m)
      expect(parsed.amountHint).toBe(hint)
    })

    it('reconstructs the same ephemeral ADDRESS from a parsed link', () => {
      const m = createClaimLinkMnemonic()
      const expectedAddress = deriveAddress(m)
      const parsed = parseClaimLinkFragment(encodeClaimLinkFragment(m))
      expect(deriveAddress(parsed.mnemonic)).toBe(expectedAddress)
    })

    it('produces a versioned fragment with base58 entropy', () => {
      const m = createClaimLinkMnemonic()
      const fragment = encodeClaimLinkFragment(m)
      const parts = fragment.split('.')
      expect(parts[0]).toBe(CLAIM_LINK_VERSION)
      // base58 of 16 bytes is ~22 chars and uses no 0/O/I/l
      expect(parts[1]).toMatch(/^[A-HJ-NP-Za-km-z1-9]+$/)
    })

    it('keeps the fragment short (chat/QR friendly)', () => {
      const fragment = encodeClaimLinkFragment(createClaimLinkMnemonic())
      // v1. + ~22 base58 chars
      expect(fragment.length).toBeLessThan(30)
    })
  })

  describe('buildClaimLink', () => {
    it('builds a /claim URL with the secret in the fragment', () => {
      const m = createClaimLinkMnemonic()
      const url = buildClaimLink('https://wallet.botho.io', m)
      expect(url.startsWith('https://wallet.botho.io/claim#')).toBe(true)
      const parsed = parseClaimLinkFragment(url)
      expect(parsed.mnemonic).toBe(m)
    })

    it('normalizes a trailing slash on the origin', () => {
      const m = createClaimLinkMnemonic()
      const url = buildClaimLink('https://wallet.botho.io/', m)
      expect(url).toContain('/claim#')
      expect(url).not.toContain('//claim')
    })
  })

  describe('parseClaimLinkFragment robustness', () => {
    it('accepts a fragment with a leading #', () => {
      const m = createClaimLinkMnemonic()
      const frag = encodeClaimLinkFragment(m)
      expect(parseClaimLinkFragment('#' + frag).mnemonic).toBe(m)
    })

    it('accepts a full URL', () => {
      const m = createClaimLinkMnemonic()
      const url = buildClaimLink('https://wallet.botho.io', m)
      expect(parseClaimLinkFragment(url).mnemonic).toBe(m)
    })

    it('rejects an empty fragment', () => {
      expect(() => parseClaimLinkFragment('')).toThrow()
      expect(() => parseClaimLinkFragment('#')).toThrow()
    })

    it('rejects an unsupported version', () => {
      expect(() => parseClaimLinkFragment('v9.abcdef')).toThrow(/version/i)
    })

    it('rejects a non-base58 secret', () => {
      expect(() => parseClaimLinkFragment(`${CLAIM_LINK_VERSION}.0OIl`)).toThrow()
    })

    it('rejects a wrong-length secret', () => {
      // base58 of a 4-byte payload — wrong length for a 16-byte secret
      expect(() => parseClaimLinkFragment(`${CLAIM_LINK_VERSION}.2VfUX`)).toThrow(/length/i)
    })

    it('ignores a malformed amount hint rather than failing', () => {
      const m = createClaimLinkMnemonic()
      const frag = encodeClaimLinkFragment(m)
      const parsed = parseClaimLinkFragment(`${frag}.notanumber`)
      expect(parsed.mnemonic).toBe(m)
      expect(parsed.amountHint).toBeUndefined()
    })
  })

  describe('encodeClaimLinkFragment guards', () => {
    it('rejects a 24-word mnemonic (needs 128-bit entropy)', () => {
      const m24 = createMnemonic()
      expect(() => encodeClaimLinkFragment(m24)).toThrow()
    })

    it('exposes the expected entropy size constant', () => {
      expect(CLAIM_LINK_ENTROPY_BYTES).toBe(16)
    })
  })
})
