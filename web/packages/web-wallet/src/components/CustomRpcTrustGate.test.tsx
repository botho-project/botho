/**
 * @vitest-environment jsdom
 *
 * Component-level tests for the custom-RPC trust gate + banner (#587).
 *
 * These render the gate/banner in isolation with a mocked network context to
 * assert the user-facing copy and the wiring of the accept / decline / revert
 * actions. The end-to-end "a `?rpc=` link never silently switches the active
 * node" guarantee is covered against the REAL provider in
 * `../contexts/network-deep-link.test.tsx`.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, cleanup, fireEvent, act } from '@testing-library/react'

const useNetworkMock = vi.fn()
vi.mock('../contexts/network', () => ({ useNetwork: () => useNetworkMock() }))

import { CustomRpcTrustGate, CustomNodeBanner } from './CustomRpcTrustGate'

describe('CustomRpcTrustGate component (#587)', () => {
  beforeEach(() => {
    cleanup()
    useNetworkMock.mockReset()
  })

  it('renders nothing when there is no pending link', () => {
    useNetworkMock.mockReturnValue({ pendingRpcLink: null })
    render(<CustomRpcTrustGate />)
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('shows the explicit trust message naming the host, with a decline default', () => {
    const decline = vi.fn()
    useNetworkMock.mockReturnValue({
      pendingRpcLink: { rpcUrl: 'https://evil.example/rpc', host: 'evil.example', trust: 'unknown' },
      acceptPendingRpcLink: vi.fn(),
      declinePendingRpcLink: decline,
    })
    render(<CustomRpcTrustGate />)

    expect(screen.getByRole('dialog')).toBeTruthy()
    expect(screen.getByText(/point your wallet at/i)).toBeTruthy()
    expect(screen.getByText('evil.example')).toBeTruthy()
    expect(screen.getByText(/balances, confirmations, and transaction relay/i)).toBeTruthy()
    // Unknown host => stronger warning.
    expect(screen.getByText(/Unknown host/i)).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: /decline \(keep current node\)/i }))
    expect(decline).toHaveBeenCalled()
  })

  it('shows a softer hint for a known Botho-operated host', () => {
    useNetworkMock.mockReturnValue({
      pendingRpcLink: {
        rpcUrl: 'https://rig.testnet.botho.io/rpc',
        host: 'rig.testnet.botho.io',
        trust: 'known',
      },
      acceptPendingRpcLink: vi.fn(),
      declinePendingRpcLink: vi.fn(),
    })
    render(<CustomRpcTrustGate />)
    expect(screen.getByText(/Botho-operated host/i)).toBeTruthy()
    expect(screen.queryByText(/Unknown host/i)).toBeNull()
  })

  it('calls acceptPendingRpcLink when the user trusts & connects', async () => {
    const accept = vi.fn().mockResolvedValue(true)
    useNetworkMock.mockReturnValue({
      pendingRpcLink: {
        rpcUrl: 'https://rig.testnet.botho.io/rpc',
        host: 'rig.testnet.botho.io',
        trust: 'known',
      },
      acceptPendingRpcLink: accept,
      declinePendingRpcLink: vi.fn(),
    })
    render(<CustomRpcTrustGate />)
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /trust & connect/i }))
    })
    expect(accept).toHaveBeenCalled()
  })
})

describe('CustomNodeBanner component (#587)', () => {
  beforeEach(() => {
    cleanup()
    useNetworkMock.mockReset()
  })

  it('is hidden when not on a link-supplied node', () => {
    useNetworkMock.mockReturnValue({ customNodeFromLink: null })
    render(<CustomNodeBanner />)
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('names the custom host and offers a one-tap revert', () => {
    const revert = vi.fn()
    useNetworkMock.mockReturnValue({
      customNodeFromLink: 'rig-x.testnet.botho.io',
      revertCustomNode: revert,
    })
    render(<CustomNodeBanner />)
    expect(screen.getByRole('status')).toBeTruthy()
    expect(screen.getByText(/from a link/i)).toBeTruthy()
    expect(screen.getByText('rig-x.testnet.botho.io')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: /switch back/i }))
    expect(revert).toHaveBeenCalled()
  })
})
