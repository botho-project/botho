/**
 * @vitest-environment jsdom
 *
 * Operator read-token capture (#707): the magic-link token is lifted from the
 * URL fragment into sessionStorage and stripped from the address bar, so the
 * bearer credential is neither persisted to disk nor left in the URL.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { captureOperatorToken, clearOperatorToken, getStoredOperatorToken } from './token'

const TOKEN = 'op.1799999999.deadbeefcafef00d'

beforeEach(() => {
  window.sessionStorage.clear()
  window.history.replaceState(null, '', '/operator')
})
afterEach(() => {
  window.sessionStorage.clear()
})

describe('captureOperatorToken', () => {
  it('lifts a #token fragment into sessionStorage and strips it from the URL', () => {
    window.history.replaceState(null, '', `/operator#token=${TOKEN}`)
    const captured = captureOperatorToken()
    expect(captured).toBe(TOKEN)
    expect(getStoredOperatorToken()).toBe(TOKEN)
    // The fragment is stripped so the credential doesn't linger in the URL.
    expect(window.location.hash).toBe('')
  })

  it('returns the previously stored token when no fragment is present', () => {
    window.sessionStorage.setItem('botho.operator.readToken', TOKEN)
    expect(captureOperatorToken()).toBe(TOKEN)
  })

  it('returns null when neither a fragment nor a stored token exists', () => {
    expect(captureOperatorToken()).toBeNull()
  })

  it('ignores an empty #token= fragment', () => {
    window.history.replaceState(null, '', '/operator#token=')
    expect(captureOperatorToken()).toBeNull()
  })

  it('clearOperatorToken forgets the stored token', () => {
    window.sessionStorage.setItem('botho.operator.readToken', TOKEN)
    clearOperatorToken()
    expect(getStoredOperatorToken()).toBeNull()
  })
})
