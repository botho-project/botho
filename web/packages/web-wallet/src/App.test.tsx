/**
 * @vitest-environment jsdom
 *
 * App-level locale routing (issue #764, phase 1): the URL's locale prefix drives
 * which language renders and the document's `<html lang>` attribute, while the
 * unprefixed default keeps every existing absolute route working.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup, waitFor } from '@testing-library/react'
import App from './App'

// jsdom here lacks localStorage; provide a minimal mock for i18n persistence.
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
  localStorage.clear()
})

afterEach(() => {
  cleanup()
  window.history.pushState({}, '', '/')
})

describe('App locale routing', () => {
  it('renders the landing page in English at the unprefixed root', async () => {
    window.history.pushState({}, '', '/')
    render(<App />)
    expect(await screen.findByText('Quantum Era')).toBeTruthy()
    expect(document.documentElement.lang).toBe('en')
  })

  it('renders the landing page in Spanish under the /es prefix', async () => {
    window.history.pushState({}, '', '/es')
    render(<App />)
    expect(await screen.findByText('Era Cuántica')).toBeTruthy()
    await waitFor(() => expect(document.documentElement.lang).toBe('es'))
  })

  it('treats an unsupported locale segment as the default (en) locale', async () => {
    // `/fr` is not a supported locale, so it is parsed as the default locale
    // with the path unchanged — no crash, `<html lang>` stays English. (The
    // path itself has no matching route, which is normal not-found behavior;
    // the point is that an unknown prefix does not switch language or throw.)
    window.history.pushState({}, '', '/fr')
    render(<App />)
    await waitFor(() => expect(document.documentElement.lang).toBe('en'))
  })

  it('keeps existing absolute routes working under the default locale', async () => {
    window.history.pushState({}, '', '/home')
    render(<App />)
    // /home renders the landing page regardless of host (existing behavior).
    expect(await screen.findByText('Quantum Era')).toBeTruthy()
    expect(document.documentElement.lang).toBe('en')
  })
})
