/**
 * @vitest-environment jsdom
 *
 * Locale-rendering coverage for the contacts page (issue #777, i18n phase 2).
 * Renders the page under both the default (`/contacts`) and `/es`-prefixed
 * (`/es/contacts`) paths and asserts the page-owned copy renders in the active
 * locale. Copy owned by shared `@botho/ui` components is out of scope.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'

const useWalletMock = vi.fn()
vi.mock('../contexts/wallet', () => ({
  useWallet: () => useWalletMock(),
}))

// Imported AFTER the mock is registered.
import { ContactsPage } from './contacts'
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
    contacts: [],
    addContact: vi.fn(ASYNC_NOOP),
    updateContact: vi.fn(ASYNC_NOOP),
    deleteContact: vi.fn(ASYNC_NOOP),
    hasWallet: true,
    isEncrypted: true,
    isLocked: false,
    setPassword: vi.fn(ASYNC_NOOP),
    changePassword: vi.fn(ASYNC_NOOP),
    ...overrides,
  }
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <ContactsPage />
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

describe('ContactsPage i18n', () => {
  beforeEach(() => {
    localStorage.clear()
    useWalletMock.mockReset()
    useWalletMock.mockReturnValue(baseWallet())
    return i18n.changeLanguage('en')
  })

  afterEach(() => cleanup())

  it('renders English page copy by default', () => {
    renderAt('/contacts')
    expect(screen.getByRole('heading', { name: 'Contacts' })).toBeTruthy()
    expect(screen.getByPlaceholderText('Search by name or address…')).toBeTruthy()
    expect(
      screen.getByText('No contacts yet. Addresses you pay will appear here, ready to label.'),
    ).toBeTruthy()
  })

  it('renders Spanish page copy when the active locale is es', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/contacts')
    expect(screen.getByRole('heading', { name: 'Contactos' })).toBeTruthy()
    expect(screen.getByPlaceholderText('Buscar por nombre o dirección…')).toBeTruthy()
    // English source string must NOT leak through untranslated.
    expect(screen.queryByRole('heading', { name: 'Contacts' })).toBeNull()
  })

  it('renders the locale switcher with the active locale on a default load', () => {
    renderAt('/contacts')
    const select = localeSwitcherSelect()
    expect(select.value).toBe('en')
    expect(select.options[select.selectedIndex].textContent).toBe('English')
  })

  it('renders the locale switcher label reflecting Spanish on a direct /es load', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/contacts')
    const select = localeSwitcherSelect()
    expect(select.value).toBe('es')
    expect(select.options[select.selectedIndex].textContent).toBe('Español')
  })
})
