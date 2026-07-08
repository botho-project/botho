/**
 * Operator read-token capture + session storage (#707, P4.2 of #695).
 *
 * The node mints a magic-link READ token (`botho operator mint-read-link`) and
 * the operator opens `https://<dashboard>/operator#token=op.<exp>.<hmac>`. This
 * module lifts that token out of the URL fragment into `sessionStorage` and
 * strips it from the address bar, so:
 *   - the credential is never persisted to disk (sessionStorage clears when the
 *     tab closes) — the #474 no-plaintext-at-rest posture, applied to a bearer
 *     read credential;
 *   - it is not left sitting in `location.href` (browser history / referer).
 *
 * The token is opaque to the client: only the NODE verifies it (constant-time
 * HMAC, signature-before-expiry). The dashboard just carries it as a `token`
 * param on `operator_*` calls and degrades gracefully to the public read-only
 * view when it is absent or rejected.
 */

const STORAGE_KEY = 'botho.operator.readToken'
const HASH_PREFIX = '#token='

/** Best-effort `sessionStorage` access (guards SSR / privacy-mode throws). */
function safeSession(): Storage | null {
  try {
    return typeof window !== 'undefined' ? window.sessionStorage : null
  } catch {
    return null
  }
}

/**
 * Capture an operator token from the URL fragment (`#token=...`) into
 * sessionStorage, then strip it from the address bar. Returns the effective
 * token (freshly captured, or the previously stored one), or `null` if none.
 *
 * Idempotent: safe to call on every mount.
 */
export function captureOperatorToken(): string | null {
  const store = safeSession()

  if (typeof window !== 'undefined' && window.location.hash.startsWith(HASH_PREFIX)) {
    const token = window.location.hash.slice(HASH_PREFIX.length).trim()
    if (token) {
      store?.setItem(STORAGE_KEY, token)
      // Strip the fragment so the credential doesn't linger in the URL.
      try {
        const url = window.location.pathname + window.location.search
        window.history.replaceState(null, '', url)
      } catch {
        // Non-fatal: worst case the token stays in the hash for this load.
      }
      return token
    }
  }

  return store?.getItem(STORAGE_KEY) ?? null
}

/** The stored operator token, if any. Does not read the URL fragment. */
export function getStoredOperatorToken(): string | null {
  return safeSession()?.getItem(STORAGE_KEY) ?? null
}

/** Forget the stored operator token (sign-out / after a rejection). */
export function clearOperatorToken(): void {
  safeSession()?.removeItem(STORAGE_KEY)
}
