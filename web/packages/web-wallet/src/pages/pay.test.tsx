/**
 * @vitest-environment jsdom
 *
 * Hardening of the /pay confirm screen against malicious payment-request links
 * (#588): a link's `to`/`amount`/`memo` are all attacker-controllable, so the
 * confirm screen must (a) flag a first-time recipient, (b) frame the memo as
 * coming from the requester (not Botho), and (c) never treat a large
 * link-supplied amount as pre-approved.
 */
import { StrictMode } from 'react'
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import {
  createMnemonic12,
  deriveDefaultSubaddressPublicKeys,
  formatAddress,
  BTH_MULTIPLIER,
  type Contact,
} from '@botho/core'

// Local helper: real classical keys + placeholder PQ bytes of the correct v2
// lengths (real ML-KEM/ML-DSA derivation lives in @botho/wasm-signer).
function deriveAddressFromMnemonic(mnemonic: string, network: 'mainnet' | 'testnet'): string {
  const { viewPublic, spendPublic } = deriveDefaultSubaddressPublicKeys(mnemonic, 0)
  return formatAddress(viewPublic, spendPublic, new Uint8Array(1184), new Uint8Array(1952), network)
}
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

// This environment's jsdom does not provide localStorage; mirror the mock used
// by core's wallet.test.ts. WalletGate reads it via getWalletInfo (#673).
const localStorageMock = (() => {
  let store: Record<string, string> = {}
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => { store[key] = value },
    removeItem: (key: string) => { delete store[key] },
    clear: () => { store = {} },
  }
})()
Object.defineProperty(globalThis, 'localStorage', { value: localStorageMock })

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

/**
 * Fragment-survives-double-invoke regression (#654).
 *
 * The mount effect strips the fragment (#589). Before the fix, a SECOND effect
 * invocation (React StrictMode double-invokes effects in dev; a service-worker
 * reload in prod) read the now-empty `window.location.hash` and clobbered the
 * parsed `ready` state with the "No payment request found." error — so EVERY
 * valid link rendered "not found". Capturing the fragment once at state-init
 * time makes the effect idempotent.
 */
describe('PayPage valid-link survives StrictMode double-invoke (#654)', () => {
  beforeEach(() => {
    cleanup()
    useWalletMock.mockReset()
    window.location.hash = ''
  })

  function renderPayStrict(req: PaymentRequest) {
    window.location.hash = '#' + buildPaymentRequestFragment(req)
    return render(
      <StrictMode>
        <MemoryRouter>
          <PayPage />
        </MemoryRouter>
      </StrictMode>,
    )
  }

  it('renders the pay-confirm UI (not "not found") under StrictMode', () => {
    useWalletMock.mockReturnValue(baseWallet())
    renderPayStrict({ to: RECIPIENT, amount: 1n * BTH_MULTIPLIER })

    // The parsed `ready` state must survive both effect invocations: the
    // PayConfirm UI (recipient address + amount field) renders, not the error.
    expect(screen.queryByText(/No payment request found/i)).toBeNull()
    expect(screen.getByText(/Amount \(BTH\)/i)).toBeDefined()
    expect(screen.getByText(RECIPIENT)).toBeDefined()
  })

  it('still strips the fragment from the address bar (preserves #589)', () => {
    useWalletMock.mockReturnValue(baseWallet())
    renderPayStrict({ to: RECIPIENT })

    // The requester's address must not linger in the URL after reading.
    expect(window.location.hash).toBe('')
  })

  it('still surfaces a parse error for a malformed fragment under StrictMode', () => {
    useWalletMock.mockReturnValue(baseWallet())
    window.location.hash = '#not-a-valid-payment-request-fragment'
    render(
      <StrictMode>
        <MemoryRouter>
          <PayPage />
        </MemoryRouter>
      </StrictMode>,
    )

    // A malformed fragment must still reach the invalid state (no regression).
    expect(screen.getByText(/No payment request found|not valid/i)).toBeDefined()
  })
})

/**
 * Required-password policy on the link-flow WalletGate (#672).
 *
 * The main `/wallet` setup enforces #475 (password REQUIRED, seed encrypted at
 * rest), but the /pay gate's create/import previously called
 * `createWallet`/`importWallet` with NO password — persisting a plaintext seed
 * for exactly the users whose first touch of Botho is a shared link. These
 * tests pin the gate to the same policy: the buttons stay disabled without a
 * valid password, and the password is plumbed through to the context.
 */
