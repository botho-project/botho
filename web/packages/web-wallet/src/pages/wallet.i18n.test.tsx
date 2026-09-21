/**
 * @vitest-environment jsdom
 *
 * Locale-rendering coverage for the wallet page (issue #777, i18n phase 2).
 * With no wallet present the page renders the create/import setup view; we
 * assert its page-owned copy renders in the active locale under both the
 * default and `/es`-prefixed paths.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup, fireEvent } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'

const useWalletMock = vi.fn()
vi.mock('../contexts/wallet', () => ({
  useWallet: () => useWalletMock(),
}))
vi.mock('../contexts/network', () => ({
  useNetwork: () => ({ hasFaucet: false }),
}))
// NetworkSelector renders the network context UI (shared chrome) — stub it so
// this page-copy test stays focused on wallet-owned strings.
vi.mock('../components/NetworkSelector', () => ({
  NetworkSelector: () => null,
}))
vi.mock('../components/CustomRpcTrustGate', () => ({
  CustomRpcTrustGate: () => null,
  CustomNodeBanner: () => null,
}))

vi.mock('../components/OfflineBanner', () => ({ OfflineBanner: () => null }))
vi.mock('../components/OutstandingLinks', () => ({ OutstandingLinks: () => null }))

// Imported AFTER the mocks are registered.
import { WalletPage } from './wallet'
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

function noWallet(overrides: Record<string, unknown> = {}) {
  return {
    hasWallet: false,
    isLocked: false,
    isConnecting: false,
    address: null,
    createWallet: vi.fn(ASYNC_NOOP),
    importWallet: vi.fn(ASYNC_NOOP),
    unlockWallet: vi.fn(ASYNC_NOOP),
    ...overrides,
  }
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <WalletPage />
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

describe('WalletPage i18n', () => {
  beforeEach(() => {
    localStorage.clear()
    useWalletMock.mockReset()
    useWalletMock.mockReturnValue(noWallet())
    return i18n.changeLanguage('en')
  })

  afterEach(() => cleanup())

  it('renders English setup copy by default', () => {
    renderAt('/wallet')
    expect(screen.getByRole('heading', { name: 'Create New Wallet' })).toBeTruthy()
    expect(
      screen.getByText('Write down your recovery phrase and store it safely.'),
    ).toBeTruthy()
  })

  it('renders Spanish setup copy when the active locale is es', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/wallet')
    expect(screen.getByRole('heading', { name: 'Crear nuevo monedero' })).toBeTruthy()
    expect(
      screen.getByText('Anota tu frase de recuperación y guárdala de forma segura.'),
    ).toBeTruthy()
    // English source string must NOT leak through untranslated.
    expect(screen.queryByRole('heading', { name: 'Create New Wallet' })).toBeNull()
  })

  it('renders the locale switcher with the active locale on a default load', () => {
    renderAt('/wallet')
    // Identify the locale switcher by its locale-invariant option endonyms
    // ("English"/"Español"), so the assertion holds regardless of which locale's
    // aria-label ("Language"/"Idioma") is active (PR #798 pattern, generalized).
    const select = localeSwitcherSelect()
    expect(select.value).toBe('en')
    expect(select.options[select.selectedIndex].textContent).toBe('English')
  })

  it('renders the locale switcher label reflecting Spanish on a direct /es load', async () => {
    await i18n.changeLanguage('es')
    renderAt('/es/wallet')
    const select = localeSwitcherSelect()
    expect(select.value).toBe('es')
    expect(select.options[select.selectedIndex].textContent).toBe('Español')
  })
})


describe('WalletPage scan errors', () => {
  const refreshBalance = vi.fn(ASYNC_NOOP)
  const refreshTransactions = vi.fn(ASYNC_NOOP)

  beforeEach(async () => {
    refreshBalance.mockClear()
    refreshTransactions.mockClear()
    await i18n.changeLanguage('en')
    useWalletMock.mockReturnValue(noWallet({
      hasWallet: true,
      isConnected: true,
      isEncrypted: true,
      balance: null,
      transactions: [],
      balanceUnavailable: true,
      historyUnavailable: true,
      contacts: [],
      autoLockMinutes: 5,
      refreshBalance,
      refreshTransactions,
    }))
  })
  afterEach(() => cleanup())

  it.each(['en', 'es', 'zh'])('shows localized unavailable states and distinct retries in %s', async locale => {
    await i18n.changeLanguage(locale)
    renderAt('/wallet')
    expect(screen.getByText(i18n.t('dashboard.balanceUnavailable', { ns: 'wallet' }))).toBeTruthy()
    expect(screen.getByText(i18n.t('dashboard.historyUnavailable', { ns: 'wallet' }))).toBeTruthy()
    expect(screen.queryByText('No transactions yet')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: i18n.t('dashboard.retryBalance', { ns: 'wallet' }) }))
    fireEvent.click(screen.getByRole('button', { name: i18n.t('dashboard.retryHistory', { ns: 'wallet' }) }))
    expect(refreshBalance).toHaveBeenCalledOnce()
    expect(refreshTransactions).toHaveBeenCalledOnce()
  })

  it('returns to the normal empty state after a successful empty scan', () => {
    const state = useWalletMock()
    const view = renderAt('/wallet')
    useWalletMock.mockReturnValue({
      ...state,
      balance: { available: 0n, pending: 0n, total: 0n },
      balanceUnavailable: false,
      historyUnavailable: false,
    })
    view.rerender(<MemoryRouter initialEntries={['/wallet']}><WalletPage /></MemoryRouter>)
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.getByText('No transactions yet')).toBeTruthy()
  })
})
