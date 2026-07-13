/**
 * @vitest-environment jsdom
 *
 * Locale-rendering coverage for the pay page (issue #777, i18n phase 2).
 * With no payment-request fragment the page lands in the deterministic
 * "invalid" state; we assert its page-owned copy renders in the active locale
 * under both the default and `/es`-prefixed paths.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'

const useWalletMock = vi.fn()
vi.mock('../contexts/wallet', () => ({
  useWallet: () => useWalletMock(),
}))
vi.mock('../contexts/network', () => ({
  useNetwork: () => ({ network: { explorerUrl: 'https://explorer.test' } }),
}))

// Imported AFTER the mocks are registered.
import { PayPage } from './pay'
import i18n from '../lib/i18n'

const ASYNC_NOOP = async () => {}

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

function baseWallet(overrides: Record<string, unknown> = {}) {
  return {
    hasWallet: true,
    isLocked: false,
    address: null,
    contacts: [],
    addContact: vi.fn(ASYNC_NOOP),
    send: vi.fn(async () => 'deadbeef'),
    refreshBalance: vi.fn(ASYNC_NOOP),
    refreshTransactions: vi.fn(ASYNC_NOOP),
    createWallet: vi.fn(ASYNC_NOOP),
    importWallet: vi.fn(ASYNC_NOOP),
    unlockWallet: vi.fn(ASYNC_NOOP),
    balance: null,
    ...overrides,
  }
}

function renderAt(path: string) {
  window.location.hash = ''
  return render(
    <MemoryRouter initialEntries={[path]}>
      <PayPage />
    </MemoryRouter>,
  )
}

// The LocaleSwitcher's <select> is uniquely identified by its locale-invariant
// option endonyms ("English"/"Español"); this avoids depending on the active
// locale's aria-label and tolerates other <select>s on the page.
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

describe('PayPage i18n', () => {
  beforeEach(() => {
    localStorage.clear()
    useWalletMock.mockReset()
    useWalletMock.mockReturnValue(baseWallet())
    return i18n.changeLanguage('en')
  })

  afterEach(() => cleanup())

  it('renders English page copy by default', () => {
    renderAt('/pay')
    expect(screen.getByRole('heading', { name: 'Send a Payment' })).toBeTruthy()
    expect(
      screen.getByText('No payment request found. The link should look like .../pay#…'),
    ).toBeTruthy()
  })

  it('renders Spanish page copy when the active locale is es', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/pay')
    expect(screen.getByRole('heading', { name: 'Enviar un pago' })).toBeTruthy()
    expect(
      screen.getByText(
        'No se encontró ninguna solicitud de pago. El enlace debería verse como .../pay#…',
      ),
    ).toBeTruthy()
    // English source string must NOT leak through untranslated.
    expect(screen.queryByRole('heading', { name: 'Send a Payment' })).toBeNull()
  })

  it('renders the locale switcher with the active locale on a default load', () => {
    renderAt('/pay')
    const select = localeSwitcherSelect()
    expect(select.value).toBe('en')
    expect(select.options[select.selectedIndex].textContent).toBe('English')
  })

  it('renders the locale switcher label reflecting Spanish on a direct /es load', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/pay')
    const select = localeSwitcherSelect()
    expect(select.value).toBe('es')
    expect(select.options[select.selectedIndex].textContent).toBe('Español')
  })
})
