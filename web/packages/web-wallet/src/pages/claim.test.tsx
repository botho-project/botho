/**
 * @vitest-environment jsdom
 */
import { StrictMode } from 'react'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { buildClaimLink, createClaimLinkMnemonic } from '@botho/core'

/**
 * Unfurl-safety regression test (#589).
 *
 * The security-load-bearing invariant that makes sharing a claim link over an
 * E2E messenger safe: the `/claim` page performs NO network fetch keyed on the
 * bearer secret until the recipient EXPLICITLY acts. So a messenger
 * link-preview / unfurl fetch — or any incidental load of the page — can never
 * trigger an on-chain scan or claim, nor leak the secret.
 *
 * We mock the transaction ops (`scanEphemeral` / `sweepEphemeral`) — the ONLY
 * functions that touch the node with the secret — and assert they are not
 * invoked on mount, only after the explicit "Reveal" action.
 */

const scanEphemeralMock = vi.fn()
const sweepEphemeralMock = vi.fn()
vi.mock('../lib/claim-link-ops', () => ({
  scanEphemeral: (...args: unknown[]) => scanEphemeralMock(...args),
  sweepEphemeral: (...args: unknown[]) => sweepEphemeralMock(...args),
}))

// Fake adapter: every method is a spy so we can assert the page makes NO
// network call before explicit user action.
const adapterSpies = {
  isConnected: vi.fn(() => true),
  getBlockHeight: vi.fn(async () => 100),
  estimateFee: vi.fn(async () => ({ fee: 0n, clusterFactorDisplay: '1.00x' })),
  getRawOutputs: vi.fn(async () => []),
  areKeyImagesSpent: vi.fn(async () => []),
  submitTransaction: vi.fn(async () => ({ success: true, txHash: 'tx' })),
  onNewBlock: vi.fn(() => () => {}),
}
vi.mock('../contexts/wallet', () => ({
  useAdapter: () => adapterSpies,
}))
vi.mock('../contexts/network', () => ({
  useNetwork: () => ({ network: { explorerUrl: null } }),
}))

// Imported AFTER the mocks are registered.
import { ClaimPage } from './claim'

function setClaimHash(): string {
  const mnemonic = createClaimLinkMnemonic()
  const url = buildClaimLink('https://wallet.botho.io', mnemonic)
  const fragment = url.slice(url.indexOf('#'))
  window.location.hash = fragment
  return fragment
}

function renderClaim() {
  return render(
    <MemoryRouter>
      <ClaimPage />
    </MemoryRouter>,
  )
}

describe('ClaimPage unfurl-safety (#589): no network fetch before user action', () => {
  beforeEach(() => {
    cleanup()
    scanEphemeralMock.mockReset()
    scanEphemeralMock.mockResolvedValue({ gross: 5_000_000_000_000n, fee: 100_000_000n, net: 4_900_000_000_000n })
    sweepEphemeralMock.mockReset()
    for (const spy of Object.values(adapterSpies)) spy.mockClear()
    window.history.replaceState(null, '', '/claim')
  })

  afterEach(() => {
    window.location.hash = ''
  })

  it('does NOT scan/claim or hit the node on mount with a valid link in the fragment', async () => {
    setClaimHash()
    renderClaim()

    // The page parsed the fragment locally and is waiting for explicit action.
    await screen.findByRole('button', { name: /reveal/i })

    // The security invariant: nothing reached the node, and no scan/claim ran.
    expect(scanEphemeralMock).not.toHaveBeenCalled()
    expect(sweepEphemeralMock).not.toHaveBeenCalled()
    expect(adapterSpies.getRawOutputs).not.toHaveBeenCalled()
    expect(adapterSpies.areKeyImagesSpent).not.toHaveBeenCalled()
    expect(adapterSpies.submitTransaction).not.toHaveBeenCalled()
  })

  it('strips the secret from the URL on mount (fragment never lingers)', async () => {
    setClaimHash()
    renderClaim()
    await screen.findByRole('button', { name: /reveal/i })
    // The fragment has been cleared from the address bar by replaceState.
    expect(window.location.hash).toBe('')
  })

  it('only scans AFTER the explicit Reveal action', async () => {
    setClaimHash()
    renderClaim()

    const revealBtn = await screen.findByRole('button', { name: /reveal/i })
    expect(scanEphemeralMock).not.toHaveBeenCalled()

    fireEvent.click(revealBtn)

    // The first network-touching call happens only now, keyed on the secret.
    await waitFor(() => expect(scanEphemeralMock).toHaveBeenCalledTimes(1))
  })

  it('shows an invalid state (no scan) when there is no fragment at all', async () => {
    window.location.hash = ''
    renderClaim()
    await screen.findByText(/no claim link found/i)
    expect(scanEphemeralMock).not.toHaveBeenCalled()
  })
})

