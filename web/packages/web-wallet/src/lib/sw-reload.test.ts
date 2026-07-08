import { describe, it, expect } from 'vitest'
import { shouldReloadOnControllerChange, FRAGMENT_CONSUMING_PATHS } from './sw-reload'

/**
 * The service-worker auto-update reload must NOT fire on the fragment-consuming
 * link pages (`/pay`, `/claim`) — the fragment has already been stripped for
 * privacy (#589) by the time the SW activates, so a reload would destroy a
 * valid payment/claim link and render "not found" (#654).
 */
describe('shouldReloadOnControllerChange (#654)', () => {
  it('skips the reload on fragment-consuming link pages', () => {
    for (const path of FRAGMENT_CONSUMING_PATHS) {
      expect(shouldReloadOnControllerChange(path)).toBe(false)
    }
  })

  it('allows the reload on ordinary pages', () => {
    for (const path of ['/', '/wallet', '/explorer', '/contacts', '/node', '/docs']) {
      expect(shouldReloadOnControllerChange(path)).toBe(true)
    }
  })

  it('only matches the exact link paths, not sub-paths or look-alikes', () => {
    // Guard against accidentally suppressing reloads on unrelated routes.
    expect(shouldReloadOnControllerChange('/payment')).toBe(true)
    expect(shouldReloadOnControllerChange('/pay/extra')).toBe(true)
    expect(shouldReloadOnControllerChange('/claims')).toBe(true)
  })
})
