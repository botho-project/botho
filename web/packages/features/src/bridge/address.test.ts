import { describe, expect, it } from 'vitest'
import { isValidDestinationAddress } from './address'

describe('isValidDestinationAddress', () => {
  it('accepts a well-formed EVM address (0x + 40 hex, any case)', () => {
    expect(
      isValidDestinationAddress('ethereum', '0x49b985ec427ee771a601f11b18f7d4402fa2dd7b'),
    ).toBe(true)
    // Mixed case (EIP-55-style) is accepted — we do not enforce the checksum.
    expect(
      isValidDestinationAddress('ethereum', '0x16C4fDbe2b7497EA67f1DC8205dd2F5B31458D53'),
    ).toBe(true)
  })

  it('rejects malformed EVM addresses', () => {
    expect(isValidDestinationAddress('ethereum', '')).toBe(false)
    expect(isValidDestinationAddress('ethereum', '0x123')).toBe(false)
    expect(isValidDestinationAddress('ethereum', '49b985ec427ee771a601f11b18f7d4402fa2dd7b')).toBe(
      false,
    )
    // 39 hex nibbles (one short).
    expect(isValidDestinationAddress('ethereum', '0x49b985ec427ee771a601f11b18f7d4402fa2dd7')).toBe(
      false,
    )
    // Non-hex character.
    expect(isValidDestinationAddress('ethereum', '0xZZb985ec427ee771a601f11b18f7d4402fa2dd7b')).toBe(
      false,
    )
  })

  it('accepts base58 Solana addresses of the right length', () => {
    expect(
      isValidDestinationAddress('solana', 'F7LsiATxVQxnDEBWemfuq1BgFDYbuzqMMJ5eZjaB7LFX'),
    ).toBe(true)
    expect(
      isValidDestinationAddress('solana', '9Yog17D3nt1v9cREJBh1Ddeo6fRGuS48hbQcaH9WH1JS'),
    ).toBe(true)
  })

  it('rejects Solana addresses with non-base58 chars or bad length', () => {
    expect(isValidDestinationAddress('solana', '')).toBe(false)
    // Contains 0/O/I/l — not in the base58 alphabet.
    expect(isValidDestinationAddress('solana', '0OIl' + 'a'.repeat(40))).toBe(false)
    // Too short.
    expect(isValidDestinationAddress('solana', 'abc')).toBe(false)
    // An EVM address is not a valid Solana address.
    expect(
      isValidDestinationAddress('solana', '0x49b985ec427ee771a601f11b18f7d4402fa2dd7b'),
    ).toBe(false)
  })

  it('trims surrounding whitespace before validating', () => {
    expect(
      isValidDestinationAddress('ethereum', '  0x49b985ec427ee771a601f11b18f7d4402fa2dd7b  '),
    ).toBe(true)
  })
})