/**
 * Fragment-survives-double-invoke regression (#654).
 *
 * The mount effect strips the fragment (#589). Before the fix, a SECOND effect
 * invocation (React StrictMode double-invokes effects in dev; a service-worker
 * reload in prod) read the now-empty `window.location.hash` and clobbered the
 * parsed `ready` state with the "No claim link found." error — so EVERY valid
 * claim link rendered "not found". Capturing the fragment once at state-init
 * time makes the effect idempotent.
 */
describe('ClaimPage valid-link survives StrictMode double-invoke (#654)', () => {
  beforeEach(() => {
    cleanup()
    scanEphemeralMock.mockReset()
    sweepEphemeralMock.mockReset()
    for (const spy of Object.values(adapterSpies)) spy.mockClear()
    window.history.replaceState(null, '', '/claim')
  })

  afterEach(() => {
    window.location.hash = ''
  })

  function renderClaimStrict() {
    return render(
      <StrictMode>
        <MemoryRouter>
          <ClaimPage />
        </MemoryRouter>
      </StrictMode>,
    )
  }

  it('reaches the ready "Reveal" state (not "not found") under StrictMode', async () => {
    setClaimHash()
    renderClaimStrict()

    // The parsed `ready` state must survive both effect invocations.
    await screen.findByRole('button', { name: /reveal/i })
    expect(screen.queryByText(/no claim link found/i)).toBeNull()
    // Still unfurl-safe: no network touched just by reaching ready.
    expect(scanEphemeralMock).not.toHaveBeenCalled()
  })

  it('still strips the secret from the address bar (preserves #589)', async () => {
    setClaimHash()
    renderClaimStrict()
    await screen.findByRole('button', { name: /reveal/i })
    expect(window.location.hash).toBe('')
  })

  it('still surfaces an invalid state for a malformed fragment under StrictMode', async () => {
    window.location.hash = '#v1.not-valid-base58-secret'
    renderClaimStrict()
    // A malformed fragment must still reach the invalid state (no regression).
    await screen.findByText(/no claim link found|not valid/i)
  })
})

// This environment's jsdom does not provide localStorage; mirror the mock used
// by core's wallet.test.ts so the REAL saveWallet can persist through it.
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

/**
 * Required-password policy for the in-flow "Create one" wallet (#672).
 *
 * `handleSweep` previously persisted the freshly created wallet with
 * `saveWallet(mnemonic)` — no password — writing the raw 12-word seed to
 * localStorage in plaintext (`botho-wallet-encrypted = 'false'`), bypassing the
 * #475 policy for exactly the users whose first wallet arrives via a claim
 * link. This locks in: no claim while the password is missing, and the
 * persisted seed is an encrypted vault blob.
 */
