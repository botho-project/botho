/**
 * @vitest-environment jsdom
 *
 * Standalone operator-entry shell (#772, §8.3.1 option (a)). The split operator
 * build boots `OperatorApp` instead of the full SPA `App`; this asserts it
 * mounts the operator dashboard at `/operator` and at a locale-prefixed
 * `/:locale/operator`, matching the `LocaleRoutes` contract of the main SPA.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup, waitFor } from '@testing-library/react'
import OperatorApp from './OperatorApp'

// jsdom lacks localStorage; provide a minimal mock for i18n persistence.
const localStorageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => {
      store[key] = value
    },
    removeItem: (key: string) => {
      delete store[key]
    },
    clear: () => {
      store = {}
    },
  }
})()
Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock })

beforeEach(() => {
  localStorageMock.clear()
})

afterEach(() => {
  cleanup()
})

function renderAt(path: string) {
  window.history.pushState({}, '', path)
  return render(<OperatorApp />)
}

// `getByRole` throws if the element is absent, so its mere resolution is the
// assertion; this project does not wire up jest-dom matchers.
const fleetTab = () => screen.getByRole('tab', { name: /fleet/i })

describe('OperatorApp standalone shell', () => {
  it('mounts the operator dashboard at /operator', async () => {
    renderAt('/operator')
    // The operator header renders the tablist with a Fleet tab.
    await waitFor(() => fleetTab())
    expect(document.documentElement.lang).toBe('en')
  })

  it('mounts the operator dashboard under a non-default locale prefix (/es/operator)', async () => {
    renderAt('/es/operator')
    await waitFor(() => fleetTab())
    // Locale side effects mirror the main SPA's LocaleRoutes.
    await waitFor(() => expect(document.documentElement.lang).toBe('es'))
  })

  it('redirects an unknown in-document path to /operator', async () => {
    renderAt('/somewhere-else')
    await waitFor(() => fleetTab())
    expect(window.location.pathname).toBe('/operator')
  })
})
