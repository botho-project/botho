import { describe, it, expect } from 'vitest'
import {
  buildPaymentRequestFragment,
  buildPaymentRequestLink,
  parsePaymentRequestFragment,
  type PaymentRequest,
} from './payment-request'

const ADDR = 'tbotho://2/abcdef0123456789'

/**
 * The PREVIOUS `btoa`/`TextEncoder`-based encoder, reproduced verbatim so we can
 * assert the SES-safe `@scure/base` + pure-UTF-8 encoder (#1108) produces
 * byte-for-byte identical fragments. This is the regression guard that keeps
 * `/pay#…` links minted before the SES swap round-tripping. `btoa`/`TextEncoder`
 * are fine HERE (the test runs under jsdom/Node) — the point is the PROMOTED
 * encoder must match this legacy output without touching those globals.
 */
function legacyEncodeFragment(req: PaymentRequest): string {
  const wire: { to: string; amount?: string; memo?: string } = { to: req.to.trim() }
  if (req.amount !== undefined && req.amount > 0n) wire.amount = req.amount.toString()
  const memo = req.memo?.trim()
  if (memo) wire.memo = memo
  const json = JSON.stringify(wire)
  const bytes = new TextEncoder().encode(json)
  let binary = ''
  for (const b of bytes) binary += String.fromCharCode(b)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

/** base64url-encode a JSON value the same way the lib does, for crafting fixtures. */
function encodeWire(value: unknown): string {
  const json = JSON.stringify(value)
  const bytes = new TextEncoder().encode(json)
  let binary = ''
  for (const b of bytes) binary += String.fromCharCode(b)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

describe('payment-request fragment helpers (@botho/core, #1108)', () => {
  describe('build / parse round-trip', () => {
    it('round-trips an address with amount and memo', () => {
      const req: PaymentRequest = { to: ADDR, amount: 5_000_000_000_000n, memo: 'Lunch' }
      const parsed = parsePaymentRequestFragment(buildPaymentRequestFragment(req))
      expect(parsed.to).toBe(ADDR)
      expect(parsed.amount).toBe(5_000_000_000_000n)
      expect(parsed.memo).toBe('Lunch')
    })

    it('round-trips with no amount (payer chooses)', () => {
      const parsed = parsePaymentRequestFragment(buildPaymentRequestFragment({ to: ADDR }))
      expect(parsed.to).toBe(ADDR)
      expect(parsed.amount).toBeUndefined()
      expect(parsed.memo).toBeUndefined()
    })

    it('round-trips with amount but no memo', () => {
      const parsed = parsePaymentRequestFragment(
        buildPaymentRequestFragment({ to: ADDR, amount: 1n }),
      )
      expect(parsed.to).toBe(ADDR)
      expect(parsed.amount).toBe(1n)
      expect(parsed.memo).toBeUndefined()
    })

    it('round-trips with memo but no amount', () => {
      const parsed = parsePaymentRequestFragment(
        buildPaymentRequestFragment({ to: ADDR, memo: 'Invoice #42' }),
      )
      expect(parsed.to).toBe(ADDR)
      expect(parsed.amount).toBeUndefined()
      expect(parsed.memo).toBe('Invoice #42')
    })

    it('treats a zero amount as "no amount"', () => {
      const parsed = parsePaymentRequestFragment(
        buildPaymentRequestFragment({ to: ADDR, amount: 0n }),
      )
      expect(parsed.amount).toBeUndefined()
    })

    it('preserves a large bigint amount exactly', () => {
      const big = 123_456_789_012_345_678_901n
      const parsed = parsePaymentRequestFragment(
        buildPaymentRequestFragment({ to: ADDR, amount: big }),
      )
      expect(parsed.amount).toBe(big)
    })

    it('preserves unicode in the memo', () => {
      const memo = 'Café ☕ — 支払い'
      const parsed = parsePaymentRequestFragment(
        buildPaymentRequestFragment({ to: ADDR, memo }),
      )
      expect(parsed.memo).toBe(memo)
    })

    it('preserves an astral (emoji) code point in the memo', () => {
      // 💸 (U+1F4B8) is a surrogate pair — exercises the pure-UTF-8 codec's
      // 4-byte path in BOTH directions.
      const memo = 'thanks 💸'
      const parsed = parsePaymentRequestFragment(
        buildPaymentRequestFragment({ to: ADDR, memo }),
      )
      expect(parsed.memo).toBe(memo)
    })

    it('parses a fragment carrying a leading #', () => {
      const frag = buildPaymentRequestFragment({ to: ADDR, amount: 7n })
      expect(parsePaymentRequestFragment('#' + frag).amount).toBe(7n)
    })

    it('parses a whole URL, using only the fragment portion', () => {
      const url = buildPaymentRequestLink('https://wallet.botho.io', { to: ADDR, amount: 7n })
      const parsed = parsePaymentRequestFragment(url)
      expect(parsed.to).toBe(ADDR)
      expect(parsed.amount).toBe(7n)
    })
  })

  describe('SES-safe encoder is byte-identical to the legacy btoa encoder (#1108)', () => {
    // Guards the base64url swap: fragments minted before the SES-safe change must
    // still parse, and freshly minted fragments must equal the legacy output.
    const cases: PaymentRequest[] = [
      { to: ADDR },
      { to: ADDR, amount: 5_000_000_000_000n },
      { to: ADDR, amount: 1n, memo: 'Invoice #42' },
      { to: ADDR, memo: 'Café ☕ — 支払い' },
      { to: ADDR, memo: 'thanks 💸' },
      { to: ADDR, amount: 123_456_789_012_345_678_901n, memo: 'big' },
    ]
    it.each(cases.map((c, i) => [i, c] as const))(
      'case %#: fragment matches legacy byte-for-byte',
      (_i, req) => {
        expect(buildPaymentRequestFragment(req)).toBe(legacyEncodeFragment(req))
      },
    )

    it('parses a fragment produced by the legacy btoa encoder', () => {
      const legacy = legacyEncodeFragment({ to: ADDR, amount: 42n, memo: 'legacy 支払い' })
      const parsed = parsePaymentRequestFragment(legacy)
      expect(parsed.to).toBe(ADDR)
      expect(parsed.amount).toBe(42n)
      expect(parsed.memo).toBe('legacy 支払い')
    })
  })

  describe('buildPaymentRequestFragment validation', () => {
    it('rejects a missing recipient', () => {
      expect(() => buildPaymentRequestFragment({ to: '' })).toThrow()
      expect(() => buildPaymentRequestFragment({ to: '   ' })).toThrow()
    })

    it('rejects a negative amount', () => {
      expect(() => buildPaymentRequestFragment({ to: ADDR, amount: -1n })).toThrow()
    })

    it('trims the recipient address', () => {
      const parsed = parsePaymentRequestFragment(
        buildPaymentRequestFragment({ to: `  ${ADDR}  ` }),
      )
      expect(parsed.to).toBe(ADDR)
    })
  })

  describe('buildPaymentRequestLink', () => {
    it('targets /pay and carries the payload in the fragment', () => {
      const url = buildPaymentRequestLink('https://wallet.botho.io', { to: ADDR })
      expect(url.startsWith('https://wallet.botho.io/pay#')).toBe(true)
      // The address must NOT appear in the query string portion.
      const [beforeHash] = url.split('#')
      expect(beforeHash).not.toContain('?')
    })

    it('tolerates a trailing slash on the origin', () => {
      const url = buildPaymentRequestLink('https://wallet.botho.io/', { to: ADDR })
      expect(url.startsWith('https://wallet.botho.io/pay#')).toBe(true)
    })
  })

  describe('parsePaymentRequestFragment malformed input', () => {
    it('rejects an empty fragment', () => {
      expect(() => parsePaymentRequestFragment('')).toThrow()
      expect(() => parsePaymentRequestFragment('#')).toThrow()
    })

    it('rejects non-base64 / non-JSON garbage', () => {
      expect(() => parsePaymentRequestFragment('!!!not-base64!!!')).toThrow()
    })

    it('rejects valid base64 that is not JSON', () => {
      // "hello" base64url'd is "aGVsbG8" — decodes fine but is not JSON.
      expect(() => parsePaymentRequestFragment('aGVsbG8')).toThrow()
    })

    it('rejects JSON without a recipient', () => {
      expect(() => parsePaymentRequestFragment(encodeWire({ amount: '5' }))).toThrow()
    })

    it('rejects a non-numeric amount', () => {
      expect(() => parsePaymentRequestFragment(encodeWire({ to: ADDR, amount: 'abc' }))).toThrow()
    })

    it('rejects a JSON array payload', () => {
      expect(() => parsePaymentRequestFragment(encodeWire([ADDR]))).toThrow()
    })
  })
})
