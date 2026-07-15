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
// The in-flow "Create one" wallet derives a v2 address via wasm; stub it so the
// test never loads the (unbuilt) wasm artifact. Deterministic per mnemonic.
// The sweep path validates the destination via `isValidAddress`, which requires
// a full 3200-byte v2 body, so the stub returns a REAL golden v2 address (the
// node-derived address for the canonical BIP39 mnemonic).
const { GOLDEN_V2_ADDRESS } = vi.hoisted(() => ({
  GOLDEN_V2_ADDRESS:
    'tbotho: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',
}))
vi.mock('@botho/wasm-signer', () => ({
  deriveV2Address: vi.fn(async () => GOLDEN_V2_ADDRESS),
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
