/**
 * @vitest-environment jsdom
 *
 * App-level locale routing (issue #764, phase 1): the URL's locale prefix drives
 * which language renders and the document's `<html lang>` attribute, while the
 * unprefixed default keeps every existing absolute route working.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react'
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

  it('redirects the /en orphan to the unprefixed English landing page (not blank)', async () => {
    // `/en` matches no locale prefix (en is the unprefixed default) and no route,
    // so without the catch-all it would render blank (#797, item 4c). The client
    // redirect must land on `/` and show the English landing hero.
    window.history.pushState({}, '', '/en')
    render(<App />)
    expect(await screen.findByText('Quantum Era')).toBeTruthy()
    await waitFor(() => expect(window.location.pathname).toBe('/'))
    expect(document.documentElement.lang).toBe('en')
  })

  it('strips the /en prefix from a deeper orphan path (/en/wallet -> /wallet)', async () => {
    window.history.pushState({}, '', '/en/wallet')
    render(<App />)
    await waitFor(() => expect(window.location.pathname).toBe('/wallet'))
    expect(document.documentElement.lang).toBe('en')
  })

  // The switcher's <select> is identified by its locale-invariant option
  // endonyms; the landing page renders one in the header and one in the footer,
  // so take the first. (Same disambiguation pattern as the *.i18n tests.)
  function localeSwitcherSelect(): HTMLSelectElement {
    const match = screen
      .getAllByRole('combobox')
      .find((el) =>
        Array.from((el as HTMLSelectElement).options).some(
          (o) => o.textContent === 'Español',
        ),
      )
    if (!match) throw new Error('LocaleSwitcher <select> not found')
    return match as HTMLSelectElement
  }

  it('shows the active locale in the switcher under the /es prefix', async () => {
    // Regression: under a non-default locale, pages render inside
    // `<Routes location={strippedLocation}>`, so `useLocation()` in the subtree
    // reports the locale-STRIPPED path. A path-derived switcher value therefore
    // read "English" on /es while the page rendered Spanish.
    window.history.pushState({}, '', '/es')
    render(<App />)
    expect(await screen.findByText('Era Cuántica')).toBeTruthy()
    await waitFor(() => expect(localeSwitcherSelect().value).toBe('es'))
  })

  it('switches back to English from /es via the switcher', async () => {
    // Regression: with the stale path-derived value ('en'), picking English was
    // a no-op (`next === activeLocale` early-return) — the visitor was stuck.
    window.history.pushState({}, '', '/es')
    render(<App />)
    expect(await screen.findByText('Era Cuántica')).toBeTruthy()
    await waitFor(() => expect(localeSwitcherSelect().value).toBe('es'))

    fireEvent.change(localeSwitcherSelect(), { target: { value: 'en' } })

    expect(await screen.findByText('Quantum Era')).toBeTruthy()
    await waitFor(() => expect(window.location.pathname).toBe('/'))
    await waitFor(() => expect(document.documentElement.lang).toBe('en'))
  })
})
