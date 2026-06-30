/**
 * @vitest-environment jsdom
 */
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
  estimateFee: vi.fn(async () => 0n),
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
