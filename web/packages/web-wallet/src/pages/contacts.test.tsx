/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, cleanup, fireEvent } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { ContactsPage } from './contacts'

// Mock the wallet context so the page is self-contained (#488). Each test sets
// the relevant wallet flags (isEncrypted / isLocked / hasWallet).
const useWalletMock = vi.fn()
vi.mock('../contexts/wallet', () => ({
  useWallet: () => useWalletMock(),
}))

const ASYNC_NOOP = async () => {}

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

function renderPage() {
  return render(
    <MemoryRouter>
      <ContactsPage />
    </MemoryRouter>,
  )
}

describe('ContactsPage persistence gating (#488)', () => {
  beforeEach(() => {
    cleanup()
    useWalletMock.mockReset()
  })

  it('shows a set-password hint + CTA on a plaintext (no-password) wallet', () => {
    useWalletMock.mockReturnValue(
      baseWallet({ isEncrypted: false, isLocked: false }),
    )
    renderPage()

    // The hint explains contacts require a password-protected wallet.
    expect(
      screen.getByText(/Contacts require a password-protected wallet/i),
    ).toBeDefined()
    // The CTA that opens the shared #489 set-password flow is present.
    expect(screen.getByRole('button', { name: /Set a password/i })).toBeDefined()
  })

  it('does not silently accept a save on a plaintext wallet: the Add button is disabled', () => {
    useWalletMock.mockReturnValue(
      baseWallet({ isEncrypted: false, isLocked: false }),
    )
    renderPage()

    const addButton = screen.getByRole('button', { name: /^Add$/ })
    expect((addButton as HTMLButtonElement).disabled).toBe(true)

    // Clicking the disabled Add must not open the contact editor (no Name field).
    fireEvent.click(addButton)
    expect(screen.queryByText('Add contact')).toBeNull()
  })

  it('opens the shared set-password modal when the CTA is clicked', () => {
    useWalletMock.mockReturnValue(
      baseWallet({ isEncrypted: false, isLocked: false }),
    )
    renderPage()

    fireEvent.click(screen.getByRole('button', { name: /Set a password/i }))
    // The PasswordSettingsModal (#489) renders its "Set a password" heading.
    expect(screen.getByRole('heading', { name: /Set a password/i })).toBeDefined()
  })

  it('encrypted + unlocked wallet view is unchanged: no hint, Add enabled', () => {
    useWalletMock.mockReturnValue(
      baseWallet({ isEncrypted: true, isLocked: false }),
    )
    renderPage()

    expect(
      screen.queryByText(/Contacts require a password-protected wallet/i),
    ).toBeNull()
    expect(screen.queryByText(/Wallet is locked/i)).toBeNull()

    const addButton = screen.getByRole('button', { name: /^Add$/ })
    expect((addButton as HTMLButtonElement).disabled).toBe(false)

    // Add opens the editor (proving saves are reachable for encrypted wallets).
    fireEvent.click(addButton)
    expect(screen.getByRole('heading', { name: 'Add contact' })).toBeDefined()
  })

  it('prompts to unlock on a locked wallet and gates Add', () => {
    useWalletMock.mockReturnValue(
      baseWallet({ isEncrypted: true, isLocked: true }),
    )
    renderPage()

    expect(screen.getByText(/Wallet is locked/i)).toBeDefined()
    const addButton = screen.getByRole('button', { name: /^Add$/ })
    expect((addButton as HTMLButtonElement).disabled).toBe(true)
  })
})