describe('ClaimPage in-flow wallet create persists ENCRYPTED (#672)', () => {
  const VALID_PASSWORD = 'correct-horse-battery'

  beforeEach(() => {
    cleanup()
    scanEphemeralMock.mockReset()
    scanEphemeralMock.mockResolvedValue({
      gross: 5_000_000_000_000n,
      fee: 100_000_000n,
      net: 4_900_000_000_000n,
    })
    sweepEphemeralMock.mockReset()
    sweepEphemeralMock.mockResolvedValue({ txHash: 'tx' })
    for (const spy of Object.values(adapterSpies)) spy.mockClear()
    window.history.replaceState(null, '', '/claim')
    localStorageMock.clear()
  })

  afterEach(() => {
    window.location.hash = ''
    localStorageMock.clear()
  })

  it('requires a password before claiming and stores a vault blob, not plaintext', async () => {
    setClaimHash()
    renderClaim()

    fireEvent.click(await screen.findByRole('button', { name: /reveal/i }))
    fireEvent.click(await screen.findByRole('button', { name: /create one/i }))

    // Destination is pre-filled from the new wallet, but there is no password
    // yet — the Claim button must stay disabled.
    const claimBtn = screen.getByRole('button', { name: /^Claim / }) as HTMLButtonElement
    expect(claimBtn.disabled).toBe(true)

    fireEvent.change(screen.getByPlaceholderText(/^Password \(min/i), {
      target: { value: VALID_PASSWORD },
    })
    fireEvent.change(screen.getByPlaceholderText(/Confirm password/i), {
      target: { value: VALID_PASSWORD },
    })
    await waitFor(() => expect(claimBtn.disabled).toBe(false))

    fireEvent.click(claimBtn)
    // Real saveWallet + PBKDF2 (600k iterations) runs here — allow extra time.
    await screen.findByText(/on their way to your address/i, {}, { timeout: 20000 })

    expect(localStorageMock.getItem('botho-wallet-encrypted')).toBe('true')
    const stored = localStorageMock.getItem('botho-wallet-mnemonic')
    expect(stored).not.toBeNull()
    // An encrypted vault blob is one opaque token — never a 12-word phrase.
    expect(stored!.trim().split(/\s+/).length).toBe(1)
  }, 30000)
})

/**
 * Overwrite guard for the in-flow "Create one" wallet (#673): claiming with a
 * freshly created wallet on a device that ALREADY stores a wallet replaces the
 * stored seed (funds loss). The claim must stay blocked until the recipient
 * explicitly acknowledges the replacement, which names the existing address.
 */
describe('ClaimPage in-flow create requires overwrite acknowledgement (#673)', () => {
  const VALID_PASSWORD = 'correct-horse-battery'

  beforeEach(() => {
    cleanup()
    scanEphemeralMock.mockReset()
    scanEphemeralMock.mockResolvedValue({
      gross: 5_000_000_000_000n,
      fee: 100_000_000n,
      net: 4_900_000_000_000n,
    })
    sweepEphemeralMock.mockReset()
    for (const spy of Object.values(adapterSpies)) spy.mockClear()
    window.history.replaceState(null, '', '/claim')
    localStorageMock.clear()
  })

  afterEach(() => {
    window.location.hash = ''
    localStorageMock.clear()
  })

  it('blocks the claim until the stored-wallet replacement is acknowledged', async () => {
    // A wallet already sits in storage BEFORE the page loads.
    localStorageMock.setItem('botho-wallet-mnemonic', 'opaque-vault-blob')
    localStorageMock.setItem('botho-wallet-address', 'tbotho://1/ExistingAddr12345678')
    localStorageMock.setItem('botho-wallet-encrypted', 'true')

    setClaimHash()
    renderClaim()

    fireEvent.click(await screen.findByRole('button', { name: /reveal/i }))
    fireEvent.click(await screen.findByRole('button', { name: /create one/i }))

    // Password valid, destination pre-filled — only the overwrite ack missing.
    fireEvent.change(screen.getByPlaceholderText(/^Password \(min/i), {
      target: { value: VALID_PASSWORD },
    })
    fireEvent.change(screen.getByPlaceholderText(/Confirm password/i), {
      target: { value: VALID_PASSWORD },
    })

    const claimBtn = screen.getByRole('button', { name: /^Claim / }) as HTMLButtonElement
    expect(screen.getByText(/already has a wallet/i).textContent).toContain(
      'tbotho://1/E',
    )
    expect(claimBtn.disabled).toBe(true)

    fireEvent.click(screen.getByRole('checkbox', { name: /already has a wallet/i }))
    await waitFor(() => expect(claimBtn.disabled).toBe(false))
  })

  it('shows no overwrite warning when the device has no stored wallet', async () => {
    setClaimHash()
    renderClaim()
    fireEvent.click(await screen.findByRole('button', { name: /reveal/i }))
    fireEvent.click(await screen.findByRole('button', { name: /create one/i }))
    expect(screen.queryByText(/already has a wallet/i)).toBeNull()
  })
})
