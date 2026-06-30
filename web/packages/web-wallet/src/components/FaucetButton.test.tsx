/**
 * @vitest-environment jsdom
 *
 * Tests for the faucet button's cold-start handling (issue #583):
 *   - a structured `warmingUp` result renders a friendly "warming up" state
 *     (NOT a scary error), with the have/need decoy counts.
 *   - a genuine JSON-RPC error still renders as an error (no over-suppression).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, cleanup, fireEvent } from '@testing-library/react'
import { FaucetButton } from './FaucetButton'

// --- Mock the two contexts the button reads --------------------------------
const useNetworkMock = vi.fn()
const useWalletMock = vi.fn()
vi.mock('../contexts/network', () => ({ useNetwork: () => useNetworkMock() }))
vi.mock('../contexts/wallet', () => ({ useWallet: () => useWalletMock() }))

function setupContexts() {
  useNetworkMock.mockReturnValue({
    network: {
      faucetEndpoint: 'https://faucet.example/rpc',
      explorerUrl: 'https://explorer.example',
    },
    hasFaucet: true,
  })
  useWalletMock.mockReturnValue({ address: 'view:abc\nspend:def' })
}

function mockFetchJson(json: unknown) {
  globalThis.fetch = vi.fn().mockResolvedValue({
    json: async () => json,
  }) as unknown as typeof fetch
}

describe('FaucetButton cold-start warming-up handling', () => {
  beforeEach(() => {
    cleanup()
    useNetworkMock.mockReset()
    useWalletMock.mockReset()
    setupContexts()
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('renders a friendly "warming up" state for a structured warmingUp result', async () => {
    mockFetchJson({ result: { warmingUp: true, haveDecoys: 4, needDecoys: 19 } })

    render(<FaucetButton />)
    fireEvent.click(screen.getByRole('button', { name: /request bth/i }))

    expect(await screen.findByText(/warming up/i)).toBeTruthy()
    // Shows progress toward the needed anonymity set.
    expect(screen.getByText(/4\/19 ready/)).toBeTruthy()
    // It must NOT be presented as a raw decoy error.
    expect(screen.queryByText(/insufficient decoy/i)).toBeNull()
  })

  it('still surfaces a genuine faucet error (no over-suppression)', async () => {
    mockFetchJson({ error: { code: -32000, message: 'Faucet has insufficient balance' } })

    render(<FaucetButton />)
    fireEvent.click(screen.getByRole('button', { name: /request bth/i }))

    expect(await screen.findByText(/insufficient balance/i)).toBeTruthy()
    expect(screen.queryByText(/warming up/i)).toBeNull()
  })
})
