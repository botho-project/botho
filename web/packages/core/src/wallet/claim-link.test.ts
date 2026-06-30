import { describe, it, expect } from 'vitest'
import {
  CLAIM_LINK_VERSION,
  CLAIM_LINK_ENTROPY_BYTES,
  CLAIM_LINK_MAX_AMOUNT_PICOCREDITS,
  createClaimLinkMnemonic,
  encodeClaimLinkFragment,
  buildClaimLink,
  parseClaimLinkFragment,
  isWithinClaimLinkCap,
  assertClaimLinkAmountWithinCap,
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

  // -------------------------------------------------------------------------
  // Unfurl-safety invariant (#589)
  //
  // The property that makes sharing a claim link over an E2E messenger safe:
  // the bearer secret lives ONLY in the URL FRAGMENT. Browsers never transmit
  // the fragment to a server, so a messenger link-preview / unfurl fetch (a
  // server-side GET of the URL) can never see — let alone act on — the secret.
  // These tests codify that the secret never leaks into the path or query.
  // -------------------------------------------------------------------------
  describe('unfurl-safety invariant: secret is fragment-only', () => {
    function secretOf(fragment: string): string {
      const s = fragment.replace(/^#/, '').split('.')[1]
      expect(s).toBeTruthy()
      return s
    }

    it('puts the bearer secret ONLY in the fragment (never path or query)', () => {
      const m = createClaimLinkMnemonic()
      const url = buildClaimLink('https://wallet.botho.io', m)
      const u = new URL(url)
      const secret = secretOf(u.hash)

      // The fragment carries the secret...
      expect(u.hash.slice(1)).toContain(secret)
      // ...and the path / query carry none of it.
      expect(u.pathname).toBe('/claim')
      expect(u.search).toBe('')
      expect(u.pathname).not.toContain(secret)
      expect(u.search).not.toContain(secret)
    })

    it('keeps the secret out of the part of the URL a server would receive', () => {
      // A messenger preview bot / web server only ever sees everything BEFORE
      // the `#` (browsers strip the fragment before sending the request).
      const m = createClaimLinkMnemonic()
      const amountHint = 5_000_000_000_000n
      const url = buildClaimLink('https://wallet.botho.io', m, amountHint)
      const hashIdx = url.indexOf('#')
      const serverVisible = url.slice(0, hashIdx)
      const secret = secretOf(url.slice(hashIdx))

      expect(serverVisible).toBe('https://wallet.botho.io/claim')
      expect(serverVisible).not.toContain(secret)
      // The amount hint is also fragment-only — nothing about the link reaches
      // the server beyond the static `/claim` path.
      expect(serverVisible).not.toContain(amountHint.toString())
      expect(serverVisible).not.toContain('?')
    })

    it('never emits a query string, even with an amount hint', () => {
      const m = createClaimLinkMnemonic()
      const url = buildClaimLink('https://wallet.botho.io', m, 1n)
      const u = new URL(url)
      expect(u.search).toBe('')
      // Everything secret-bearing is after the `#`.
      expect(url.indexOf('?')).toBe(-1)
    })
  })

  // -------------------------------------------------------------------------
  // Per-link amount cap (#589): treat a claim link like cash.
  // -------------------------------------------------------------------------
  describe('per-link amount cap', () => {
    it('caps the maximum at 1,000 BTH (in picocredits)', () => {
      expect(CLAIM_LINK_MAX_AMOUNT_PICOCREDITS).toBe(1_000n * 1_000_000_000_000n)
    })

    it('isWithinClaimLinkCap accepts positive amounts up to the cap', () => {
      expect(isWithinClaimLinkCap(1n)).toBe(true)
      expect(isWithinClaimLinkCap(CLAIM_LINK_MAX_AMOUNT_PICOCREDITS)).toBe(true)
    })

    it('isWithinClaimLinkCap rejects non-positive and over-cap amounts', () => {
      expect(isWithinClaimLinkCap(0n)).toBe(false)
      expect(isWithinClaimLinkCap(-1n)).toBe(false)
      expect(isWithinClaimLinkCap(CLAIM_LINK_MAX_AMOUNT_PICOCREDITS + 1n)).toBe(false)
    })

    it('assertClaimLinkAmountWithinCap throws above the cap with a request-link nudge', () => {
      expect(() =>
        assertClaimLinkAmountWithinCap(CLAIM_LINK_MAX_AMOUNT_PICOCREDITS + 1n),
      ).toThrow(/request link/i)
    })

    it('assertClaimLinkAmountWithinCap permits amounts at or below the cap', () => {
      expect(() => assertClaimLinkAmountWithinCap(CLAIM_LINK_MAX_AMOUNT_PICOCREDITS)).not.toThrow()
      expect(() => assertClaimLinkAmountWithinCap(1n)).not.toThrow()
    })
  })
})
