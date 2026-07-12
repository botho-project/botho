/**
 * @vitest-environment jsdom
 *
 * Locale-rendering coverage for the wallet page (issue #777, i18n phase 2).
 * With no wallet present the page renders the create/import setup view; we
 * assert its page-owned copy renders in the active locale under both the
 * default and `/es`-prefixed paths.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'
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
})
