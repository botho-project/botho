import { describe, it, expect } from 'vitest'
import {
  mintStatusToken,
  verifyStatusToken,
  DEFAULT_STATUS_TOKEN_TTL_SECONDS,
} from './status-link'

const SECRET = 'test-status-link-secret'
const CUSTOMER = 'cus_ABC123'
const NOW = 1_700_000_000

describe('status-link tokens', () => {
  it('round-trips: a freshly minted token verifies and yields the customer id', async () => {
    const token = await mintStatusToken(CUSTOMER, SECRET, { nowSeconds: NOW })
    const result = await verifyStatusToken(token, SECRET, { nowSeconds: NOW + 10 })
    expect(result.ok).toBe(true)
    if (result.ok) {
      expect(result.customerId).toBe(CUSTOMER)
      expect(result.exp).toBe(NOW + DEFAULT_STATUS_TOKEN_TTL_SECONDS)
    }
  })

  it('rejects a missing/empty token', async () => {
    expect((await verifyStatusToken(null, SECRET)).ok).toBe(false)
    expect((await verifyStatusToken('', SECRET)).ok).toBe(false)
  })

  it('rejects a malformed token (wrong field count)', async () => {
    expect((await verifyStatusToken('cus_ABC123.123', SECRET)).ok).toBe(false)
    expect((await verifyStatusToken('garbage', SECRET)).ok).toBe(false)
  })

  it('rejects a token whose customer id was tampered with (signature mismatch)', async () => {
    const token = await mintStatusToken(CUSTOMER, SECRET, { nowSeconds: NOW })
    const parts = token.split('.')
    // Swap in a DIFFERENT customer id but keep the original exp + signature.
    const forged = `cus_VICTIM999.${parts[1]}.${parts[2]}`
    const result = await verifyStatusToken(forged, SECRET, { nowSeconds: NOW + 10 })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.reason).toBe('signature mismatch')
  })

  it('rejects a token signed with a different secret', async () => {
    const token = await mintStatusToken(CUSTOMER, 'other-secret', { nowSeconds: NOW })
    const result = await verifyStatusToken(token, SECRET, { nowSeconds: NOW + 10 })
    expect(result.ok).toBe(false)
  })

  it('rejects an expired token', async () => {
    const token = await mintStatusToken(CUSTOMER, SECRET, {
      ttlSeconds: 60,
      nowSeconds: NOW,
    })
    const result = await verifyStatusToken(token, SECRET, { nowSeconds: NOW + 61 })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.reason).toBe('token expired')
  })

  it('rejects an expiry-tampered token before honouring the new expiry', async () => {
    const token = await mintStatusToken(CUSTOMER, SECRET, {
      ttlSeconds: 60,
      nowSeconds: NOW,
    })
    const parts = token.split('.')
    // Extend the expiry far into the future without re-signing.
    const forged = `${parts[0]}.${NOW + 999_999}.${parts[2]}`
    const result = await verifyStatusToken(forged, SECRET, { nowSeconds: NOW + 70 })
    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.reason).toBe('signature mismatch')
  })

  it('refuses to mint a token for an implausible customer id', async () => {
    await expect(mintStatusToken('not-a-customer', SECRET)).rejects.toThrow()
    await expect(mintStatusToken('cus_with.dot', SECRET)).rejects.toThrow()
  })

  it('returns a clear failure when the secret is unset', async () => {
    const r = await verifyStatusToken('cus_X.1.2', '')
    expect(r.ok).toBe(false)
  })
})
