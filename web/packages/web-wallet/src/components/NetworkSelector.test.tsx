/**
 * @vitest-environment jsdom
 *
 * Interaction coverage for the NetworkSelector custom-endpoint UI (#806):
 *   - the collapsed trigger shows the truncated active custom host (not the
 *     literal "Custom RPC") when a custom endpoint is active,
 *   - manual entry routes the pasted endpoint through `setCustomEndpoint` and
 *     surfaces its validationError on rejection.
 *
 * The context is mocked (as in OfflineBanner.test.tsx) so this suite tests the
 * component's wiring, not the validation internals — those live in
 * `config/networks.test.ts`.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react'
import type { NetworkConfig, NodeHealth } from '../config/networks'
import { INGRESS_NODES } from '../config/networks'
import { NetworkSelector } from './NetworkSelector'

const useNetworkMock = vi.fn()
vi.mock('../contexts/network', () => ({ useNetwork: () => useNetworkMock() }))

const CUSTOM_NETWORK: NetworkConfig = {
  id: 'custom',
  name: 'Custom',
  rpcEndpoint: 'https://node-1tsb0k9x2q.testnet.botho.io/rpc',
  networkId: 'botho-custom',
  isTestnet: false,
}

const SEED_NETWORK: NetworkConfig = {
  id: 'testnet:seed',
  name: 'Testnet',
  rpcEndpoint: 'https://seed.botho.io/rpc',
  networkId: 'botho-testnet',
  isTestnet: true,
}

interface Overrides {
  ingressId?: string
  network?: NetworkConfig
  setCustomEndpoint?: (endpoint: string) => Promise<boolean>
  isValidating?: boolean
  validationError?: string | null
}

function setup(overrides: Overrides = {}) {
  const setCustomEndpoint = overrides.setCustomEndpoint ?? vi.fn(async () => true)
  const nodeHealth: Record<string, NodeHealth> = {}
  for (const n of INGRESS_NODES) nodeHealth[n.id] = { status: 'online', chainHeight: 1, synced: true }
  useNetworkMock.mockReturnValue({
    network: overrides.network ?? SEED_NETWORK,
    ingressId: overrides.ingressId ?? 'seed',
    ingressNodes: INGRESS_NODES,
    nodeHealth,
    selectIngress: vi.fn(),
    setCustomEndpoint,
    isValidating: overrides.isValidating ?? false,
    validationError: overrides.validationError ?? null,
    startHealthPolling: vi.fn(() => () => {}),
  })
  return { setCustomEndpoint }
}

describe('NetworkSelector trigger label', () => {
  beforeEach(() => {
    cleanup()
    useNetworkMock.mockReset()
  })

  it('shows the selected built-in ingress name when not on a custom node', () => {
    setup({ ingressId: 'seed', network: SEED_NETWORK })
    render(<NetworkSelector />)
    expect(screen.getByText('US seed 1')).toBeTruthy()
  })

  it('shows the truncated custom host (not "Custom RPC") when on a custom node', () => {
    setup({ ingressId: 'custom', network: CUSTOM_NETWORK })
    render(<NetworkSelector />)
    // Long leftmost label is shortened; operator domain stays intact.
    expect(screen.getByText('node-1tsb0….testnet.botho.io')).toBeTruthy()
    // The literal generic label is NOT used on the collapsed trigger.
    expect(screen.queryByText(/^Custom RPC$/)).toBeNull()
  })
})

describe('NetworkSelector manual custom-endpoint entry', () => {
  beforeEach(() => {
    cleanup()
    useNetworkMock.mockReset()
  })

  function openCustomInput() {
    // Open the dropdown, then reveal the custom-endpoint input row.
    fireEvent.click(screen.getByRole('button', { name: /US seed 1/i }))
    fireEvent.click(screen.getByText('Custom RPC'))
  }

  it('routes a pasted endpoint through setCustomEndpoint', async () => {
    const setCustomEndpoint = vi.fn(async () => true)
    setup({ setCustomEndpoint })
    render(<NetworkSelector />)
    openCustomInput()

    const input = screen.getByPlaceholderText('https://node.example.com/rpc')
    fireEvent.change(input, { target: { value: 'https://node-x.testnet.botho.io/rpc' } })
    fireEvent.click(screen.getByRole('button', { name: /connect/i }))

    await waitFor(() =>
      expect(setCustomEndpoint).toHaveBeenCalledWith('https://node-x.testnet.botho.io/rpc'),
    )
  })

  it('surfaces the validation error when the endpoint is rejected', () => {
    setup({
      setCustomEndpoint: vi.fn(async () => false),
      validationError: 'This node is on a different network (botho-mainnet)',
    })
    render(<NetworkSelector />)
    openCustomInput()
    expect(screen.getByText('This node is on a different network (botho-mainnet)')).toBeTruthy()
  })

  it('does not call setCustomEndpoint for an empty input', () => {
    const setCustomEndpoint = vi.fn(async () => true)
    setup({ setCustomEndpoint })
    render(<NetworkSelector />)
    openCustomInput()
    // Connect is disabled while the input is empty.
    const connect = screen.getByRole('button', { name: /connect/i }) as HTMLButtonElement
    expect(connect.disabled).toBe(true)
    fireEvent.click(connect)
    expect(setCustomEndpoint).not.toHaveBeenCalled()
  })
})
