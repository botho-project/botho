/**
 * @vitest-environment jsdom
 *
 * Locale-rendering coverage for the claim page (issue #777, i18n phase 2).
 * With no claim-link fragment the page lands in the deterministic "invalid"
 * state; we assert its page-owned copy renders in the active locale under both
 * the default and `/es`-prefixed paths.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'

vi.mock('../lib/claim-link-ops', () => ({
  scanEphemeral: vi.fn(),
  sweepEphemeral: vi.fn(),
}))
vi.mock('../contexts/wallet', () => ({
  useAdapter: () => ({ onNewBlock: () => () => {} }),
}))
vi.mock('../contexts/network', () => ({
  useNetwork: () => ({ network: { explorerUrl: null } }),
}))

// Imported AFTER the mocks are registered.
import { ClaimPage } from './claim'
import i18n from '../lib/i18n'

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

function renderAt(path: string) {
  window.location.hash = ''
  return render(
    <MemoryRouter initialEntries={[path]}>
      <ClaimPage />
    </MemoryRouter>,
  )
}

describe('ClaimPage i18n', () => {
  beforeEach(() => {
    localStorage.clear()
    return i18n.changeLanguage('en')
  })

  afterEach(() => cleanup())

  it('renders English page copy by default', () => {
    renderAt('/claim')
    expect(screen.getByRole('heading', { name: 'Claim Your BTH' })).toBeTruthy()
    expect(
      screen.getByText('No claim link found. The link should look like .../claim#…'),
    ).toBeTruthy()
  })

  it('renders Spanish page copy when the active locale is es', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/claim')
    expect(screen.getByRole('heading', { name: 'Reclama tus BTH' })).toBeTruthy()
    expect(
      screen.getByText(
        'No se encontró ningún enlace de reclamación. El enlace debería verse como .../claim#…',
      ),
    ).toBeTruthy()
    // English source string must NOT leak through untranslated.
    expect(screen.queryByRole('heading', { name: 'Claim Your BTH' })).toBeNull()
  })
})