describe('PayPage WalletGate enforces the required-password policy (#672)', () => {
  const VALID_PASSWORD = 'correct-horse-battery'

  beforeEach(() => {
    cleanup()
    useWalletMock.mockReset()
    window.location.hash = ''
  })

  it('create flow: stays disabled without a password, passes it to createWallet', async () => {
    const createWallet = vi.fn(ASYNC_NOOP)
    useWalletMock.mockReturnValue(
      baseWallet({ hasWallet: false, address: null, createWallet }),
    )
    renderPay({ to: RECIPIENT })

    // Reveal the phrase and tick the stored-safely box — previously sufficient.
    fireEvent.click(screen.getByText(/Click to reveal/i))
    fireEvent.click(screen.getByRole('checkbox'))

    const createBtn = screen.getByRole('button', {
      name: /Create & Continue/i,
    }) as HTMLButtonElement
    expect(createBtn.disabled).toBe(true)

    // A too-short password is not enough.
    const pw = screen.getByPlaceholderText(/^Password \(min/i)
    const confirm = screen.getByPlaceholderText(/Confirm password/i)
    fireEvent.change(pw, { target: { value: 'short' } })
    fireEvent.change(confirm, { target: { value: 'short' } })
    expect(createBtn.disabled).toBe(true)

    fireEvent.change(pw, { target: { value: VALID_PASSWORD } })
    fireEvent.change(confirm, { target: { value: VALID_PASSWORD } })
    expect(createBtn.disabled).toBe(false)

    fireEvent.click(createBtn)
    await waitFor(() => expect(createWallet).toHaveBeenCalledTimes(1))
    // The seed is encrypted at rest because the password reaches the context.
    expect(createWallet).toHaveBeenCalledWith(expect.any(String), VALID_PASSWORD)
  })

  it('import flow: stays disabled without a password, passes it to importWallet', async () => {
    const importWallet = vi.fn(ASYNC_NOOP)
    useWalletMock.mockReturnValue(
      baseWallet({ hasWallet: false, address: null, importWallet }),
    )
    renderPay({ to: RECIPIENT })

    fireEvent.click(screen.getByRole('button', { name: /Import Existing/i }))
    fireEvent.change(screen.getByPlaceholderText(/recovery phrase/i), {
      target: { value: createMnemonic12() },
    })

    const importBtn = screen.getByRole('button', {
      name: /Import & Continue/i,
    }) as HTMLButtonElement
    // A valid seed alone must NOT enable the button.
    expect(importBtn.disabled).toBe(true)

    fireEvent.change(screen.getByPlaceholderText(/^Password \(min/i), {
      target: { value: VALID_PASSWORD },
    })
    fireEvent.change(screen.getByPlaceholderText(/Confirm password/i), {
      target: { value: VALID_PASSWORD },
    })
    expect(importBtn.disabled).toBe(false)

    fireEvent.click(importBtn)
    await waitFor(() => expect(importWallet).toHaveBeenCalledTimes(1))
    expect(importWallet).toHaveBeenCalledWith(expect.any(String), VALID_PASSWORD)
  })
})

/**
 * WalletGate hydration + overwrite safety (#673).
 *
 * (1) On a cold `/pay` load the wallet context hydrates AFTER the gate mounts,
 * so `mode` (initialized once) stayed 'create' even when the device holds a
 * locked wallet — showing a fresh mnemonic with no path to unlock. (2) The
 * create/import flows overwrote a stored wallet's seed with no warning.
 */
describe('PayPage WalletGate hydration + overwrite guard (#673)', () => {
  const VALID_PASSWORD = 'correct-horse-battery'

  beforeEach(() => {
    cleanup()
    useWalletMock.mockReset()
    window.location.hash = ''
    localStorageMock.clear()
  })

  it('flips to the unlock form when the context resolves to a locked wallet', () => {
    // Pre-hydration: no wallet known yet -> gate mounts in create mode.
    useWalletMock.mockReturnValue(
      baseWallet({ hasWallet: false, isLocked: false, address: null }),
    )
    const view = renderPay({ to: RECIPIENT })
    expect(screen.getByText(/Click to reveal/i)).toBeDefined()

    // Context hydrates: this device has an encrypted (locked) wallet.
    useWalletMock.mockReturnValue(baseWallet({ hasWallet: true, isLocked: true }))
    view.rerender(
      <MemoryRouter>
        <PayPage />
      </MemoryRouter>,
    )

    // The gate must now show the UNLOCK form, not a fresh mnemonic.
    expect(screen.getByPlaceholderText(/Enter password/i)).toBeDefined()
    expect(screen.queryByRole('button', { name: /Create & Continue/i })).toBeNull()
  })

  it('requires an explicit acknowledgement before creating over a stored wallet', () => {
    // A wallet already sits in storage (context not hydrated yet — the guard
    // must read storage directly).
    localStorageMock.setItem('botho-wallet-mnemonic', 'opaque-vault-blob')
    localStorageMock.setItem('botho-wallet-address', 'tbotho://1/ExistingAddr12345678')
    localStorageMock.setItem('botho-wallet-encrypted', 'true')

    const createWallet = vi.fn(ASYNC_NOOP)
    useWalletMock.mockReturnValue(
      baseWallet({ hasWallet: false, address: null, createWallet }),
    )
    renderPay({ to: RECIPIENT })

    // Complete every OTHER requirement of the create flow.
    fireEvent.click(screen.getByText(/Click to reveal/i))
    fireEvent.click(
      screen.getByRole('checkbox', { name: /written down my recovery phrase/i }),
    )
    fireEvent.change(screen.getByPlaceholderText(/^Password \(min/i), {
      target: { value: VALID_PASSWORD },
    })
    fireEvent.change(screen.getByPlaceholderText(/Confirm password/i), {
      target: { value: VALID_PASSWORD },
    })

    // The warning names the existing address and the button stays disabled.
    expect(screen.getByText(/already has a wallet/i).textContent).toContain(
      'tbotho://1/E',
    )
    const createBtn = screen.getByRole('button', {
      name: /Create & Continue/i,
    }) as HTMLButtonElement
    expect(createBtn.disabled).toBe(true)

    fireEvent.click(screen.getByRole('checkbox', { name: /already has a wallet/i }))
    expect(createBtn.disabled).toBe(false)
  })

  it('does not show the overwrite warning on a device with no stored wallet', () => {
    useWalletMock.mockReturnValue(baseWallet({ hasWallet: false, address: null }))
    renderPay({ to: RECIPIENT })
    expect(screen.queryByText(/already has a wallet/i)).toBeNull()
  })
})
