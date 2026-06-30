/**
 * @vitest-environment jsdom
 *
 * Hardening of the /pay confirm screen against malicious payment-request links
 * (#588): a link's `to`/`amount`/`memo` are all attacker-controllable, so the
 * confirm screen must (a) flag a first-time recipient, (b) frame the memo as
 * coming from the requester (not Botho), and (c) never treat a large
 * link-supplied amount as pre-approved.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, cleanup } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import {
  createMnemonic12,
  deriveAddressFromMnemonic,
  BTH_MULTIPLIER,
  type Contact,
} from '@botho/core'
import { buildPaymentRequestFragment, type PaymentRequest } from '../lib/payment-request'
import { PayPage } from './pay'

// Mock the wallet + network contexts so the page is self-contained.
const useWalletMock = vi.fn()
vi.mock('../contexts/wallet', () => ({
  useWallet: () => useWalletMock(),
}))
vi.mock('../contexts/network', () => ({
  useNetwork: () => ({ network: { explorerUrl: 'https://explorer.test' } }),
}))

const ASYNC_NOOP = async () => {}

// Two distinct, structurally valid testnet addresses.
const RECIPIENT = deriveAddressFromMnemonic(createMnemonic12(), 'testnet')
const OWN = deriveAddressFromMnemonic(createMnemonic12(), 'testnet')

function makeContact(address: string, name: string): Contact {
  return {
    id: 'c1',
    name,
    address,
    createdAt: 0 as Contact['createdAt'],
    updatedAt: 0 as Contact['updatedAt'],
    txCount: 1,
  }
}

function baseWallet(overrides: Record<string, unknown> = {}) {
  return {
    hasWallet: true,
    isLocked: false,
    address: OWN,
    contacts: [] as Contact[],
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

function renderPay(req: PaymentRequest) {
  window.location.hash = '#' + buildPaymentRequestFragment(req)
  return render(
    <MemoryRouter>
      <PayPage />
    </MemoryRouter>,
  )
}

describe('PayPage recipient + memo + amount hardening (#588)', () => {
  beforeEach(() => {
    cleanup()
    useWalletMock.mockReset()
    window.location.hash = ''
  })

  it('shows the unknown-recipient badge when the address is not a contact', () => {
    useWalletMock.mockReturnValue(baseWallet({ contacts: [] }))
    renderPay({ to: RECIPIENT })

    expect(screen.getByText(/You have not paid this address before/i)).toBeDefined()
    // The reassuring "in your contacts" line must NOT appear for a stranger.
    expect(screen.queryByText(/in your contacts/i)).toBeNull()
  })

  it('does NOT show the unknown-recipient badge for a saved contact', () => {
    useWalletMock.mockReturnValue(
      baseWallet({ contacts: [makeContact(RECIPIENT, 'Alice')] }),
    )
    renderPay({ to: RECIPIENT })

    expect(screen.queryByText(/You have not paid this address before/i)).toBeNull()
    expect(screen.getByText(/in your contacts/i)).toBeDefined()
  })

  it('matches contacts case-insensitively (no false stranger warning)', () => {
    useWalletMock.mockReturnValue(
      baseWallet({ contacts: [makeContact(RECIPIENT.toUpperCase(), 'Bob')] }),
    )
    renderPay({ to: RECIPIENT })

    expect(screen.queryByText(/You have not paid this address before/i)).toBeNull()
  })

  it('frames the memo as coming from the requester, not Botho', () => {
    useWalletMock.mockReturnValue(baseWallet())
    renderPay({ to: RECIPIENT, memo: 'Verified by Botho — safe to send' })

    // The provenance label distances the app from attacker-supplied text.
    expect(screen.getByText(/Note from the requester/i)).toBeDefined()
    expect(screen.getByText(/not from Botho/i)).toBeDefined()
    // The attacker text itself still renders (as untrusted content).
    expect(screen.getByText(/Verified by Botho — safe to send/i)).toBeDefined()
  })

  it('requires an explicit acknowledgement for a large link-supplied amount', () => {
    useWalletMock.mockReturnValue(baseWallet())
    renderPay({ to: RECIPIENT, amount: 250n * BTH_MULTIPLIER })

    // A large prefilled amount surfaces an acknowledgement, and Pay stays
    // disabled until it is ticked (link amount is never pre-approved).
    expect(screen.getByText(/This link is requesting a large amount/i)).toBeDefined()
    const payButton = screen.getByRole('button', { name: /Pay/i }) as HTMLButtonElement
    expect(payButton.disabled).toBe(true)
  })

  it('does not gate a small link-supplied amount behind acknowledgement', () => {
    useWalletMock.mockReturnValue(baseWallet())
    renderPay({ to: RECIPIENT, amount: 1n * BTH_MULTIPLIER })

    expect(screen.queryByText(/This link is requesting a large amount/i)).toBeNull()
  })
})
